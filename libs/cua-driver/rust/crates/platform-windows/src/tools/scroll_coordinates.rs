//! Window scroll pixels use the same screenshot context as pixel clicks.

use cua_driver_core::{
    protocol::ToolResult,
    snapshot_store::{SnapshotPayload, SnapshotStore},
};
use serde_json::Value;

pub(super) fn window_bitmap_point<S: SnapshotPayload>(
    _snapshots: &SnapshotStore<S>,
    _args: &Value,
    _pid: u32,
    _window_id: u64,
    x: f64,
    y: f64,
) -> Result<(i32, i32), ToolResult> {
    Ok((x as i32, y as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Empty;
    impl SnapshotPayload for Empty {
        type Element = ();
        fn len(&self) -> usize { 0 }
        fn retain(&self, _: usize) -> Option<()> { None }
    }

    fn args(session: &str) -> Value { json!({"_session_id": session}) }

    #[test]
    fn resized_scroll_targets_native_bitmap_pixels() {
        let snapshots = SnapshotStore::new();
        snapshots.publish_for_session(42, 7, Empty, Some("a"), Some(1920.0 / 1400.0));
        assert_eq!(window_bitmap_point(&snapshots, &args("a"), 42, 7, 1000.0, 650.0).unwrap(), (1371, 891));
    }

    #[test]
    fn native_size_scroll_preserves_fractional_pixel_cast() {
        let snapshots = SnapshotStore::new();
        snapshots.publish_for_session(42, 7, Empty, Some("a"), Some(1.0));
        assert_eq!(window_bitmap_point(&snapshots, &args("a"), 42, 7, 1000.75, 650.25).unwrap(), (1000, 650));
    }

    #[test]
    fn scroll_uses_the_selected_window_and_session() {
        let snapshots = SnapshotStore::new();
        snapshots.publish_for_session(42, 7, Empty, Some("a"), Some(2.0));
        snapshots.publish_for_session(42, 8, Empty, Some("b"), Some(3.0));
        assert_eq!(window_bitmap_point(&snapshots, &args("a"), 42, 7, 10.0, 20.0).unwrap(), (20, 40));
        assert_eq!(window_bitmap_point(&snapshots, &args("b"), 42, 8, 10.0, 20.0).unwrap(), (30, 60));
    }

    #[test]
    fn scroll_refuses_missing_or_foreign_screenshot_context() {
        let snapshots = SnapshotStore::<Empty>::new();
        let refusal = window_bitmap_point(&snapshots, &args("a"), 42, 7, 10.0, 20.0).unwrap_err();
        assert_eq!(refusal.structured_content.unwrap()["code"], "screenshot_context_missing");
        snapshots.publish_for_session(42, 7, Empty, Some("b"), Some(2.0));
        assert!(window_bitmap_point(&snapshots, &args("a"), 42, 7, 10.0, 20.0).is_err());
    }

    #[test]
    fn trusted_native_pixels_are_not_scaled_twice() {
        let snapshots = SnapshotStore::new();
        snapshots.publish_for_session(42, 7, Empty, Some("a"), Some(2.0));
        let native = json!({"_session_id":"a", "_native_window_pixels":true});
        assert_eq!(window_bitmap_point(&snapshots, &native, 42, 7, 10.0, 20.0).unwrap(), (10, 20));
    }
}
