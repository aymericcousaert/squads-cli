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
//      "3:::{id,status:200,body:\"\"}" and, for a /messaging EventMessage, surface it.
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
use crate::types::Message;

/// A new chat message delivered over Trouter.
#[derive(Debug, Clone)]
pub struct TrouterMessage {
    pub chat_id: String,
    pub from_mri: String,
    pub from: String,
    pub content: String,
    pub message_id: String,
    pub message_type: String,
}

impl TeamsClient {
    /// Connect to Trouter and invoke `on_message` for each new chat message.
    /// Returns when the connection closes or errors (caller may reconnect).
    pub async fn trouter_listen<F>(&self, mut on_message: F) -> Result<()>
    where
        F: FnMut(TrouterMessage),
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

        let socketio = reg["socketio"]
            .as_str()
            .unwrap_or("https://go.trouter.teams.microsoft.com/")
            .to_string();
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
        let tc = urlencoding::encode(r#"{"cv":"2024.23.01.2","ua":"TeamsCDL","hr":"","v":"1.0.0"}"#);
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
        let ws_url = format!("wss://{}/socket.io/1/websocket/{}?{}", host, session_id, query);
        let mut request = ws_url.into_client_request()?;
        request
            .headers_mut()
            .insert("x-skypetoken", HeaderValue::from_str(&skype.value)?);
        let (ws, _) = tokio_tungstenite::connect_async(request).await?;
        let (mut write, mut read) = ws.split();

        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cmd_count: u64 = 0;

        loop {
            tokio::select! {
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
                            let auth = json!({
                                "name": "user.authenticate",
                                "args": [{
                                    "headers": {
                                        "X-Ms-Test-User": "False",
                                        "Authorization": format!("Bearer {}", bearer.value),
                                        "X-MS-Migration": "True"
                                    },
                                    "connectparams": connectparams
                                }]
                            });
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
                                    if let Some(m) = parse_event_message(&req) {
                                        if debug {
                                            eprintln!("[trouter] msg from={} chat={} : {}", m.from, m.chat_id, m.content.chars().take(80).collect::<String>());
                                        }
                                        on_message(m);
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

/// Decode a Trouter request `body` (string) into JSON, handling optional gzip+base64.
fn decode_body(req: &Value) -> Option<Value> {
    let body = req["body"].as_str()?;
    let gzip = req["headers"]
        .get("X-Microsoft-Skype-Content-Encoding")
        .and_then(|v| v.as_str())
        == Some("gzip");
    if gzip {
        let raw = B64.decode(body).ok()?;
        let mut gz = flate2::read::GzDecoder::new(&raw[..]);
        let mut out = String::new();
        gz.read_to_string(&mut out).ok()?;
        serde_json::from_str(&out).ok()
    } else {
        serde_json::from_str(body).ok()
    }
}

/// Extract a TrouterMessage from a request envelope if it's a new chat message.
fn parse_event_message(req: &Value) -> Option<TrouterMessage> {
    let url = req["url"].as_str()?;
    if !url.ends_with("/messaging") {
        return None;
    }
    let body = decode_body(req)?;
    if body["type"].as_str() != Some("EventMessage") {
        return None;
    }
    if body["resourceType"].as_str() != Some("NewMessage") {
        return None;
    }
    let msg: Message = serde_json::from_value(body["resource"].clone()).ok()?;
    let mt = msg.message_type.unwrap_or_default();
    // only real user messages (skip control / system messages)
    if mt != "RichText/Html" && mt != "Text" {
        return None;
    }
    let chat_id = extract_chat_id(msg.conversation_link.as_deref()?)?;
    Some(TrouterMessage {
        chat_id,
        from_mri: msg.from.unwrap_or_default(),
        from: msg.im_display_name.unwrap_or_default(),
        content: msg.content.unwrap_or_default(),
        message_id: msg.id.unwrap_or_default(),
        message_type: mt,
    })
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
