use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn response(body: &str, media: &str) -> DriverServiceResponse {
    DriverServiceResponse {
        status: 200,
        headers: vec![DriverServiceHeader {
            name: "Content-Type".into(),
            value: media.into(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn strict_parser_rejects_ambiguous_responses() {
    for (body, media) in [
        (
            r#"{"jsonrpc":"2.0","id":"expected","id":"expected","result":{}}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"expected","result":{"a":1,"a":2}}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"expected","result":NaN}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"expected","result":1e999}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":true,"result":{}}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"wrong","result":{}}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"expected","result":{},"error":{"code":1}}"#,
            "application/json",
        ),
        (
            r#"{"jsonrpc":"2.0","id":"expected","error":{"code":true}}"#,
            "application/json",
        ),
        ("data: {}\n", "text/event-stream"),
        ("data: {}\n\ndata: {}\n\n", "text/event-stream"),
        ("event: unexpected\ndata: {}\n\n", "text/event-stream"),
        ("{}", "text/html"),
    ] {
        assert_eq!(
            decode_rpc(&response(body, media), "expected").unwrap_err(),
            MALFORMED,
            "{body}"
        );
    }
    let valid = r#"{"jsonrpc":"2.0","id":"expected","result":{"value":null}}"#;
    assert_eq!(
        decode_rpc(
            &response(valid, "application/json; charset=utf-8"),
            "expected"
        )
        .unwrap(),
        json!({"value":null})
    );
    assert_eq!(
        decode_rpc(
            &response(
                &format!("retry: 100\r\n: comment\r\nevent: message\r\ndata: {valid}\r\n\r\n"),
                "text/event-stream"
            ),
            "expected"
        )
        .unwrap(),
        json!({"value":null})
    );
    let mut oversized = response(valid, "application/json");
    oversized.body = vec![b' '; RESPONSE_LIMIT + 1];
    assert!(decode_rpc(&oversized, "expected").is_err());
}

#[derive(Default)]
struct Transport {
    events: Mutex<Vec<String>>,
    sequence: AtomicUsize,
    unsupported: AtomicBool,
    malformed_init: AtomicBool,
    malformed_open: AtomicBool,
    lost_exchange: AtomicBool,
    hold_init: AtomicBool,
    hold_exchange: AtomicBool,
    init_entered: tokio::sync::Notify,
    init_release: tokio::sync::Notify,
    exchange_entered: tokio::sync::Notify,
    exchange_release: tokio::sync::Notify,
    cancel_entered: tokio::sync::Notify,
}

#[async_trait]
impl DriverServiceTransport for Transport {
    async fn send(
        &self,
        request: DriverServiceRequest,
    ) -> Result<DriverServiceResponse, DriverServiceTransportError> {
        assert_eq!(request.path, "/mcp");
        assert!(request.timeout_ms > 0);
        let body: Value = if request.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&request.body).unwrap()
        };
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("DELETE");
        self.events.lock().unwrap().push(method.into());
        let session = request
            .headers
            .iter()
            .find(|h| h.name == "Mcp-Session-Id")
            .map(|h| h.value.clone());
        if matches!(
            method,
            "cua/driver/v1/exchange" | "cua/driver/v1/cancel" | "cua/driver/v1/close"
        ) {
            assert_eq!(
                body["params"]["connection_id"],
                json!(session.as_ref().unwrap())
            );
            assert_eq!(
                body["params"]["generation"],
                json!(format!("gen-{}", session.as_ref().unwrap()))
            );
        }
        let mut headers = vec![DriverServiceHeader {
            name: "Content-Type".into(),
            value: "application/json".into(),
        }];
        let result = match method {
            "initialize" => {
                assert!(session.is_none());
                self.init_entered.notify_one();
                if self.hold_init.load(Ordering::SeqCst) {
                    self.init_release.notified().await;
                }
                let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
                headers.push(DriverServiceHeader {
                    name: "Mcp-Session-Id".into(),
                    value: format!("session-{sequence}"),
                });
                json!({"protocolVersion":PROTOCOL,"capabilities":{"experimental":{"ai.cua.driver.envelopes":{"version":if self.unsupported.load(Ordering::SeqCst) {2} else {1}}}}})
            }
            "notifications/initialized" => {
                return Ok(DriverServiceResponse {
                    status: 202,
                    headers: vec![],
                    body: vec![],
                })
            }
            "cua/driver/v1/open" => {
                json!({"connection_id":session.as_ref().unwrap(),"generation":format!("gen-{}",session.as_ref().unwrap()),"public_session":"host-label","capabilities":{"minimum_envelope_version":1,"maximum_envelope_version":1,"supports_cancellation":!self.malformed_open.load(Ordering::SeqCst)}})
            }
            "cua/driver/v1/exchange" => {
                self.exchange_entered.notify_one();
                if self.hold_exchange.load(Ordering::SeqCst) {
                    self.exchange_release.notified().await;
                }
                if self.lost_exchange.load(Ordering::SeqCst) {
                    return Err(DriverServiceTransportError::Failed {
                        reason: "secret transport detail".into(),
                    });
                }
                self.events.lock().unwrap().push("drained".into());
                json!({"envelope_version":1,"request_id":body["params"]["envelope"]["request_id"],"ok":true,"completion_known":true,"result":null})
            }
            "cua/driver/v1/cancel" => {
                self.cancel_entered.notify_one();
                json!({"ok":true})
            }
            "cua/driver/v1/close" => json!({"ok":true}),
            "DELETE" => {
                return Ok(DriverServiceResponse {
                    status: 200,
                    headers: vec![],
                    body: vec![],
                })
            }
            _ => panic!("unexpected method {method}"),
        };
        if method != "initialize" {
            assert!(session.is_some());
            assert!(request
                .headers
                .iter()
                .any(|h| h.name == "MCP-Protocol-Version" && h.value == PROTOCOL));
        }
        let id = if method == "initialize" && self.malformed_init.load(Ordering::SeqCst) {
            json!("wrong")
        } else {
            body["id"].clone()
        };
        Ok(DriverServiceResponse {
            status: 200,
            headers,
            body: serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap(),
        })
    }
}

fn channel(transport: Arc<Transport>) -> Arc<McpDriverChannel> {
    open_mcp_driver_channel(transport, "host-principal".into()).unwrap()
}

fn request() -> DriverRequestEnvelope {
    DriverRequestEnvelope {
        envelope_version: 1,
        request_id: "request-1".into(),
        operation: "call".into(),
        name: Some("test".into()),
        arguments: None,
        deadline_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            + 10000,
    }
}

#[tokio::test]
async fn canonical_driver_identity_no_reconnect_and_close_isolation() {
    let transport = Arc::new(Transport::default());
    let first = channel(transport.clone());
    let second = channel(transport.clone());
    first.open().await.unwrap();
    second.open().await.unwrap();
    assert!(Arc::ptr_eq(
        &first.driver().unwrap(),
        &first.driver().unwrap()
    ));
    assert_eq!(first.public_session().unwrap(), "host-label");
    assert_eq!(first.inner.authenticated_principal(), "host-principal");
    assert_ne!(
        first.inner.connection_generation(),
        second.inner.connection_generation()
    );
    assert!(first.open().await.is_err());
    first.close().await.unwrap();
    assert!(first.driver().is_err());
    assert_eq!(
        second.inner.exchange(request()).await.unwrap().result,
        Some(Value::Null)
    );
    second.close().await.unwrap();
    assert_eq!(
        transport
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v.as_str() == "DELETE")
            .count(),
        2
    );
}

#[tokio::test]
async fn malformed_negotiation_cleans_allocated_resources() {
    for case in 0..3 {
        let transport = Arc::new(Transport::default());
        transport.unsupported.store(case == 0, Ordering::SeqCst);
        transport.malformed_init.store(case == 1, Ordering::SeqCst);
        transport.malformed_open.store(case == 2, Ordering::SeqCst);
        let connection = channel(transport.clone());
        assert!(connection.open().await.is_err());
        let events = transport.events.lock().unwrap();
        assert_eq!(events.last().unwrap(), "DELETE");
        assert_eq!(events.contains(&"cua/driver/v1/close".into()), case == 2);
    }
}

#[tokio::test]
async fn close_during_initialize_reaps_late_session_without_receiver_open() {
    let transport = Arc::new(Transport::default());
    transport.hold_init.store(true, Ordering::SeqCst);
    let connection = channel(transport.clone());
    let opening = tokio::spawn({
        let connection = connection.clone();
        async move { connection.open().await }
    });
    transport.init_entered.notified().await;
    let closing = tokio::spawn({
        let connection = connection.clone();
        async move { connection.close().await }
    });
    while !connection.inner.state.lock().unwrap().closed {
        tokio::task::yield_now().await;
    }
    transport.init_release.notify_one();
    assert!(opening
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("closed during"));
    closing.await.unwrap().unwrap();
    assert_eq!(
        *transport.events.lock().unwrap(),
        vec!["initialize", "DELETE"]
    );
}

#[tokio::test]
async fn initialization_finishing_after_bounded_close_still_reaps_session() {
    let transport = Arc::new(Transport::default());
    transport.hold_init.store(true, Ordering::SeqCst);
    let connection = channel(transport.clone());
    let opening = tokio::spawn({
        let connection = connection.clone();
        async move { connection.open().await }
    });
    transport.init_entered.notified().await;
    let error = tokio::time::timeout(Duration::from_secs(3), connection.close())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("initialization drain timed out"));
    assert!(!opening.is_finished());
    transport.init_release.notify_one();
    assert!(opening.await.unwrap().is_err());
    connection.close().await.unwrap();
    assert_eq!(
        *transport.events.lock().unwrap(),
        vec!["initialize", "DELETE"]
    );
}

#[tokio::test]
async fn cancellation_drains_before_receiver_close_and_rejects_late_result() {
    let transport = Arc::new(Transport::default());
    transport.hold_exchange.store(true, Ordering::SeqCst);
    let connection = channel(transport.clone());
    connection.open().await.unwrap();
    let exchange = tokio::spawn({
        let inner = connection.inner.clone();
        async move { inner.exchange(request()).await }
    });
    transport.exchange_entered.notified().await;
    let closing = tokio::spawn({
        let connection = connection.clone();
        async move { connection.close().await }
    });
    transport.cancel_entered.notified().await;
    assert!(!closing.is_finished());
    assert!(!transport
        .events
        .lock()
        .unwrap()
        .contains(&"cua/driver/v1/close".into()));
    transport.exchange_release.notify_one();
    closing.await.unwrap().unwrap();
    assert!(exchange
        .await
        .unwrap()
        .unwrap_err()
        .contains("after close or cancellation"));
    let events = transport.events.lock().unwrap();
    assert!(
        events.iter().position(|v| v == "drained")
            < events.iter().position(|v| v == "cua/driver/v1/close")
    );
    assert_eq!(events.last().unwrap(), "DELETE");
}

#[tokio::test]
async fn dropped_exchange_future_keeps_wire_response_alive_until_drain() {
    let transport = Arc::new(Transport::default());
    transport.hold_exchange.store(true, Ordering::SeqCst);
    let connection = channel(transport.clone());
    connection.open().await.unwrap();
    let exchange = tokio::spawn({
        let inner = connection.inner.clone();
        async move { inner.exchange(request()).await }
    });
    transport.exchange_entered.notified().await;
    exchange.abort();
    let _ = exchange.await;
    transport.cancel_entered.notified().await;
    transport.exchange_release.notify_one();
    connection.close().await.unwrap();
    assert!(transport.events.lock().unwrap().contains(&"drained".into()));
}

#[tokio::test]
async fn lost_exchange_is_sanitized_and_never_replayed() {
    let transport = Arc::new(Transport::default());
    let connection = channel(transport.clone());
    connection.open().await.unwrap();
    transport.lost_exchange.store(true, Ordering::SeqCst);
    let error = connection.inner.exchange(request()).await.unwrap_err();
    assert!(error.contains("completion is unknown"));
    assert!(!error.contains("secret"));
    assert!(connection.inner.exchange(request()).await.is_err());
    assert!(connection.close().await.is_err());
    let events = transport.events.lock().unwrap();
    assert_eq!(
        events.iter().filter(|v| v.as_str() == "initialize").count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|v| v.as_str() == "cua/driver/v1/exchange")
            .count(),
        1
    );
    assert_eq!(events.last().unwrap(), "DELETE");
}

#[tokio::test]
async fn drain_timeout_reports_unconfirmed_without_aborting_or_destroying_session() {
    let transport = Arc::new(Transport::default());
    transport.hold_exchange.store(true, Ordering::SeqCst);
    let connection = channel(transport.clone());
    connection.open().await.unwrap();
    let exchange = tokio::spawn({
        let inner = connection.inner.clone();
        async move { inner.exchange(request()).await }
    });
    transport.exchange_entered.notified().await;
    let error = tokio::time::timeout(Duration::from_secs(3), connection.close())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("drain timed out"));
    assert!(!exchange.is_finished());
    assert_eq!(
        transport.events.lock().unwrap().last().unwrap(),
        "cua/driver/v1/cancel"
    );
    transport.exchange_release.notify_one();
    assert!(exchange.await.unwrap().unwrap_err().contains("after close"));
    assert!(!transport.events.lock().unwrap().contains(&"DELETE".into()));
}

struct StaticTransport(DriverServiceResponse);
#[async_trait]
impl DriverServiceTransport for StaticTransport {
    async fn send(
        &self,
        _: DriverServiceRequest,
    ) -> Result<DriverServiceResponse, DriverServiceTransportError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn session_headers_are_strict_and_never_rebound() {
    for sessions in [vec![], vec!["one", "two"], vec!["bad session"], vec![""]] {
        let mut reply = response("{}", "application/json");
        reply
            .headers
            .extend(sessions.into_iter().map(|value| DriverServiceHeader {
                name: "mCp-SeSsIoN-iD".into(),
                value: value.into(),
            }));
        let connection =
            open_mcp_driver_channel(Arc::new(StaticTransport(reply)), "host".into()).unwrap();
        assert!(connection
            .inner
            .http("POST", None, 1, true)
            .await
            .unwrap_err()
            .contains("session negotiation"));
        assert!(connection.inner.state.lock().unwrap().session.is_none());
    }
    let mut reply = response("{}", "application/json");
    reply.headers.push(DriverServiceHeader {
        name: "Mcp-Session-Id".into(),
        value: "new-session".into(),
    });
    let connection =
        open_mcp_driver_channel(Arc::new(StaticTransport(reply)), "host".into()).unwrap();
    connection.inner.state.lock().unwrap().session = Some("original".into());
    assert!(connection
        .inner
        .http("POST", None, 1, false)
        .await
        .unwrap_err()
        .contains("changed unexpectedly"));
    assert_eq!(
        connection.inner.state.lock().unwrap().session.as_deref(),
        Some("original")
    );
}

#[test]
fn envelope_validation_preserves_null_and_sanitizes_guest_errors() {
    let request = request();
    let valid = json!({"envelope_version":1,"request_id":request.request_id,"ok":false,"completion_known":false,"error":"private diagnostic","error_code":"Failed","result":null});
    let result = decode_envelope(valid.clone(), &request).unwrap();
    assert_eq!(result.result, Some(Value::Null));
    assert_eq!(result.error.as_deref(), Some("Driver operation failed"));
    for (field, value) in [
        ("request_id", json!("wrong")),
        ("envelope_version", json!(1.0)),
        ("completion_known", json!(0)),
        ("ok", json!(1)),
        ("error_code", json!("invalid space")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(decode_envelope(invalid, &request).is_err());
    }
}

#[tokio::test]
async fn deadline_expiry_cancels_and_drains_without_replay() {
    let transport = Arc::new(Transport::default());
    transport.hold_exchange.store(true, Ordering::SeqCst);
    let connection = channel(transport.clone());
    connection.open().await.unwrap();
    let mut envelope = request();
    envelope.deadline_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 50;
    let exchange = tokio::spawn({
        let inner = connection.inner.clone();
        async move { inner.exchange(envelope).await }
    });
    transport.cancel_entered.notified().await;
    assert!(!exchange.is_finished());
    transport.exchange_release.notify_one();
    assert!(exchange
        .await
        .unwrap()
        .unwrap_err()
        .contains("deadline has expired; completion is unknown"));
    assert_eq!(transport.events.lock().unwrap().last().unwrap(), "DELETE");
}

#[tokio::test]
async fn invalid_deadline_and_oversized_request_never_reach_transport() {
    let transport = Arc::new(Transport::default());
    let connection = channel(transport.clone());
    connection.open().await.unwrap();
    let count = transport.events.lock().unwrap().len();
    let mut expired = request();
    expired.deadline_unix_ms = 0;
    assert!(connection
        .inner
        .exchange(expired)
        .await
        .unwrap_err()
        .contains("expired"));
    let mut oversized = request();
    oversized.arguments = Some(json!("x".repeat(REQUEST_LIMIT)));
    assert!(connection
        .inner
        .exchange(oversized)
        .await
        .unwrap_err()
        .contains("size limit"));
    assert_eq!(transport.events.lock().unwrap().len(), count);
    connection.close().await.unwrap();
}
