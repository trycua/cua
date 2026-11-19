//! Pushes every sandbox's deadline to the Cua side, so a sandbox ends at its
//! E2B `timeout` / Daytona auto-delete even when this gateway (and its
//! reaper, [`crate::state::reaper`]) is dead: the Cua daemon enforces the
//! Space lifetime. The gateway's reaper stays the authority while it runs
//! (pausing, auto-resume and timeout refreshes change deadlines); this
//! task only keeps the daemon's copy equal to the record's, so the daemon's
//! deadline is never earlier than the record says and never later.
//!
//! Backends without a Cua-side lifetime answer `Ok(false)` and nothing
//! changes: the gateway reaper remains the only enforcer.

use crate::backend::Backend;
use crate::state::{OnTimeout, Store, now_ms};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// The deadline the daemon should hold for a record: the soonest of its
/// soft and hard ends, `None` when it has none (a paused sandbox that is
/// only resumed by hand).
fn deadline(r: &crate::state::Record) -> Option<(u64, &'static str)> {
    let soft = (!r.paused || r.on_timeout == OnTimeout::Kill)
        .then_some(r.end_at_ms)
        .flatten()
        .map(|t| {
            (
                t,
                if r.on_timeout == OnTimeout::Pause && !r.paused {
                    "suspend"
                } else {
                    "delete"
                },
            )
        });
    // A hard end always deletes.
    let hard = r.hard_end_at_ms.map(|t| (t, "delete"));
    [soft, hard].into_iter().flatten().min_by_key(|(t, _)| *t)
}

/// Keeps the Cua-side lifetimes equal to the records' (spawn once).
pub async fn sync(store: Arc<Store>, backend: Arc<dyn Backend>) {
    let mut pushed: HashMap<String, (u64, &'static str)> = HashMap::new();
    loop {
        let records = store.all().await;
        pushed.retain(|id, _| records.iter().any(|r| &r.id == id));
        for r in records {
            let Some((at, action)) = deadline(&r) else {
                continue;
            };
            if pushed.get(&r.id) == Some(&(at, action)) {
                continue;
            }
            let secs = at.saturating_sub(now_ms()).div_ceil(1000).max(1);
            match backend
                .set_lifetime(&r.identity(), &r.cua_name, secs, action)
                .await
            {
                Ok(_) => {
                    pushed.insert(r.id.clone(), (at, action));
                }
                // Retry on the next pass (daemon down, sandbox starting).
                Err(e) => tracing::debug!("compat: lifetime of {}: {e}", r.id),
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), store.synced.notified()).await;
    }
}
