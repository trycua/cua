//! Keeps the spawn channel of every running Space on this machine that has
//! children on, and ends the capabilities of Spaces that are gone.
//!
//! For each such Space the host mints a capability, starts the guest's end
//! (`TunnelService.StartSpawn`) and keeps the WebSocket open, serving each
//! stream that arrives with [`super::channel_app`] for that Space. When the
//! guest restarts, the socket closes and the host starts the channel again
//! with a fresh capability (the old one, in a snapshot or a fork, is dead).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::capability::{DEFAULT_MAX_DEPTH, DEFAULT_TTL_SECS, RENEW_AFTER_SECS};
use crate::server::Shared;

/// How often the loop looks again without being woken.
const EVERY: Duration = Duration::from_secs(5);
/// Wait before trying a Space that could not be reached again.
const RETRY: Duration = Duration::from_secs(5);
/// Wait before asking a guest without the spawn channel again (an older
/// cua-spacesd).
const NO_FEATURE_RETRY: Duration = Duration::from_secs(60);

/// How often orphaned children are judged.
const REAP_EVERY: Duration = Duration::from_secs(60);

/// What the reaper sees of this machine's registry, or `None` when it can
/// not be read (then nothing is decided).
pub(crate) fn seen(shared: &Shared) -> Option<Vec<super::reaper::Seen>> {
    let spaces = shared.runtime.spaces();
    let records = spaces.registry().list().ok()?;
    let meta = spaces.registry().all_meta();
    Some(
        records
            .into_iter()
            .map(|r| {
                let m = meta.get(&r.id);
                super::reaper::Seen {
                    parent: m.and_then(|m| m.labels.get(super::api::LABEL_PARENT).cloned()),
                    created_at: m.map(|m| m.created_at_unix).unwrap_or(0),
                    id: r.id,
                }
            })
            .collect(),
    )
}

/// Judges the children of this machine and ends the ones whose parent is
/// provably gone for the grace period (each with everything below it).
pub(crate) fn reap(shared: &Arc<Shared>) {
    use super::reaper::{Memory, Verdict, judge};
    let Some(seen) = seen(shared) else {
        return;
    };
    let home = shared.runtime.spaces().home_dir().to_path_buf();
    let mut memory = Memory::load(&home);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let verdicts = judge(&seen, &mut memory, now);
    if let Err(e) = memory.save(&home) {
        tracing::warn!(error = %e, "could not save the orphan records");
    }
    let links = super::api::links_of(shared.runtime.spaces());
    for (id, verdict) in verdicts {
        let Verdict::End { why } = verdict else {
            continue;
        };
        let mut order = super::tree::subtree(&links, &id);
        order.push(id.clone());
        tracing::warn!(space = %id, below = order.len() - 1, %why, "ending an orphaned child Space");
        let shared = shared.clone();
        tokio::spawn(async move {
            use cua_proto::daemon::v1::space_service_server::SpaceService as _;
            for victim in order {
                let owner = crate::server::Svc(shared.clone());
                let r = owner
                    .delete_space(tonic::Request::new(
                        cua_proto::daemon::v1::DeleteSpaceRequest {
                            space: victim.clone(),
                        },
                    ))
                    .await;
                if let Some(a) = shared.spawn_authority() {
                    a.revoke(&victim);
                }
                match r {
                    Ok(_) => tracing::info!(space = %victim, "orphaned child Space ended"),
                    Err(e) if e.code() == tonic::Code::NotFound => {}
                    Err(e) => {
                        tracing::warn!(space = %victim, error = %e, "could not end an orphaned child Space")
                    }
                }
            }
        });
    }
}

/// Only Spaces this machine runs itself hold a channel to it.
fn runs_here(id: &str) -> bool {
    id.starts_with("local:")
}

/// The loop; ends with the daemon.
pub(crate) fn spawn(shared: Arc<Shared>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if shared.children.is_none() {
            return;
        }
        let mut bridges: BTreeMap<String, JoinHandle<()>> = BTreeMap::new();
        let mut last_reap = std::time::Instant::now() - REAP_EVERY;
        loop {
            reconcile(&shared, &mut bridges);
            if last_reap.elapsed() >= REAP_EVERY {
                last_reap = std::time::Instant::now();
                reap(&shared);
            }
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = cua_spaces::children::CHANGED.notified() => {}
                _ = tokio::time::sleep(EVERY) => {}
            }
        }
        for (_, t) in bridges {
            t.abort();
        }
    })
}

/// One pass: channels for the Spaces that want one, none for the rest, and
/// no capability or setting left for a Space that is gone.
pub(crate) fn reconcile(shared: &Arc<Shared>, bridges: &mut BTreeMap<String, JoinHandle<()>>) {
    let Some(children) = shared.children.as_deref() else {
        return;
    };
    let spaces = shared.runtime.spaces();
    let Ok(records) = spaces.registry().list() else {
        return;
    };
    let registered: BTreeSet<String> = records.into_iter().map(|r| r.id).collect();
    for id in children.authority.spaces() {
        if !registered.contains(&id) {
            children.authority.revoke(&id);
        }
    }
    spaces.forget_children_of_gone(&registered);
    let wanted: BTreeSet<String> = spaces
        .children_enabled()
        .into_iter()
        .filter(|id| runs_here(id))
        .collect();
    bridges.retain(|id, task| {
        let keep = wanted.contains(id) && !task.is_finished();
        if !keep {
            task.abort();
            if !wanted.contains(id) {
                children.authority.revoke(id);
            }
        }
        keep
    });
    for id in wanted {
        if !bridges.contains_key(&id) {
            bridges.insert(id.clone(), tokio::spawn(run(shared.clone(), id)));
        }
    }
}

/// The depth limit of `root`'s tree: what its own capability carries, else
/// the default.
fn tree_limit(shared: &Shared, root: &str) -> u32 {
    shared
        .spawn_authority()
        .and_then(|a| a.record(root))
        .map(|r| r.max_depth)
        .unwrap_or(DEFAULT_MAX_DEPTH)
}

/// Keeps one Space's channel until the task is aborted.
async fn run(shared: Arc<Shared>, id: String) {
    let mut warned = false;
    loop {
        let spaces = shared.runtime.spaces();
        let space = match spaces.space(&id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(space = %id, error = %e, "spawn channel waits for the Space");
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        if !space
            .feature(cua_spaces::spawn::SPAWN_FEATURE)
            .is_some_and(|f| f.supported)
        {
            if !warned {
                tracing::info!(
                    space = %id,
                    "this Space's cua-spacesd has no spawn channel; it cannot make children"
                );
                warned = true;
            }
            tokio::time::sleep(NO_FEATURE_RETRY).await;
            continue;
        }
        let Some(authority) = shared.spawn_authority() else {
            return;
        };
        let (root, depth) = super::api::position(spaces, &id);
        let limit = if root == id {
            DEFAULT_MAX_DEPTH
        } else {
            tree_limit(&shared, &root)
        };
        let capability = authority.mint(&id, &root, depth, limit, DEFAULT_TTL_SECS);
        let app = super::channel_app(shared.clone(), id.clone());
        let dial = move |_host: String, _port: u16| {
            let app = app.clone();
            async move {
                let (ours, theirs) = tokio::io::duplex(256 * 1024);
                tokio::spawn(super::serve_stream(app, theirs));
                Ok::<_, std::io::Error>(ours)
            }
        };
        let mut channel = match space.start_spawn_with(&capability, dial).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(space = %id, error = %e, "could not start the spawn channel");
                authority.revoke(&id);
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        tracing::info!(space = %id, address = %channel.address(), "spawn channel up");
        tokio::select! {
            _ = channel.closed() => {
                tracing::info!(space = %id, "spawn channel closed; starting it again");
            }
            _ = tokio::time::sleep(Duration::from_secs(RENEW_AFTER_SECS)) => {}
        }
    }
}
