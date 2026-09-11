// Trouter real-time push for Teams chat messages.
//
// Implements the Skype/Teams "Trouter" notification protocol (socket.io 0.9 over a
// WebSocket) so we receive new chat messages the instant they arrive, instead of
// polling. Flow:
//   1. POST go.trouter.teams.microsoft.com/v4/a  -> socketio url, surl, connectparams
//   2. GET  {socketio}socket.io/1/?<params>      -> session id (socket.io 0.9 handshake)
//   3. WS   wss://.../socket.io/1/websocket/<session>?<params>
//   4. on frame "1" (connected): send user.authenticate, then POST registrar to route
//      message notifications to our trouter `surl`.
//   5. incoming events arrive as "3:::{id,method,url,headers,body}"; ack each with
//      "3:::{id,status:200,body:\"\"}" and surface the ones we understand.
//
// Events come from two url families. A /messaging url carries chat activity: new
// messages, message updates (edits and reactions), read horizon updates and typing.
// A unifiedPresenceService url carries availability changes. See `TrouterEvent`.
//
// Protocol reference: EionRobb/purple-teams (teams_trouter.c).

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{TeamsClient, SCOPE_CHATSVCAGG};

/// A chat message delivered over Trouter.
#[derive(Debug, Clone)]
pub struct TrouterMessage {
    pub chat_id: String,
    pub from_mri: String,
    pub from: String,
    pub content: String,
    pub message_id: String,
    pub message_type: String,
}

/// A real-time event delivered over Trouter.
#[derive(Debug, Clone)]
pub enum TrouterEvent {
    /// A new chat message.
    NewMessage(TrouterMessage),
    /// An existing message changed: an edit, or a reaction landing on it.
    MessageUpdate(TrouterMessage),
    /// Someone moved their read marker in a chat.
    ReadHorizon { chat_id: String },
    /// Someone is typing in a chat.
    Typing { chat_id: String, from: String },
    /// A user's availability changed.
    Presence {
        user_id: String,
        availability: String,
    },
}

impl TeamsClient {
    /// Connect to Trouter and invoke `on_event` for each event we understand.
    /// Returns when the connection closes or errors (caller may reconnect).
    pub async fn trouter_listen<F>(&self, mut on_event: F) -> Result<()>
    where
        F: FnMut(TrouterEvent),
    {
        let debug = std::env::var("SQUADS_TROUTER_DEBUG").is_ok();
        let skype = self.get_skype_token().await?;
        let bearer = self.get_token(SCOPE_CHATSVCAGG).await?;
        let epid = uuid::Uuid::new_v4().to_string();
        if debug {
            eprintln!("[trouter] registering epid={epid}");
        }

        // 1. Trouter registration
        let reg_url = format!(
            "https://go.trouter.teams.microsoft.com/v4/a?epid={}",
            urlencoding::encode(&epid)
        );
        let reg: Value = self
            .http
            .post(&reg_url)
            .header("x-skypetoken", &skype.value)
            .header("content-length", "0")
            .send()
            .await?
            .json()
            .await?;

        let mut socketio = reg["socketio"]
            .as_str()
            .unwrap_or("https://go.trouter.teams.microsoft.com/")
            .to_string();
        // ensure a trailing slash so `{socketio}socket.io/1/...` is well-formed
        if !socketio.ends_with('/') {
            socketio.push('/');
        }
        let surl = reg["surl"]
            .as_str()
            .ok_or_else(|| anyhow!("trouter response missing surl"))?
            .to_string();
        let ccid = reg["ccid"].as_str().map(|s| s.to_string());
        let connectparams = reg["connectparams"].clone();

        // build the shared query string (connectparams + tc + con_num + epid + ccid)
        let mut cp_q = String::new();
        if let Some(obj) = connectparams.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    cp_q.push_str(&format!("{}={}&", k, urlencoding::encode(s)));
                }
            }
        }
        let tc =
            urlencoding::encode(r#"{"cv":"2024.23.01.2","ua":"TeamsCDL","hr":"","v":"1.0.0"}"#);
        let con_num = Utc::now().timestamp_millis();
        let ccid_q = ccid
            .as_ref()
            .map(|c| format!("&ccid={}", urlencoding::encode(c)))
            .unwrap_or_default();
        let query = format!(
            "v=v4&{}tc={}&con_num={}&epid={}{}&auth=true&timeout=40",
            cp_q,
            tc,
            con_num,
            urlencoding::encode(&epid),
            ccid_q
        );

        // 2. socket.io 0.9 handshake -> session id
        let hs_url = format!("{}socket.io/1/?{}", socketio, query);
        let hs = self
            .http
            .get(&hs_url)
            .header("x-skypetoken", &skype.value)
            .send()
            .await?
            .text()
            .await?;
        let session_id = hs
            .split(':')
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("bad socket.io handshake: {hs}"))?
            .to_string();

        // 3. WebSocket connect
        let host = socketio
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let ws_url = format!(
            "wss://{}/socket.io/1/websocket/{}?{}",
            host, session_id, query
        );
        let mut request = ws_url.into_client_request()?;
        request
            .headers_mut()
            .insert("x-skypetoken", HeaderValue::from_str(&skype.value)?);
        let (ws, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut write, mut read) = ws.split();

        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ping.tick().await; // consume the immediate first tick so we don't ping before auth
        let mut cmd_count: u64 = 0;

        // Force a clean reconnect every few hours so tokens + the registrar
        // subscription are refreshed on a long-lived session.
        let max_session = tokio::time::sleep(Duration::from_secs(5 * 3600));
        tokio::pin!(max_session);

        loop {
            tokio::select! {
                _ = &mut max_session => { break; }
                _ = ping.tick() => {
                    cmd_count += 1;
                    let f = format!("5:{}+::{{\"name\":\"ping\"}}", cmd_count);
                    if write.send(WsMessage::Text(f.into())).await.is_err() { break; }
                }
                frame = read.next() => {
                    let frame = match frame {
                        Some(Ok(f)) => f,
                        _ => break,
                    };
                    let txt = match frame {
                        WsMessage::Text(t) => t.as_str().to_string(),
                        WsMessage::Ping(p) => { let _ = write.send(WsMessage::Pong(p)).await; continue; }
                        WsMessage::Close(_) => break,
                        _ => continue,
                    };
                    if txt.is_empty() { continue; }
                    if debug {
                        let head: String = txt.chars().take(160).collect();
                        eprintln!("[trouter] <- {head}");
                    }
                    match txt.as_bytes()[0] {
                        b'1' => {
                            // connected: authenticate over the socket, then register via HTTP
                            let mut auth_args = json!({
                                "headers": {
                                    "X-Ms-Test-User": "False",
                                    "Authorization": format!("Bearer {}", bearer.value),
                                    "X-MS-Migration": "True"
                                }
                            });
                            if !connectparams.is_null() {
                                auth_args["connectparams"] = connectparams.clone();
                            }
                            let auth = json!({"name": "user.authenticate", "args": [auth_args]});
                            let _ = write.send(WsMessage::Text(format!("5:::{}", auth).into())).await;
                            if let Err(e) = self.trouter_register(&skype.value, &bearer.value, &surl, &epid).await {
                                tracing::warn!("registrar failed: {e}");
                            }
                        }
                        b'3' => {
                            if let Some(payload) = after_nth_colon(&txt, 3) {
                                if let Ok(req) = serde_json::from_str::<Value>(payload) {
                                    // ack the request on the socket
                                    let ack = json!({"id": req["id"], "status": 200, "body": ""});
                                    let _ = write.send(WsMessage::Text(format!("3:::{}", ack).into())).await;
                                    if let Some(ev) = parse_event(&req) {
                                        if debug {
                                            match &ev {
                                                TrouterEvent::NewMessage(m) | TrouterEvent::MessageUpdate(m) => eprintln!("[trouter] msg from={} chat={} : {}", m.from, m.chat_id, m.content.chars().take(80).collect::<String>()),
                                                other => eprintln!("[trouter] {other:?}"),
                                            }
                                        }
                                        on_event(ev);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Register our trouter endpoint with the Teams registrar so message notifications
    /// are routed to it.
    async fn trouter_register(
        &self,
        skype: &str,
        bearer: &str,
        surl: &str,
        epid: &str,
    ) -> Result<()> {
        let url = "https://teams.microsoft.com/registrar/prod/V2/registrations";
        let body = json!({
            "clientDescription": {
                "appId": "TeamsCDLWebWorker",
                "aesKey": "",
                "languageId": "en-US",
                "platform": "edge",
                "templateKey": "TeamsCDLWebWorker_2.1",
                "platformUIVersion": "1.0.0"
            },
            "registrationId": epid,
            "nodeId": "",
            "transports": {
                "TROUTER": [{
                    "context": "",
                    "path": surl,
                    "ttl": 86400
                }]
            }
        });
        let res = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .header("x-skypetoken", skype)
            .header("authorization", format!("Bearer {}", bearer))
            .body(body.to_string())
            .send()
            .await?;
        if !res.status().is_success() {
            let s = res.status();
            return Err(anyhow!("registrar returned {s}"));
        }
        Ok(())
    }
}

/// Return the substring after the nth ':' in a socket.io frame, or None.
fn after_nth_colon(s: &str, n: usize) -> Option<&str> {
    let mut seen = 0;
    for (i, c) in s.char_indices() {
        if c == ':' {
            seen += 1;
            if seen == n {
                return Some(&s[i + 1..]);
            }
        }
    }
    None
}

/// Decode a Trouter request `body` (string) into JSON, handling optional gzip+base64
/// at the envelope level and the nested `cp` (gzip+base64) / `gp` (base64) payloads.
fn decode_body(req: &Value) -> Option<Value> {
    let body = req["body"].as_str()?;
    let gzip = req["headers"]
        .get("X-Microsoft-Skype-Content-Encoding")
        .and_then(|v| v.as_str())
        == Some("gzip");
    let outer: Value = if gzip {
        serde_json::from_str(&gunzip_b64(body)?).ok()?
    } else {
        serde_json::from_str(body).ok()?
    };
    // The real payload is sometimes wrapped: `cp` = gzip+base64, `gp` = base64.
    if let Some(cp) = outer.get("cp").and_then(|v| v.as_str()) {
        return serde_json::from_str(&gunzip_b64(cp)?).ok();
    }
    if let Some(gp) = outer.get("gp").and_then(|v| v.as_str()) {
        let raw = B64.decode(gp).ok()?;
        return serde_json::from_slice(&raw).ok();
    }
    Some(outer)
}

/// base64-decode then gunzip to a UTF-8 string.
fn gunzip_b64(s: &str) -> Option<String> {
    let raw = B64.decode(s).ok()?;
    let mut gz = flate2::read::GzDecoder::new(&raw[..]);
    let mut out = String::new();
    gz.read_to_string(&mut out).ok()?;
    Some(out)
}

/// Turn a request envelope into an event, or None if we do not handle it.
fn parse_event(req: &Value) -> Option<TrouterEvent> {
    let url = req["url"].as_str()?;
    if url.contains("unifiedPresenceService") {
        return parse_presence(req);
    }
    if !url.ends_with("/messaging") {
        log_unhandled(url, "not a messaging url");
        return None;
    }

    let body = decode_body(req)?;
    let resource = body.get("resource")?;
    let resource_type = body["resourceType"].as_str().unwrap_or_default();
    let message_type = resource["messagetype"].as_str().unwrap_or_default();
    let chat_id = extract_chat_id(resource["conversationLink"].as_str()?)?;

    // Teams also labels control messages as resourceType "NewMessage", so the
    // messagetype cases must be matched first or they never reach their branch.
    match (message_type, resource_type) {
        ("ThreadActivity/MemberConsumptionHorizonUpdate", _) => {
            Some(TrouterEvent::ReadHorizon { chat_id })
        }
        ("Control/Typing", _) => {
            let from = resource["imdisplayname"].as_str().unwrap_or_default();
            if from.is_empty() {
                return None;
            }
            Some(TrouterEvent::Typing {
                chat_id,
                from: from.to_string(),
            })
        }
        ("RichText/Html" | "Text", "NewMessage") => {
            Some(TrouterEvent::NewMessage(parse_message(resource, chat_id)))
        }
        (_, "MessageUpdate") => Some(TrouterEvent::MessageUpdate(parse_message(
            resource, chat_id,
        ))),
        _ => {
            log_unhandled(
                url,
                &format!("resourceType={resource_type} messagetype={message_type}"),
            );
            None
        }
    }
}

/// Build a TrouterMessage from a message `resource` object.
fn parse_message(resource: &Value, chat_id: String) -> TrouterMessage {
    let field = |k: &str| resource[k].as_str().unwrap_or_default().to_string();
    // `from` is a contacts URL on most events and a bare MRI on some, and an update
    // may omit it entirely, so take the last path segment of whatever is there.
    let from_mri = resource["from"]
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    TrouterMessage {
        chat_id,
        from_mri,
        from: field("imdisplayname"),
        content: field("content"),
        message_id: field("id"),
        message_type: field("messagetype"),
    }
}

/// Extract an availability change from a unifiedPresenceService envelope.
fn parse_presence(req: &Value) -> Option<TrouterEvent> {
    let body = decode_body(req)?;
    // A frame can carry several users. We do not send presence subscriptions yet,
    // so the first entry is all we get; widen this when we do.
    let entry = body["presence"].as_array()?.first()?;
    let mri = entry["mri"].as_str()?;
    let user_id = mri.strip_prefix("8:orgid:").unwrap_or(mri);
    let availability = entry["presence"]["availability"].as_str()?;
    if user_id.is_empty() || availability.is_empty() {
        return None;
    }
    Some(TrouterEvent::Presence {
        user_id: user_id.to_string(),
        availability: availability.to_string(),
    })
}

/// Report an event we drop, so unknown types are discoverable instead of invisible.
fn log_unhandled(url: &str, detail: &str) {
    if std::env::var("SQUADS_TROUTER_DEBUG").is_ok() {
        eprintln!("[trouter] unhandled url={url} {detail}");
    }
}

/// Pull the conversation/chat id out of a conversationLink URL.
fn extract_chat_id(link: &str) -> Option<String> {
    let after = link.split("/conversations/").nth(1)?;
    let id = after.split('/').next()?.split(';').next()?;
    Some(
        urlencoding::decode(id)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| id.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const LINK: &str = "https://emea.notifications.skype.net/v1/users/ME/conversations/19:abc123@thread.v2/messages/1700000000000";

    /// Wrap a decoded body in the request envelope Trouter sends.
    fn envelope(url: &str, body: Value) -> Value {
        json!({"id": 7, "method": "POST", "url": url, "headers": {}, "body": body.to_string()})
    }

    fn messaging(resource_type: &str, resource: Value) -> Value {
        envelope(
            "https://trouter.teams.microsoft.com/v4/f/x/messaging",
            json!({"type": "EventMessage", "resourceType": resource_type, "resource": resource}),
        )
    }

    fn gzip_b64(s: &str) -> String {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(s.as_bytes()).unwrap();
        B64.encode(enc.finish().unwrap())
    }

    #[test]
    fn parses_new_message() {
        let req = messaging(
            "NewMessage",
            json!({
                "id": "1700000000000",
                "messagetype": "RichText/Html",
                "conversationLink": LINK,
                "from": "https://notifications.skype.net/v1/users/ME/contacts/8:orgid:u1",
                "imdisplayname": "Ada Fenwick",
                "content": "<p>hello</p>",
            }),
        );
        match parse_event(&req) {
            Some(TrouterEvent::NewMessage(m)) => {
                assert_eq!(m.chat_id, "19:abc123@thread.v2");
                assert_eq!(m.from_mri, "8:orgid:u1");
                assert_eq!(m.from, "Ada Fenwick");
                assert_eq!(m.content, "<p>hello</p>");
                assert_eq!(m.message_id, "1700000000000");
                assert_eq!(m.message_type, "RichText/Html");
            }
            other => panic!("expected NewMessage, got {other:?}"),
        }
    }

    #[test]
    fn parses_message_update() {
        let req = messaging(
            "MessageUpdate",
            json!({
                "id": "1700000000001",
                "messagetype": "RichText/Html",
                "conversationLink": LINK,
                "content": "<p>edited</p>",
            }),
        );
        match parse_event(&req) {
            Some(TrouterEvent::MessageUpdate(m)) => {
                assert_eq!(m.chat_id, "19:abc123@thread.v2");
                assert_eq!(m.content, "<p>edited</p>");
            }
            other => panic!("expected MessageUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parses_read_horizon() {
        let req = messaging(
            "NewMessage",
            json!({
                "messagetype": "ThreadActivity/MemberConsumptionHorizonUpdate",
                "conversationLink": LINK,
            }),
        );
        match parse_event(&req) {
            Some(TrouterEvent::ReadHorizon { chat_id }) => {
                assert_eq!(chat_id, "19:abc123@thread.v2")
            }
            other => panic!("expected ReadHorizon, got {other:?}"),
        }
    }

    #[test]
    fn parses_typing() {
        let req = messaging(
            "NewMessage",
            json!({
                "messagetype": "Control/Typing",
                "conversationLink": LINK,
                "imdisplayname": "Ada Fenwick",
            }),
        );
        match parse_event(&req) {
            Some(TrouterEvent::Typing { chat_id, from }) => {
                assert_eq!(chat_id, "19:abc123@thread.v2");
                assert_eq!(from, "Ada Fenwick");
            }
            other => panic!("expected Typing, got {other:?}"),
        }
    }

    #[test]
    fn parses_presence() {
        let req = envelope(
            "https://trouter.teams.microsoft.com/v4/f/x/unifiedPresenceService",
            json!({"presence": [{
                "mri": "8:orgid:11111111-2222-3333-4444-555555555555",
                "presence": {"availability": "Available"}
            }]}),
        );
        match parse_event(&req) {
            Some(TrouterEvent::Presence {
                user_id,
                availability,
            }) => {
                assert_eq!(user_id, "11111111-2222-3333-4444-555555555555");
                assert_eq!(availability, "Available");
            }
            other => panic!("expected Presence, got {other:?}"),
        }
    }

    #[test]
    fn ignores_unknown_resource() {
        let req = messaging(
            "ConversationUpdate",
            json!({"messagetype": "ThreadActivity/AddMember", "conversationLink": LINK}),
        );
        assert!(parse_event(&req).is_none());
    }

    #[test]
    fn ignores_non_messaging_url() {
        let req = envelope(
            "https://trouter.teams.microsoft.com/v4/f/x/callingMessages",
            json!({"resourceType": "NewMessage"}),
        );
        assert!(parse_event(&req).is_none());
    }

    #[test]
    fn decodes_plain_body() {
        let req = envelope("https://x/messaging", json!({"a": 1}));
        assert_eq!(decode_body(&req).unwrap()["a"], 1);
    }

    #[test]
    fn decodes_gzipped_envelope() {
        let req = json!({
            "url": "https://x/messaging",
            "headers": {"X-Microsoft-Skype-Content-Encoding": "gzip"},
            "body": gzip_b64(r#"{"a":1}"#),
        });
        assert_eq!(decode_body(&req).unwrap()["a"], 1);
    }

    #[test]
    fn decodes_cp_payload() {
        let req = envelope("https://x/messaging", json!({"cp": gzip_b64(r#"{"a":2}"#)}));
        assert_eq!(decode_body(&req).unwrap()["a"], 2);
    }

    #[test]
    fn decodes_gp_payload() {
        let req = envelope(
            "https://x/messaging",
            json!({"gp": B64.encode(r#"{"a":3}"#)}),
        );
        assert_eq!(decode_body(&req).unwrap()["a"], 3);
    }

    #[test]
    fn extracts_chat_id_from_link() {
        assert_eq!(
            extract_chat_id(LINK).unwrap(),
            "19:abc123@thread.v2".to_string()
        );
    }
}
