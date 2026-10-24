use super::{GetWindowStateTool, ToolState};
use cua_driver_core::tool::Tool;

// T3 Code feature-detects the preview mode from this schema property.
#[test]
fn display_only_is_advertised() {
    let tool = GetWindowStateTool {
        state: ToolState::new(),
    };
    assert_eq!(
        tool.def().input_schema["properties"]["display_only"]["type"],
        "boolean"
    );
}

#[tokio::test]
async fn display_only_refuses_accessibility_snapshot_requests() {
    let tool = GetWindowStateTool {
        state: ToolState::new(),
    };
    for tree in [None, Some(true)] {
        let mut args = serde_json::json!({"pid": 42, "window_id": 7, "display_only": true});
        if let Some(tree) = tree {
            args["include_accessibility_tree"] = serde_json::json!(tree);
        }
        let result = tool.invoke(args).await;
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(cua_driver_core::protocol::ToolResult::error(
                "display_only requires include_accessibility_tree:false"
            ))
            .unwrap()
        );
    }
}
