
use super::ClickTool;
use cua_driver_core::tool::Tool;

/// The portable click contract pins `button` and the other accepted
/// fields. A live schema may broaden `capture_id`, which the subset gate
/// allows, so the non-empty bound is pinned here.
#[test]
fn schema_requires_non_empty_capture_id() {
    let tool = ClickTool {
        state: super::ToolState::new(None),
    };
    let d = tool.def();
    let props = d.input_schema.get("properties").expect("properties");
    let capture_id = props.get("capture_id").expect("capture_id field present");
    assert_eq!(capture_id["type"], "string");
    assert_eq!(capture_id["minLength"], 1);
}
