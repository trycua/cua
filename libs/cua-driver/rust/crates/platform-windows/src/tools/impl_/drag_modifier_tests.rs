use super::DragTool;
use cua_driver_core::tool::Tool;
use serde_json::json;

// A background drag posts mouse messages or injects pen input, neither of
// which carries live modifier-key state. The refusal comes before any window
// lookup or input, so an unknown pid never reaches the OS.
#[tokio::test]
async fn background_drag_with_modifiers_is_refused_before_input() {
    let tool = DragTool {
        state: super::ToolState::new(None),
    };
    for delivery in [json!(null), json!("background")] {
        let mut args = json!({
            "pid": 0x7fff_fff0_i64,
            "from_x": 10, "from_y": 10, "to_x": 50, "to_y": 10,
            "modifier": ["ctrl"]
        });
        if !delivery.is_null() {
            args["delivery_mode"] = delivery;
        }
        let result = tool.invoke(args).await;
        assert_eq!(
            result.is_error,
            Some(true),
            "modified background drag was not refused"
        );
        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "background_unavailable");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["escalation"]["recommended"], "foreground");
    }
}
