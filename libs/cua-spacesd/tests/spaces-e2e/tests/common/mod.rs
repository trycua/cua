// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A real cua-spacesd server core, in-process on loopback.
//!
//! Host safety: the server runs child processes as this user, so every test
//! Space gets `SystemService.Init(env = {HOME: <temp>, PATH: <temp bin>:…})`
//! before anything else. Guest `$HOME`, Downloads, the teleport import home
//! and the data dir are all temp directories; the teleport receiver and the
//! sender both use `FakeHost`; tool calls go to a fake registry. Nothing here
//! reads or writes the real home directory, launches an app or touches the
//! keychain.

#![allow(dead_code)]

use cua_spaces::{Space, Spaces};
pub use cua_spaces_e2e::{Driver, TOKEN, driver};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A `Spaces` with its registry in a temp dir and no operator display.
pub fn spaces(registry: &Path) -> Spaces {
    Spaces::builder()
        .home(registry)
        .download_dir(registry.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(10))
        .build()
}

/// Adds the driver, confines it, and returns the connected Space.
pub async fn added(spaces: &Spaces, d: &Driver) -> Space {
    let info = spaces
        .add(&d.url, Some(TOKEN.into()), Some("test".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    d.confine(&space).await;
    space
}

/// Serializes the span in which `$CUA_HOME` is overridden (a process-wide
/// env var) across this binary's concurrently-running tests.
static CUA_HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Points `$CUA_HOME` at `dir` for `f`, restoring it after. Holds
/// [`CUA_HOME_LOCK`] for the whole span so no other test in this binary
/// observes or overrides it meanwhile.
///
/// A Keyvault-routed teleport (`cua_spaces_ext::teleport::SpaceTeleport`)
/// reaches the Cua Keyvault over `$CUA_HOME/keyvault.sock`, by design the
/// same real, per-OS-user socket in every process (never scoped by a
/// `Spaces`/`RuntimeConfig` home dir): a malicious embedder overriding it is
/// exactly what the Keyvault treats as untrusted (red-team F16). So any test
/// that can reach that code path must call this rather than leave
/// `$CUA_HOME` at its real default, or it risks reaching the developer's own
/// real `~/.cua/keyvault.sock`.
pub async fn with_isolated_cua_home<T>(dir: &Path, f: impl std::future::Future<Output = T>) -> T {
    let _guard = CUA_HOME_LOCK.lock().await;
    let saved = std::env::var_os("CUA_HOME");
    // SAFETY: serialized by `CUA_HOME_LOCK`, the only place in this binary
    // that touches `CUA_HOME`.
    unsafe { std::env::set_var("CUA_HOME", dir) };
    let out = f.await;
    // SAFETY: as above.
    unsafe {
        match &saved {
            Some(v) => std::env::set_var("CUA_HOME", v),
            None => std::env::remove_var("CUA_HOME"),
        }
    }
    out
}
