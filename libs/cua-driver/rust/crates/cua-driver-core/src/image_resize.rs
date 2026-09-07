use std::collections::HashMap;
use std::sync::Mutex;

type WindowRatios = HashMap<(u32, Option<u64>), f64>;

#[derive(Default)]
pub struct ResizeRegistry {
    owners: Mutex<HashMap<Option<String>, WindowRatios>>,
}

impl ResizeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_ratio(&self, session: Option<&str>, pid: u32, window_id: Option<u64>, ratio: f64) {
        let mut owners = self.owners.lock().unwrap();
        if session.is_some_and(crate::session::is_session_ended) {
            return;
        }
        owners
            .entry(session.map(str::to_owned))
            .or_default()
            .insert((pid, window_id), ratio);
    }

    pub fn clear_ratio(&self, session: Option<&str>, pid: u32, window_id: Option<u64>) {
        let mut owners = self.owners.lock().unwrap();
        let owner = session.map(str::to_owned);
        if let Some(ratios) = owners.get_mut(&owner) {
            ratios.remove(&(pid, window_id));
            if ratios.is_empty() {
                owners.remove(&owner);
            }
        }
    }

    pub fn ratio(&self, session: Option<&str>, pid: u32, window_id: Option<u64>) -> Option<f64> {
        let owners = self.owners.lock().unwrap();
        let ratios = owners.get(&session.map(str::to_owned))?;
        if let Some(window_id) = window_id {
            return ratios.get(&(pid, Some(window_id))).copied();
        }
        let mut agreed = None;
        for (_, &ratio) in ratios
            .iter()
            .filter(|((owner_pid, _), _)| *owner_pid == pid)
        {
            match agreed {
                None => agreed = Some(ratio),
                Some(previous) if (previous - ratio).abs() < 1e-9 => {}
                Some(_) => return None,
            }
        }
        agreed
    }

    pub fn pixel_refusal(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Option<crate::protocol::ToolResult> {
        if !matches!(
            tool,
            "click"
                | "double_click"
                | "right_click"
                | "drag"
                | "scroll"
                | "type_text"
                | "type_text_chars"
                | "press_key"
                | "hotkey"
                | "mouse_button_down"
                | "mouse_drag"
                | "mouse_button_up"
        ) || args.get("scope").and_then(serde_json::Value::as_str) == Some("desktop")
            || args.get("from_zoom").and_then(serde_json::Value::as_bool) == Some(true)
            || args
                .get("element_token")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|token| !token.is_empty())
            || args
                .get("element_index")
                .and_then(serde_json::Value::as_u64)
                .is_some()
            || !["x", "y", "from_x", "from_y", "to_x", "to_y"]
                .iter()
                .any(|key| args.get(*key).is_some())
        {
            return None;
        }
        let session = args
            .get("_session_id")
            .and_then(serde_json::Value::as_str)?;
        let pid = u32::try_from(args.get("pid")?.as_u64()?).ok()?;
        let window_id = args.get("window_id").and_then(serde_json::Value::as_u64);
        if self.ratio(Some(session), pid, window_id).is_some() {
            return None;
        }
        Some(crate::protocol::ToolResult::error(
            "No screenshot coordinate frame is available for this session and window. Call get_window_state with a screenshot on the same connection before using pixels. For multi-call CLI work, repeat the same explicit session label on capture and action."
        ).with_structured(serde_json::json!({
            "code": "screenshot_context_missing",
            "pid": pid,
            "window_id": window_id
        })))
    }

    pub fn clear_session(&self, session: &str) {
        self.owners
            .lock()
            .unwrap()
            .remove(&Some(session.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capture_by_another_client_does_not_clear_the_resize_ratio() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-client-a"), 10, Some(20), 7.35);
        registry.clear_ratio(Some("resize-client-b"), 10, Some(20));
        assert_eq!(
            registry.ratio(Some("resize-client-a"), 10, Some(20)),
            Some(7.35)
        );
        assert_eq!(registry.ratio(Some("resize-client-b"), 10, Some(20)), None);
    }

    #[test]
    fn differently_resized_captures_are_isolated_by_client_and_window() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-a"), 10, Some(20), 7.35);
        registry.set_ratio(Some("resize-b"), 10, Some(20), 2.0);
        registry.set_ratio(Some("resize-a"), 10, Some(21), 3.0);
        assert_eq!(registry.ratio(Some("resize-a"), 10, Some(20)), Some(7.35));
        assert_eq!(registry.ratio(Some("resize-b"), 10, Some(20)), Some(2.0));
        assert_eq!(registry.ratio(Some("resize-a"), 10, Some(21)), Some(3.0));
    }

    #[test]
    fn same_client_native_capture_clears_only_its_exact_window() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-owner"), 10, Some(20), 7.35);
        registry.set_ratio(Some("resize-owner"), 10, Some(21), 2.0);
        registry.clear_ratio(Some("resize-owner"), 10, Some(20));
        assert_eq!(registry.ratio(Some("resize-owner"), 10, Some(20)), None);
        assert_eq!(
            registry.ratio(Some("resize-owner"), 10, Some(21)),
            Some(2.0)
        );
    }

    #[test]
    fn anonymous_calls_do_not_borrow_a_named_clients_ratio() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-named"), 10, Some(20), 7.35);
        registry.set_ratio(None, 10, Some(20), 2.0);
        assert_eq!(registry.ratio(None, 10, Some(20)), Some(2.0));
        assert_eq!(
            registry.ratio(Some("resize-named"), 10, Some(20)),
            Some(7.35)
        );
        assert_eq!(registry.ratio(Some("resize-new"), 10, Some(20)), None);
    }

    #[test]
    fn windowless_lookup_never_crosses_owner_or_ambiguous_windows() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-one"), 10, Some(20), 2.0);
        registry.set_ratio(Some("resize-one"), 10, Some(21), 2.0);
        registry.set_ratio(Some("resize-two"), 10, Some(20), 7.35);
        assert_eq!(registry.ratio(Some("resize-one"), 10, None), Some(2.0));
        registry.set_ratio(Some("resize-one"), 10, Some(21), 3.0);
        assert_eq!(registry.ratio(Some("resize-one"), 10, None), None);
    }

    #[test]
    fn new_session_refuses_pixels_instead_of_borrowing_or_guessing_a_transform() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-owner-context"), 10, Some(20), 7.35);
        let args = serde_json::json!({"_session_id": "resize-new-context", "pid": 10, "window_id": 20, "x": 99.5, "y": 62.5});
        let refusal = registry
            .pixel_refusal("click", &args)
            .expect("missing owner frame must refuse");
        assert_eq!(
            refusal.structured_content.unwrap()["code"],
            "screenshot_context_missing"
        );
        let mut null_target = args.clone();
        null_target["element_token"] = serde_json::Value::Null;
        null_target["element_index"] = serde_json::Value::Null;
        assert!(registry.pixel_refusal("click", &null_target).is_some());
        registry.set_ratio(Some("resize-new-context"), 10, Some(20), 1.0);
        assert!(registry.pixel_refusal("click", &args).is_none());
    }

    #[test]
    fn desktop_zoom_and_semantic_actions_do_not_require_a_window_image_transform() {
        let registry = ResizeRegistry::new();
        for extra in [
            serde_json::json!({"scope":"desktop"}),
            serde_json::json!({"from_zoom":true}),
            serde_json::json!({"element_token":"s00000001:0"}),
        ] {
            let mut args = serde_json::json!({"_session_id":"resize-exempt", "pid":10, "window_id":20, "x":1, "y":2});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(registry.pixel_refusal("click", &args).is_none());
        }
        assert!(registry
            .pixel_refusal(
                "get_window_state",
                &serde_json::json!({"_session_id":"resize-exempt", "pid":10, "window_id":20})
            )
            .is_none());
    }

    #[test]
    fn session_end_hook_removes_state_and_rejects_late_capture_publication() {
        let registry = std::sync::Arc::new(ResizeRegistry::new());
        let ending = format!("resize-end-{}", uuid::Uuid::new_v4());
        let cleanup = registry.clone();
        let _hook = crate::session::register_scoped_session_end_hook(move |session| {
            cleanup.clear_session(session);
        });
        registry.set_ratio(Some(&ending), 10, Some(20), 7.35);
        assert!(crate::session::fire_session_end(&ending));
        assert_eq!(registry.ratio(Some(&ending), 10, Some(20)), None);
        registry.set_ratio(Some(&ending), 10, Some(20), 2.0);
        assert_eq!(registry.ratio(Some(&ending), 10, Some(20)), None);
        assert!(registry.owners.lock().unwrap().is_empty());
    }

    #[test]
    fn concurrent_capture_does_not_change_another_clients_pixel_transform() {
        let registry = std::sync::Arc::new(ResizeRegistry::new());
        registry.set_ratio(Some("resize-concurrent-a"), 10, Some(20), 7.35);
        let writer = registry.clone();
        let thread = std::thread::spawn(move || {
            for _ in 0..1000 {
                writer.set_ratio(Some("resize-concurrent-b"), 10, Some(20), 2.0);
                writer.clear_ratio(Some("resize-concurrent-b"), 10, Some(20));
            }
        });
        for _ in 0..1000 {
            let ratio = registry
                .ratio(Some("resize-concurrent-a"), 10, Some(20))
                .unwrap();
            assert!((99.5 * ratio - 731.325).abs() < 1e-9);
            assert!((62.5 * ratio - 459.375).abs() < 1e-9);
        }
        thread.join().unwrap();
    }

    #[test]
    fn disconnect_clears_only_the_ending_client() {
        let registry = ResizeRegistry::new();
        registry.set_ratio(Some("resize-ending"), 10, Some(20), 7.35);
        registry.set_ratio(Some("resize-survivor"), 10, Some(20), 2.0);
        registry.clear_session("resize-ending");
        assert_eq!(registry.ratio(Some("resize-ending"), 10, Some(20)), None);
        assert_eq!(
            registry.ratio(Some("resize-survivor"), 10, Some(20)),
            Some(2.0)
        );
        assert_eq!(registry.owners.lock().unwrap().len(), 1);
    }
}
