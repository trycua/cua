use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct ResizeRegistry {
    owners: Mutex<HashMap<Option<String>, HashMap<(u32, Option<u64>), f64>>>,
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
