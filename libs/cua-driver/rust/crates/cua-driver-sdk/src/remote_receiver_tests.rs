use super::*;
use serde_json::json;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use tokio::sync::Notify;

struct FakeExecutor {
    calls: Mutex<Vec<(String, Value)>>,
    result: Value,
    error: Mutex<Option<DriverError>>,
    blocked: bool,
    release: Notify,
    closes: AtomicUsize,
}

impl FakeExecutor {
    fn new(result: Value, blocked: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            result,
            error: Mutex::new(None),
            blocked,
            release: Notify::new(),
            closes: AtomicUsize::new(0),
        })
    }

    async fn execute(&self, operation: String, arguments: Value) -> Result<Value, DriverError> {
        self.calls.lock().unwrap().push((operation, arguments));
        if self.blocked {
            self.release.notified().await;
        }
        match self.error.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(self.result.clone()),
        }
    }

    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait]
impl DriverEnvelopeExecutor for FakeExecutor {
    async fn metadata(&self) -> Result<Value, DriverError> {
        self.execute("metadata".into(), Value::Null).await
    }

    async fn list_tools(&self) -> Result<Value, DriverError> {
        self.execute("list".into(), Value::Null).await
    }

    async fn call(&self, name: String, arguments: Value) -> Result<Value, DriverError> {
        self.execute(name, arguments).await
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

fn request(id: &str) -> DriverRequestEnvelope {
    DriverRequestEnvelope {
        envelope_version: DRIVER_ENVELOPE_VERSION,
        request_id: id.into(),
        operation: "call".into(),
        name: Some("click".into()),
        arguments: Some(json!({"x": 10, "y": 20})),
        deadline_unix_ms: now_ms() + 30_000,
    }
}

fn assert_refusal(response: &DriverResponseEnvelope, code: &str, known: bool) {
    assert!(!response.ok, "{response:?}");
    assert_eq!(response.envelope_version, DRIVER_ENVELOPE_VERSION);
    assert_eq!(response.error_code.as_deref(), Some(code));
    assert_eq!(response.completion_known, known);
    assert!(response.result.is_none());
    assert!(response
        .error
        .as_ref()
        .is_some_and(|error| !error.is_empty()));
}

// Polling once proves dispatch or queue registration without scheduler sleeps.
async fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn dropping_an_in_flight_exchange_closes_only_its_session() {
    let executor = FakeExecutor::new(Value::Null, true);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let mut exchange = Box::pin(receiver.exchange(receiver.generation(), request("dropped")));
    assert_pending(exchange.as_mut()).await;
    assert_eq!(executor.count(), 1);
    drop(exchange);
    assert_eq!(executor.closes.load(Ordering::SeqCst), 1);
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("after-drop"))
            .await,
        "connection_closed",
        true,
    );
    assert_eq!(executor.count(), 1);
}

#[tokio::test]
async fn preserves_native_results_and_arguments_for_each_operation() {
    let native = json!({
        "content": [{"type": "text", "text": "native result"}],
        "structuredContent": {"snapshot": "s1", "arbitrary": [null, 3, true]},
        "action": {"completion": "completed", "verification": {"matched": true}},
        "isError": true,
        "error_code": "native_tool_error"
    });
    let executor = FakeExecutor::new(native.clone(), false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    assert_eq!(
        receiver.capabilities(),
        DriverChannelCapabilities {
            minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
            supports_cancellation: true,
        }
    );
    for operation in ["metadata", "list", "call"] {
        let mut envelope = request(operation);
        envelope.operation = operation.into();
        if operation != "call" {
            envelope.name = None;
            envelope.arguments = None;
        }
        let response = receiver.exchange(receiver.generation(), envelope).await;
        assert!(response.ok);
        assert!(response.completion_known);
        assert_eq!(response.request_id, operation);
        assert_eq!(response.result.as_ref(), Some(&native));
        assert!(response.error.is_none());
        assert!(response.error_code.is_none());
    }
    assert_eq!(
        *executor.calls.lock().unwrap(),
        vec![
            ("metadata".into(), Value::Null),
            ("list".into(), Value::Null),
            ("click".into(), json!({"x": 10, "y": 20})),
        ]
    );
}

#[tokio::test]
async fn preserves_native_tool_error_code_and_message() {
    let executor = FakeExecutor::new(Value::Null, false);
    let error = DriverError::Tool {
        tool: "click".into(),
        message: "target is stale".into(),
        error_code: "stale_target".into(),
    };
    let expected = error.to_string();
    *executor.error.lock().unwrap() = Some(error);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let response = receiver
        .exchange(receiver.generation(), request("error"))
        .await;
    assert_refusal(&response, "stale_target", true);
    assert_eq!(response.error.as_deref(), Some(expected.as_str()));
    assert_eq!(executor.count(), 1);
    assert_eq!(executor.closes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_envelopes_never_reach_executor() {
    let executor = FakeExecutor::new(Value::Null, false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let mut invalid = Vec::new();
    let mut envelope = request("version");
    envelope.envelope_version = DRIVER_ENVELOPE_VERSION + 1;
    invalid.push(envelope);
    let mut envelope = request("expired");
    envelope.deadline_unix_ms = 0;
    invalid.push(envelope);
    let mut envelope = request("distant");
    envelope.deadline_unix_ms = now_ms() + MAX_DEADLINE_MS + 60_000;
    invalid.push(envelope);
    let mut envelope = request("oversized");
    envelope.arguments = Some(json!({"text": "x".repeat(MAX_REQUEST_BYTES)}));
    invalid.push(envelope);
    for id in ["", "bad/id"] {
        invalid.push(request(id));
    }
    for arguments in [
        json!({"_session_id": "untrusted"}),
        json!({"_permission_mode": "unrestricted"}),
        json!({"screenshot_out_file": "/tmp/receiver-test.png"}),
        json!({"image_path": "/tmp/receiver-test.png"}),
        json!({"file_path": "/tmp/receiver-test.txt"}),
        json!(["not", "an", "object"]),
    ] {
        let mut envelope = request("arguments");
        envelope.arguments = Some(arguments);
        invalid.push(envelope);
    }
    for name in ["execute_shell", "session_end", "unknown_tool"] {
        let mut envelope = request("unsupported");
        envelope.name = Some(name.into());
        invalid.push(envelope);
    }
    let mut envelope = request("operation");
    envelope.operation = "shutdown".into();
    invalid.push(envelope);
    for envelope in invalid {
        let id = envelope.request_id.clone();
        let response = receiver.exchange(receiver.generation(), envelope).await;
        assert_refusal(&response, "invalid_request", true);
        assert_eq!(response.request_id, id);
    }
    assert_eq!(executor.count(), 0);
    assert!(receiver.state.lock().unwrap().requests.is_empty());
}

#[tokio::test]
async fn stale_generation_cannot_exchange_or_cancel_current_request() {
    let executor = FakeExecutor::new(json!({"ok": true}), false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    assert_refusal(
        &receiver.exchange("stale", request("one")).await,
        "stale_connection",
        true,
    );
    assert!(receiver.cancel("stale", "one").is_err());
    assert_eq!(executor.count(), 0);
    assert!(
        receiver
            .exchange(receiver.generation(), request("one"))
            .await
            .ok
    );
    assert_eq!(executor.count(), 1);
}

#[tokio::test]
async fn duplicate_completed_request_is_not_replayed_and_completion_is_unknown() {
    let executor = FakeExecutor::new(json!({"acted": true}), false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    assert!(
        receiver
            .exchange(receiver.generation(), request("one"))
            .await
            .ok
    );
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("one"))
            .await,
        "duplicate_request",
        false,
    );
    assert_eq!(executor.count(), 1);
}

#[tokio::test]
async fn close_after_completion_is_idempotent_and_isolated() {
    let executor = FakeExecutor::new(Value::Null, false);
    let other_executor = FakeExecutor::new(Value::Null, false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let other = DriverEnvelopeReceiver::new(other_executor.clone());
    assert_ne!(receiver.generation(), other.generation());
    assert!(
        receiver
            .exchange(receiver.generation(), request("one"))
            .await
            .ok
    );
    receiver.close();
    receiver.close();
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("two"))
            .await,
        "connection_closed",
        true,
    );
    drop(receiver);
    assert_eq!(executor.closes.load(Ordering::SeqCst), 1);
    assert_eq!(executor.count(), 1);
    assert_eq!(other_executor.closes.load(Ordering::SeqCst), 0);
    assert!(other.exchange(other.generation(), request("one")).await.ok);
}

#[tokio::test]
async fn early_cancel_prevents_delayed_exchange() {
    let executor = FakeExecutor::new(Value::Null, false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    receiver.cancel(receiver.generation(), "early").unwrap();
    receiver.cancel(receiver.generation(), "early").unwrap();
    let response = receiver
        .exchange(receiver.generation(), request("early"))
        .await;
    assert!(!response.ok);
    assert_eq!(executor.count(), 0);
}

#[tokio::test]
async fn in_flight_cancel_has_unknown_completion_and_closes_connection() {
    let executor = FakeExecutor::new(Value::Null, true);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let mut exchange = Box::pin(receiver.exchange(receiver.generation(), request("running")));
    assert_pending(exchange.as_mut()).await;
    assert_eq!(executor.count(), 1);
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("running"))
            .await,
        "duplicate_request",
        false,
    );
    receiver.cancel(receiver.generation(), "running").unwrap();
    assert_refusal(&exchange.await, "action_interrupted", false);
    assert_eq!(executor.closes.load(Ordering::SeqCst), 1);
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("later"))
            .await,
        "connection_closed",
        true,
    );
    assert_eq!(executor.count(), 1);
}

#[tokio::test]
async fn queued_cancel_proves_nonexecution_and_keeps_connection_open() {
    let executor = FakeExecutor::new(json!({"acted": true}), true);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let mut first = Box::pin(receiver.exchange(receiver.generation(), request("first")));
    assert_pending(first.as_mut()).await;
    let mut queued = Box::pin(receiver.exchange(receiver.generation(), request("queued")));
    assert_pending(queued.as_mut()).await;
    receiver.cancel(receiver.generation(), "queued").unwrap();
    assert_refusal(&queued.await, "request_cancelled", true);
    assert_eq!(executor.count(), 1);
    assert_eq!(executor.closes.load(Ordering::SeqCst), 0);
    executor.release.notify_one();
    assert!(first.await.ok);
    executor.release.notify_one();
    assert!(
        receiver
            .exchange(receiver.generation(), request("later"))
            .await
            .ok
    );
    assert_eq!(executor.count(), 2);
}

#[tokio::test]
async fn timeout_after_dispatch_has_unknown_completion() {
    let executor = FakeExecutor::new(Value::Null, true);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    let mut envelope = request("timeout");
    envelope.deadline_unix_ms = now_ms() + 500;
    let mut exchange = Box::pin(receiver.exchange(receiver.generation(), envelope));
    assert_pending(exchange.as_mut()).await;
    assert_eq!(executor.count(), 1);
    let response = tokio::time::timeout(Duration::from_secs(5), exchange)
        .await
        .unwrap();
    assert_refusal(&response, "action_interrupted", false);
    assert_eq!(executor.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ledger_capacity_is_bounded_without_evicting_request_identities() {
    let executor = FakeExecutor::new(Value::Null, false);
    let receiver = DriverEnvelopeReceiver::new(executor.clone());
    assert!(
        receiver
            .exchange(receiver.generation(), request("completed"))
            .await
            .ok
    );
    for index in 1..MAX_REQUESTS {
        receiver
            .cancel(receiver.generation(), &format!("cancelled-{index}"))
            .unwrap();
    }
    assert_eq!(receiver.state.lock().unwrap().requests.len(), MAX_REQUESTS);
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("overflow"))
            .await,
        "connection_full",
        true,
    );
    assert!(receiver.cancel(receiver.generation(), "overflow").is_err());
    receiver
        .cancel(receiver.generation(), "cancelled-1")
        .unwrap();
    assert_refusal(
        &receiver
            .exchange(receiver.generation(), request("completed"))
            .await,
        "duplicate_request",
        false,
    );
    assert_eq!(receiver.state.lock().unwrap().requests.len(), MAX_REQUESTS);
    assert_eq!(executor.count(), 1);
}
