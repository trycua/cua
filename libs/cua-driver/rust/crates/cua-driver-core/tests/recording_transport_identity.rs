use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_driver_core::{
    protocol::Request,
    recording::RecordingSession,
    recording_tools::StartRecordingTool,
    server::{handle_request_with_transport_session, ToolProvider},
    tool::Tool,
};
use serde_json::{json, Value};

#[derive(Default)]
struct CaptureProvider {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl ToolProvider for CaptureProvider {
    fn tools_list(&self) -> Value {
        json!({"tools": []})
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_owned(), arguments));
        Ok(json!({
            "content": [{"type": "text", "text": "ok"}],
            "isError": false
        }))
    }
}

fn tool_call(id: u64, name: &str, arguments: Value) -> Request {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }))
    .unwrap()
}

async fn dispatch(provider: &CaptureProvider, id: u64, name: &str, arguments: Value) {
    let request = tool_call(id, name, arguments);
    let _ = handle_request_with_transport_session(request, json!(id), provider, "transport-1")
        .await;
}

#[test]
fn start_recording_contract_accepts_a_public_session_label() {
    let tool = StartRecordingTool::new(Arc::new(RecordingSession::new()));
    assert_eq!(
        tool.def().input_schema["properties"]["session"]["type"],
        "string"
    );
}

#[tokio::test]
async fn named_and_implicit_recording_calls_share_the_transport_resolved_identity() {
    let provider = CaptureProvider::default();

    dispatch(
        &provider,
        1,
        "start_recording",
        json!({"output_dir": "/tmp/named", "session": "demo"}),
    )
    .await;
    dispatch(
        &provider,
        2,
        "click",
        json!({"x": 1, "y": 2, "session": "demo"}),
    )
    .await;
    dispatch(
        &provider,
        3,
        "start_recording",
        json!({"output_dir": "/tmp/implicit"}),
    )
    .await;
    dispatch(&provider, 4, "click", json!({"x": 3, "y": 4})).await;

    let calls = provider.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);

    for index in [0usize, 1] {
        assert_eq!(calls[index].1["_session_id"], "demo");
        assert_eq!(calls[index].1["_transport_session_id"], "transport-1");
    }
    for index in [2usize, 3] {
        assert_eq!(calls[index].1["_session_id"], "transport-1");
        assert_eq!(calls[index].1["_transport_session_id"], "transport-1");
    }
}
