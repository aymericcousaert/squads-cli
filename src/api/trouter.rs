// Trouter real-time push for Teams chat messages.
//
// Implements the Skype/Teams "Trouter" notification protocol (socket.io 0.9 over a
// WebSocket) so we receive new chat messages the instant they arrive, instead of
// polling. Flow:
//   1. WS wss://go-<code>.trouter.teams.microsoft.com/v4/c?<tc,timeout,epid,ccid,
//      cor_id,con_num>, or the reconnect url the last session gave us.
//   2. on frame "1::" (connected): send user.authenticate over the socket.
//   3. the server answers with a "trouter.connected" frame carrying our `surl`:
//      POST it to the registrar so message notifications are routed to us, and
//      keep its `reconnectUrl` for the next connect.
//   4. incoming events arrive as "3:::{id,method,url,headers,body}"; ack each with
//      "3:::{id,status:200,body:\"\"}" and surface the ones we understand.
//   5. a frame whose sequence field carries a "+" wants an ack: "6:::<seq>+[]".
//      A server ping wants "6:::<seq>+[\"pong\"]" instead, and we send a bare
//      "2::" heartbeat every 15 seconds.
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

/// Client descriptor the service expects in the connect query.
const TROUTER_TC: &str = r#"{"cv":"2024.23.01.2","ua":"TeamsCDL","hr":"","v":"1.0.0"}"#;

/// Origin the service accepts a web-worker client from.
const TROUTER_ORIGIN: &str = "https://teams.cloud.microsoft";

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
    /// The service dropped notifications it could not deliver. Whatever they
    /// carried has to be picked up by a resync.
    MessageLoss,
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
        let epid = self.trouter_epid().to_string();

        // A reconnect url points at the node that held the last session, so the
        // service prefers it over the regional entry point.
        let reconnect = self.trouter_reconnect_url();
        let base = match &reconnect {
            Some(url) => url.clone(),
            None => self.regional().await.trouter_default_url(),
        };
        let query = connect_query(
            &epid,
            &uuid::Uuid::new_v4().to_string(),
            &format!("{}_0", Utc::now().timestamp_millis()),
        );
        if debug {
            eprintln!("[trouter] connecting epid={epid} url={base}");
        }

        let mut request = format!("{base}?{query}").into_client_request()?;
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_static(TROUTER_ORIGIN));
        let ws = match tokio_tungstenite::connect_async(request).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                // A reconnect url we cannot even dial would trap us in a retry loop.
                if reconnect.is_some() {
                    self.set_trouter_reconnect_url(None);
                }
                return Err(e.into());
            }
        };
        let (mut write, mut read) = ws.split();

        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await; // consume the immediate first tick
        let mut connected = false;

        // Force a clean reconnect every few hours so tokens + the registrar
        // subscription are refreshed on a long-lived session.
        let max_session = tokio::time::sleep(Duration::from_secs(5 * 3600));
        tokio::pin!(max_session);

        loop {
            tokio::select! {
                _ = &mut max_session => { break; }
                _ = heartbeat.tick() => {
                    // Nothing to keep alive until the service says the session is up.
                    if !connected { continue; }
                    if write.send(WsMessage::Text("2::".into())).await.is_err() { break; }
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

                    if txt.starts_with("1::") {
                        let auth = json!({"name": "user.authenticate", "args": [{
                            "headers": {
                                "Authorization": format!("Bearer {}", bearer.value),
                                "X-MS-Migration": "True"
                            }
                        }]});
                        let _ = write.send(WsMessage::Text(format!("5:::{auth}").into())).await;
                    } else if txt.contains("trouter.connected") {
                        if let Some(ack) = ack_frame(&txt, "[]") {
                            let _ = write.send(WsMessage::Text(ack.into())).await;
                        }
                        connected = true;
                        let data: Value = serde_json::from_str(frame_data(&txt)).unwrap_or(Value::Null);
                        let args = &data["args"][0];
                        self.set_trouter_reconnect_url(
                            args["reconnectUrl"].as_str().filter(|u| !u.is_empty()).map(str::to_string),
                        );
                        match args["surl"].as_str() {
                            Some(surl) => {
                                if let Err(e) = self.trouter_register(&skype.value, &bearer.value, surl, &epid).await {
                                    tracing::warn!("registrar failed: {e}");
                                }
                            }
                            None => tracing::warn!("trouter.connected carried no surl"),
                        }
                    } else if txt.contains(r#""name":"ping""#) {
                        if let Some(seq) = frame_seq(&txt) {
                            let pong = format!(r#"6:::{seq}+["pong"]"#);
                            let _ = write.send(WsMessage::Text(pong.into())).await;
                        }
                    } else if txt.starts_with("3:::") {
                        if let Ok(req) = serde_json::from_str::<Value>(frame_data(&txt)) {
                            // ack the request on the socket
                            let ack = json!({"id": req["id"], "status": 200, "body": ""});
                            let _ = write.send(WsMessage::Text(format!("3:::{ack}").into())).await;
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
                    } else if txt.contains("message_loss") {
                        if let Some(ack) = ack_frame(&txt, "[]") {
                            let _ = write.send(WsMessage::Text(ack.into())).await;
                        }
                        if debug {
                            eprintln!("[trouter] message_loss, a resync is needed");
                        }
                        on_event(TrouterEvent::MessageLoss);
                    }
                }
            }
        }

        // The reconnect url never produced a session, so it is stale.
        if reconnect.is_some() && !connected {
            self.set_trouter_reconnect_url(None);
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

/// Build the Trouter connect query. `ccid` is sent empty, as the web client does
/// when it has no cluster affinity to ask for.
fn connect_query(epid: &str, cor_id: &str, con_num: &str) -> String {
    format!(
        "tc={}&timeout=40&epid={}&ccid=&cor_id={}&con_num={}",
        urlencoding::encode(TROUTER_TC),
        urlencoding::encode(epid),
        urlencoding::encode(cor_id),
        urlencoding::encode(con_num)
    )
}

/// A socket.io frame is `<type>:<seq>:<endpoint>:<data>`, and the data itself
/// holds colons, so everything after the third one is the payload.
fn frame_data(frame: &str) -> &str {
    let mut seen = 0;
    for (i, c) in frame.char_indices() {
        if c == ':' {
            seen += 1;
            if seen == 3 {
                return &frame[i + 1..];
            }
        }
    }
    ""
}

/// Sequence number of a frame, without the ack marker.
fn frame_seq(frame: &str) -> Option<String> {
    frame.split(':').nth(1).map(|seq| seq.replace('+', ""))
}

/// A trailing "+" on the sequence means the server waits for an ack.
fn needs_ack(frame: &str) -> bool {
    frame.split(':').nth(1).is_some_and(|seq| seq.contains('+'))
}

/// Ack for a frame that asks for one, carrying `args` as its payload.
fn ack_frame(frame: &str, args: &str) -> Option<String> {
    if !needs_ack(frame) {
        return None;
    }
    Some(format!("6:::{}+{}", frame_seq(frame)?, args))
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

    /// The real frame the service sends once the session is live. Its JSON body
    /// is full of colons, which is what `frame_data` has to survive.
    const CONNECTED: &str = r#"5:2+::{"name":"trouter.connected","args":[{"surl":"https://go-eu.trouter.teams.microsoft.com/v4/f/abc/","reconnectUrl":"wss://go-eu.trouter.teams.microsoft.com/v4/c/abc"}]}"#;

    #[test]
    fn reads_the_frame_sequence() {
        assert_eq!(frame_seq(CONNECTED).as_deref(), Some("2"));
        assert_eq!(frame_seq("5:11+::{}").as_deref(), Some("11"));
        assert_eq!(frame_seq("3:::{\"id\":7}").as_deref(), Some(""));
        assert_eq!(frame_seq("2::").as_deref(), Some(""));
        assert_eq!(frame_seq("1").as_deref(), None);
    }

    #[test]
    fn only_a_plus_asks_for_an_ack() {
        assert!(needs_ack(CONNECTED));
        assert!(needs_ack("5:9+::{\"name\":\"ping\"}"));
        assert!(!needs_ack("3:::{\"id\":7}"));
        assert!(!needs_ack("1::"));
        assert!(!needs_ack("2::"));
    }

    #[test]
    fn acks_only_the_frames_that_ask() {
        assert_eq!(ack_frame(CONNECTED, "[]").as_deref(), Some("6:::2+[]"));
        assert_eq!(
            ack_frame("5:9+::{\"name\":\"ping\"}", r#"["pong"]"#).as_deref(),
            Some(r#"6:::9+["pong"]"#)
        );
        assert_eq!(ack_frame("3:::{}", "[]"), None);
    }

    #[test]
    fn frame_data_keeps_the_colons_of_the_payload() {
        let data: Value = serde_json::from_str(frame_data(CONNECTED)).unwrap();
        assert_eq!(data["name"], "trouter.connected");
        assert_eq!(
            data["args"][0]["reconnectUrl"],
            "wss://go-eu.trouter.teams.microsoft.com/v4/c/abc"
        );
        assert_eq!(frame_data("3:::{\"id\":7}"), "{\"id\":7}");
        assert_eq!(frame_data("2::"), "");
        assert_eq!(frame_data("1"), "");
    }

    #[test]
    fn builds_the_connect_query() {
        let query = connect_query("epid-1", "cor-1", "1700000000000_0");
        assert!(
            query.ends_with("&timeout=40&epid=epid-1&ccid=&cor_id=cor-1&con_num=1700000000000_0")
        );
        let tc = query
            .strip_prefix("tc=")
            .and_then(|rest| rest.split('&').next())
            .unwrap();
        assert!(!tc.contains('{'), "tc must be url-encoded");
        let decoded = urlencoding::decode(tc).unwrap();
        let tc: Value = serde_json::from_str(&decoded).unwrap();
        assert_eq!(tc["ua"], "TeamsCDL");
    }

    #[test]
    fn extracts_chat_id_from_link() {
        assert_eq!(
            extract_chat_id(LINK).unwrap(),
            "19:abc123@thread.v2".to_string()
        );
    }
}
