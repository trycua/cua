//! Deterministic in-process mock CDP endpoint for tests.
//!
//! A tiny scripted WebSocket server on a loopback port: each incoming
//! command is handed to a caller-supplied handler which returns the
//! events to emit *before* the reply plus the reply itself. Because
//! events precede the response frame on the wire — exactly Chromium's
//! behavior for `Target.setAutoAttach` announcing existing targets —
//! tests of the event demux and the OOPIF attach flow are fully
//! deterministic (no sleeps, no polling).

use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// One decoded incoming CDP command.
pub(crate) struct MockCall {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

/// An event frame the mock emits before a reply.
pub(crate) struct MockEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

/// The scripted reaction to one command.
pub(crate) struct MockReply {
    pub events: Vec<MockEvent>,
    pub result: Result<Value, (i64, String)>,
}

impl MockReply {
    pub fn ok(result: Value) -> Self {
        Self {
            events: Vec::new(),
            result: Ok(result),
        }
    }

    pub fn err(code: i64, message: &str) -> Self {
        Self {
            events: Vec::new(),
            result: Err((code, message.to_owned())),
        }
    }

    /// The exact shape Chromium uses for an unimplemented method.
    pub fn method_not_found(method: &str) -> Self {
        Self::err(-32601, &format!("'{method}' wasn't found"))
    }

    pub fn with_events(mut self, events: Vec<MockEvent>) -> Self {
        self.events = events;
        self
    }
}

pub(crate) type MockHandler = Arc<dyn Fn(&MockCall) -> MockReply + Send + Sync>;

/// A running mock endpoint. Dropping it aborts the accept loop.
pub(crate) struct MockCdpServer {
    addr: SocketAddr,
    accept_task: tokio::task::JoinHandle<()>,
}

impl MockCdpServer {
    pub async fn start(handler: MockHandler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    while let Some(Ok(msg)) = ws.next().await {
                        let Message::Text(text) = msg else { continue };
                        let Ok(v) = serde_json::from_str::<Value>(&text) else {
                            continue;
                        };
                        let Some(id) = v.get("id").and_then(Value::as_u64) else {
                            continue;
                        };
                        let call = MockCall {
                            method: v
                                .get("method")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            params: v.get("params").cloned().unwrap_or_else(|| json!({})),
                            session_id: v
                                .get("sessionId")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        };
                        let reply = handler(&call);
                        for event in reply.events {
                            let mut frame =
                                json!({ "method": event.method, "params": event.params });
                            if let Some(sid) = event.session_id {
                                frame["sessionId"] = json!(sid);
                            }
                            if ws.send(Message::Text(frame.to_string())).await.is_err() {
                                return;
                            }
                        }
                        let mut response = match reply.result {
                            Ok(result) => json!({ "id": id, "result": result }),
                            Err((code, message)) => {
                                json!({ "id": id, "error": { "code": code, "message": message } })
                            }
                        };
                        if let Some(sid) = &call.session_id {
                            response["sessionId"] = json!(sid);
                        }
                        if ws.send(Message::Text(response.to_string())).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { addr, accept_task }
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/devtools/browser/mock", self.addr.port())
    }
}

impl Drop for MockCdpServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}
