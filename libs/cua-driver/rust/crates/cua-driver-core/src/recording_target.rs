//! Window-video requests retain their capture target through authorization.

use crate::{protocol::ToolResult, video::WindowVideoTarget};
use serde_json::{json, Value};

fn invalid(message: &str) -> ToolResult {
    ToolResult::error(message).with_structured(json!({"code": "invalid_recording_target"}))
}

pub fn parse_window_target(args: &Value) -> Result<Option<WindowVideoTarget>, ToolResult> {
    if ["pid", "window_id", "scope"]
        .iter()
        .any(|key| args.get(key).is_some())
    {
        return Err(invalid(
            "recording accepts PID/window selection only inside target",
        ));
    }
    let Some(target) = args.get("target") else {
        return Ok(None);
    };
    if args.get("record_video").and_then(Value::as_bool) != Some(true) {
        return Err(invalid("window recording requires record_video: true"));
    }
    serde_json::from_value(target.clone())
        .map(Some)
        .map_err(|_| invalid("target must contain only kind: window, a positive 32-bit PID, and a positive window_id"))
}

pub(crate) fn attested_window_resource(
    target: &WindowVideoTarget,
    inventory: &Value,
) -> Result<Value, String> {
    let windows = inventory
        .get("windows")
        .and_then(Value::as_array)
        .ok_or_else(|| "window recording target inventory is unavailable".to_owned())?;
    let mut matching = windows
        .iter()
        .filter(|window| window.get("window_id").and_then(Value::as_u64) == Some(target.window_id));
    let window = matching
        .next()
        .ok_or_else(|| "window recording target no longer exists".to_owned())?;
    if matching.next().is_some()
        || window.get("pid").and_then(Value::as_i64) != Some(i64::from(target.pid))
    {
        return Err("window recording target ownership is unproven".to_owned());
    }
    // Do not copy titles, other windows, or account content into grant metadata.
    Ok(json!({"kind": "window", "pid": target.pid, "window_id": target.window_id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_target_requires_exact_window_and_video_opt_in() {
        let target = json!({"kind": "window", "pid": 42, "window_id": 7});
        assert_eq!(
            parse_window_target(&json!({"record_video": true, "target": target})).unwrap(),
            Some(WindowVideoTarget {
                pid: 42,
                window_id: 7
            })
        );
        for args in [
            json!({"target": target}),
            json!({"record_video": false, "target": target}),
            json!({"record_video": "true", "target": target}),
            json!({"record_video": true, "target": null}),
            json!({"record_video": true, "target": {"kind": "desktop", "display_id": "primary"}}),
            json!({"record_video": true, "target": {"kind": "window", "pid": 0, "window_id": 7}}),
            json!({"record_video": true, "target": {"kind": "window", "pid": 2147483648_u64, "window_id": 7}}),
            json!({"record_video": true, "target": {"kind": "window", "pid": 42, "window_id": 0}}),
            json!({"record_video": true, "target": {"kind": "window", "pid": 42, "window_id": 7, "extra": true}}),
            json!({"record_video": true, "target": target, "pid": 42}),
            json!({"pid": 42, "window_id": 7}),
            json!({"scope": "desktop"}),
        ] {
            assert!(parse_window_target(&args).is_err(), "accepted {args}");
        }
        assert_eq!(
            parse_window_target(&json!({"record_video": true})).unwrap(),
            None
        );
        assert_eq!(parse_window_target(&json!({})).unwrap(), None);
    }

    #[test]
    fn recording_resource_is_exact_and_excludes_inventory_content() {
        let target = WindowVideoTarget {
            pid: 42,
            window_id: 7,
        };
        let resource = attested_window_resource(
            &target,
            &json!({"windows": [
                {"pid": 42, "window_id": 7, "title": "private fixture title"},
                {"pid": 42, "window_id": 8, "title": "unrelated fixture"}
            ]}),
        )
        .unwrap();
        assert_eq!(
            resource,
            json!({"kind": "window", "pid": 42, "window_id": 7})
        );
        for inventory in [
            json!({}),
            json!({"windows": []}),
            json!({"windows": [{"pid": 43, "window_id": 7}]}),
            json!({"windows": [{"pid": 42, "window_id": 7}, {"pid": 42, "window_id": 7}]}),
        ] {
            assert!(attested_window_resource(&target, &inventory).is_err());
        }
    }
}
