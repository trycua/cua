//! The repo cache in a local Space: installing what makes a plain
//! `git clone` use it, learning from the clones the daemon can see, and
//! reading back what the Space's helper did. Every failure here is silent:
//! the Space simply clones the normal way.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cua_sandbox_core::repo_cache::{self, shim};

use crate::space::Space;

/// Local Spaces whose git uses the cache now.
fn installed() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Makes a Space that just came up use the cache: installs the helper next to
/// its git and includes the cache's git config (a script that does nothing in
/// a Space without the mount, without git, or without the rights). Takes
/// well under a tenth of a second and never longer than a few seconds.
pub(crate) async fn install(space: &Space, name: &str) {
    let cache = repo_cache::current();
    if !cache.config().enabled || cache.prepare().is_err() {
        return;
    }
    let script = shim::install_script();
    let run = tokio::time::timeout(
        Duration::from_secs(4),
        space.bash(&script, Duration::from_secs(3)),
    )
    .await;
    if let Ok(Ok(out)) = run
        && out.success()
        && out.stdout.contains(shim::INSTALLED_MARKER)
        && let Ok(mut s) = installed().lock()
    {
        s.insert(name.to_string());
    }
}

/// A command run in a Space: what it clones is learned (see
/// [`cua_sandbox_core::repo_cache::RepoCache::learn`]), and what the Space's
/// helper logged since is counted. Local Spaces only.
pub(crate) fn observe_command(space: &Space, command: &str) {
    if !matches!(space.id(), crate::id::SpaceId::Local { .. }) {
        return;
    }
    // Most commands clone nothing: no settings are read for them.
    if repo_cache::names_a_clone(command) {
        repo_cache::current().learn(command);
    }
}

/// Counts what the helper in `space` logged since last asked, when its git
/// uses the cache. Bounded to a second and a half.
pub(crate) async fn collect(space: &Space, name: &str) {
    let uses = installed().lock().is_ok_and(|s| s.contains(name));
    if !uses {
        return;
    }
    let cmd = shim::take_log_command();
    let run = tokio::time::timeout(
        Duration::from_millis(1500),
        space.bash(&cmd, Duration::from_secs(1)),
    )
    .await;
    if let Ok(Ok(out)) = run
        && !out.stdout.is_empty()
    {
        repo_cache::current().ingest_guest_log(&out.stdout);
    }
}

/// The Space is gone (deleted or stopped): it no longer uses the cache.
pub(crate) fn forget(name: &str) {
    if let Ok(mut s) = installed().lock() {
        s.remove(name);
    }
}
