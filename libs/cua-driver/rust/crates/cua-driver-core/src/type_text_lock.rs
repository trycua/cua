//! Process-wide, fail-fast serialization for generic desktop `type_text` calls.
//!
//! Text delivery can span focus changes and a character-by-character worker. A
//! second call for the same process must be refused before it can change focus;
//! queueing would let stale text land after the caller's target has changed.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::protocol::ToolResult;

fn active_pids() -> &'static Mutex<HashSet<i64>> {
    static ACTIVE: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Marks one pid as having an active generic desktop `type_text` operation.
///
/// This is intentionally process-wide rather than session-scoped: independent
/// agents can share the daemon and still target the same OS process. Acquisition
/// never waits, so a refused call can't become stale in a queue.
pub fn try_acquire(pid: i64) -> Result<TypeTextGuard, ToolResult> {
    let mut active = active_pids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !active.insert(pid) {
        return Err(ToolResult::error(format!(
            "type_text refused: input is already active for pid {pid}."
        ))
        .with_structured(serde_json::json!({
            "code": "input_busy",
            "operation": "type_text",
            "pid": pid,
        })));
    }
    Ok(TypeTextGuard { pid })
}

/// Removes the pid from the active set on every completed invocation path,
/// including delivery-worker failures.
#[derive(Debug)]
pub struct TypeTextGuard {
    pid: i64,
}

impl Drop for TypeTextGuard {
    fn drop(&mut self) {
        active_pids()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_second_call_for_the_same_pid_without_queueing() {
        let pid = 2_256_001;
        let _first = try_acquire(pid).expect("first call should acquire the pid");
        let refusal = try_acquire(pid).expect_err("second call should fail fast");

        assert_eq!(refusal.is_error, Some(true));
        assert_eq!(
            refusal
                .structured_content
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .map(serde_json::Map::len),
            Some(3),
            "the refusal payload must remain bounded"
        );
        assert_eq!(
            refusal
                .structured_content
                .as_ref()
                .and_then(|value| value.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("input_busy")
        );
        assert_eq!(
            refusal
                .structured_content
                .as_ref()
                .and_then(|value| value.get("pid"))
                .and_then(serde_json::Value::as_i64),
            Some(pid)
        );
    }

    #[test]
    fn permits_different_pids_and_releases_on_drop() {
        let first_pid = 2_256_002;
        let second_pid = 2_256_003;
        let first = try_acquire(first_pid).expect("first pid should acquire");
        let _second = try_acquire(second_pid).expect("different pid should acquire concurrently");

        drop(first);
        let _reacquired = try_acquire(first_pid).expect("dropped guard should release the pid");
    }
}
