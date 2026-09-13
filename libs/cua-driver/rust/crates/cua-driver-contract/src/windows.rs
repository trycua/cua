// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Portable native application discovery and exact-window observations.

use crate::{ToolInput, ToolOutput};
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

fn string_schema(g: &mut SchemaGenerator) -> Schema {
    String::json_schema(g)
}
fn bool_schema(g: &mut SchemaGenerator) -> Schema {
    bool::json_schema(g)
}
fn pid_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"integer", "minimum":1, "maximum":4294967295_u64})
}
fn positive_integer_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"integer", "minimum":1})
}

fn nullable_pid_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":["integer","null"], "minimum":0, "maximum":4294967295_u64})
}

fn nullable_z_index_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":["integer","null"]})
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ListAppsInput {}

impl ToolInput for ListAppsInput {
    const TOOL_NAME: &'static str = "list_apps";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ListWindowsInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "pid_schema")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "bool_schema")]
    pub on_screen_only: Option<bool>,
}

impl ToolInput for ListWindowsInput {
    const TOOL_NAME: &'static str = "list_windows";

    fn validate(&self) -> Result<(), String> {
        if self.pid == Some(0) {
            return Err("window discovery requires a positive process ID".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct GetWindowStateInput {
    #[schemars(schema_with = "pid_schema")]
    pub pid: u32,
    #[schemars(schema_with = "positive_integer_schema")]
    pub window_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "bool_schema")]
    pub include_accessibility_tree: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "bool_schema")]
    pub include_screenshot: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub screenshot_out_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "positive_integer_schema")]
    pub max_elements: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "positive_integer_schema")]
    pub max_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "positive_integer_schema")]
    pub max_dimension: Option<u32>,
}

impl ToolInput for GetWindowStateInput {
    const TOOL_NAME: &'static str = "get_window_state";
    fn validate(&self) -> Result<(), String> {
        if self.pid == 0 || self.window_id == 0 {
            return Err("window observation requires positive process and window IDs".into());
        }
        if [self.max_elements, self.max_depth, self.max_dimension].contains(&Some(0)) {
            return Err("window observation limits must be positive".into());
        }
        if self.include_accessibility_tree == Some(false) && self.include_screenshot == Some(false)
        {
            return Err("window observation requires accessibility or screenshot capture".into());
        }
        Ok(())
    }
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct AppInfo {
    pub pid: u32,
    pub name: String,
    pub running: bool,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct ListAppsOutput {
    pub apps: Vec<AppInfo>,
}

impl ToolOutput for ListAppsOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct WindowInfo {
    pub window_id: u64,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required, schema_with = "nullable_pid_schema")]
    pub pid: Option<u32>,
    pub app_name: String,
    pub title: String,
    pub bounds: WindowBounds,
    pub is_on_screen: bool,
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required, schema_with = "nullable_z_index_schema")]
    /// Higher values are closer to the front. Null means stacking order is unavailable; callers must not infer an order from array position.
    pub z_index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimized: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_space_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_current_space: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_ids: Option<Vec<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct ListWindowsOutput {
    pub windows: Vec<WindowInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_space_id: Option<u64>,
}

impl ToolOutput for ListWindowsOutput {
    fn output_schema() -> serde_json::Value {
        let mut schema = crate::outputs::output_schema_with_additional_properties::<Self>(true);
        // Stacking semantics are part of the existing live discovery contract.
        schema["properties"]["windows"]["items"]["properties"]["z_index"]["description"] =
            serde_json::json!("Higher values are closer to the front. Null means the provider cannot observe stacking order; callers must not infer an order from array position or treat null as zero.");
        schema
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct ElementFrame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct WindowElement {
    pub element_index: u64,
    pub role: String,
    pub depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_web_content: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ElementFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct SnapshotImage {
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct WindowStateOutput {
    pub pid: u32,
    pub window_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<WindowElement>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_element_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_element_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filtered_element_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements_complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_frame_valid: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_bounds: Option<WindowBounds>,
    /// Image content belongs to the MCP envelope, never structuredContent.
    #[serde(skip)]
    #[schemars(skip)]
    pub images: Vec<SnapshotImage>,
}

impl ToolOutput for WindowStateOutput {
    fn validate(&self) -> Result<(), String> {
        match (self.screenshot_width, self.screenshot_height) {
            (None, None) => {}
            (Some(width), Some(height)) if width > 0 && height > 0 => {}
            _ => return Err("screenshot dimensions must be a positive width/height pair".into()),
        }
        if self
            .screenshot_scale
            .is_some_and(|scale| !scale.is_finite() || scale <= 0.0)
        {
            return Err("screenshot_scale must be finite and positive".into());
        }
        if let (Some(elements), Some(returned)) = (&self.elements, self.returned_element_count) {
            if elements.len() as u64 != returned {
                return Err("returned_element_count does not match elements".into());
            }
        }
        if let (Some(total), Some(returned)) =
            (self.total_element_count, self.returned_element_count)
        {
            if returned > total {
                return Err("returned_element_count exceeds total_element_count".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_app_success_shapes_allow_unavailable_metadata() {
        for metadata in [
            json!({"bundle_id":"org.example.App", "kind":"desktop", "launch_path":"/Applications/Example.app", "last_used":null}),
            json!({"bundle_id":null, "kind":"desktop", "launch_path":"C:\\Example.exe", "last_used":null}),
            json!({"bundle_id":null, "kind":null, "launch_path":null, "last_used":null}),
        ] {
            let mut app =
                json!({"pid":7,"name":"Example","running":true,"active":false,"windows":[]});
            app.as_object_mut()
                .unwrap()
                .extend(metadata.as_object().unwrap().clone());
            let output: ListAppsOutput = serde_json::from_value(json!({"apps":[app]})).unwrap();
            assert_eq!(output.apps[0].pid, 7);
        }
    }

    #[test]
    fn native_window_success_shapes_preserve_unknown_pid_and_order() {
        for metadata in [
            json!({"pid":7,"z_index":4,"layer":0,"space_ids":[1],"current_space_id":1,"on_current_space":true}),
            json!({"pid":7,"z_index":0,"layer":0,"minimized":false}),
            json!({"pid":null,"z_index":null,"x":-10,"y":0,"width":100,"height":80}),
        ] {
            let mut window = json!({"window_id":9007199254740993_u64,"app_name":"Example","title":"Document","bounds":{"x":-10,"y":0,"width":100,"height":80},"is_on_screen":true});
            window
                .as_object_mut()
                .unwrap()
                .extend(metadata.as_object().unwrap().clone());
            let output: ListWindowsOutput =
                serde_json::from_value(json!({"windows":[window]})).unwrap();
            assert_eq!(output.windows[0].window_id, 9007199254740993);
        }
        let missing = json!({"windows":[{"window_id":1,"pid":null,"app_name":"Example","title":"Document","bounds":{"x":0,"y":0,"width":100,"height":80},"is_on_screen":true}]});
        assert!(serde_json::from_value::<ListWindowsOutput>(missing).is_err());
    }

    #[test]
    fn window_state_supports_capture_only_and_degraded_native_shapes() {
        let capture: WindowStateOutput = serde_json::from_value(json!({
            "pid":7,"window_id":9,"screenshot_width":100,"screenshot_height":80,
            "screenshot_mime_type":"image/png","screenshot_file_path":"/tmp/example.png",
            "window_bounds":{"x":0,"y":0,"width":100,"height":80},"screenshot_scale":1.0
        }))
        .unwrap();
        assert!(capture.elements.is_none());
        assert!(capture.images.is_empty());
        for value in [
            json!({"pid":7,"window_id":9,"element_count":0,"elements":[],"tree_markdown":"","elements_complete":false,"degraded":true,"degraded_reason":"ax_window_unresolved"}),
            json!({"pid":7,"window_id":9,"element_count":1,"elements":[{"element_index":0,"role":"button","depth":1,"element_token":"s1:0","label":"Apply","frame":{"x":10,"y":20,"w":30,"h":40},"enabled":true}],"elements_complete":false,"screenshot_error":"capture unavailable"}),
            json!({"pid":7,"window_id":9,"elements":[{"element_index":0,"role":"button","depth":1}],"degraded":true,"degraded_reason":"accessibility_window_identity_unproven"}),
        ] {
            let output: WindowStateOutput = serde_json::from_value(value).unwrap();
            assert!(output.elements.is_some());
            output.validate().unwrap();
        }
    }

    #[test]
    fn images_are_envelope_only_and_not_structured_schema_fields() {
        let mut output: WindowStateOutput =
            serde_json::from_value(json!({"pid":7,"window_id":9})).unwrap();
        output.images.push(SnapshotImage {
            mime_type: "image/png".into(),
            data_base64: "aGVsbG8=".into(),
        });
        assert!(serde_json::to_value(&output)
            .unwrap()
            .get("images")
            .is_none());
        assert!(WindowStateOutput::output_schema()["properties"]
            .get("images")
            .is_none());
        assert_eq!(
            GetWindowStateInput::input_schema()["properties"]["max_elements"]["minimum"],
            1
        );
        assert_eq!(ListAppsInput::input_schema()["properties"], json!({}));
    }

    #[test]
    fn invalid_window_observation_metadata_fails_validation() {
        for extra in [
            json!({"screenshot_width":100}),
            json!({"screenshot_width":0,"screenshot_height":80}),
            json!({"screenshot_scale":0}),
            json!({"elements":[],"returned_element_count":1}),
            json!({"total_element_count":1,"returned_element_count":2}),
        ] {
            let mut value = json!({"pid":7,"window_id":9});
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let output: WindowStateOutput = serde_json::from_value(value).unwrap();
            assert!(output.validate().is_err());
        }
    }
}
