use super::{GetWindowStateTool, ToolState};
use cua_driver_core::tool::Tool;

#[test]
fn schema_exposes_zero_as_native_resolution() {
    let tool = GetWindowStateTool {
        state: ToolState::new(),
    };
    assert_eq!(
        tool.def().input_schema["properties"]["max_image_dimension"]["minimum"],
        0
    );
}
