// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Claude Code session export.
//!
//! Moves an authenticated Claude Code CLI session from the source machine into a
//! Linux Cua Space so the destination can relaunch `claude` already logged in.
//! Modeled on the Chrome provider: a `manifest` for consent UIs and granular
//! `export_selected` capture ([`layout::claude_code`]); the receiver lands the
//! files in the guest home and opens a terminal running `claude`.
//!
//! Captured items:
//! - **Logged-in session** (`claude/.credentials.json`, sensitive, default
//!   checked): the OAuth token blob. On macOS it lives in the login Keychain
//!   (`security find-generic-password -s "Claude Code-credentials" -w`); older
//!   installs and non-macOS hosts keep it at `$HOME/.claude/.credentials.json`.
//! - **Config** (`claude/.claude.json`, default checked): `$HOME/.claude.json`,
//!   which carries onboarding/first-run state so the guest doesn't re-prompt.
//! - **Conversations** (`claude/projects/`, sensitive, *not* default checked):
//!   the per-project `*.jsonl` transcripts under `$HOME/.claude/projects/`.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bundle::{BundleWriter, DEFAULT_MAX_TOTAL_BYTES};
use crate::host::{EffectKind, HostCommand, HostEffects, default_host};
use crate::layout::claude_code::{
    APP_IDS as CLAUDE_CODE_APP_IDS, CONFIG_FILE, CONFIG_REL, CRED_REL, CREDENTIALS_FILE,
    KEYCHAIN_SERVICE, PROJECTS_DIR, PROJECTS_PREFIX,
};
use crate::providers::util::{dir_len, file_len};
use crate::{
    AppRef, ExportProvider, ManifestItem, Platform, Result, TeleportError, TransferManifest,
    TransferScope, WindowRef,
};

/// The Claude Code session provider.
pub struct ClaudeCodeProvider {
    /// Test/override hook: when set, this directory stands in for `$HOME`
    /// instead of the process environment.
    home_override: Option<PathBuf>,
    /// Whether the macOS Keychain lookup is attempted before the on-disk
    /// fallback. Disabled by [`Self::without_keychain`] so tests never touch the
    /// developer's real credentials.
    allow_keychain: bool,
    max_total_bytes: u64,
    /// Every Keychain, `$HOME` and authorization effect goes through this.
    host: Arc<dyn HostEffects>,
}

impl Default for ClaudeCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeProvider {
    pub fn new() -> Self {
        Self {
            home_override: None,
            allow_keychain: true,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// Override the source home directory (used by tests and callers that know
    /// the exact home path).
    pub fn with_home(mut self, dir: impl Into<PathBuf>) -> Self {
        self.home_override = Some(dir.into());
        self
    }

    /// Disable the macOS Keychain lookup so capture reads only the on-disk
    /// fallback file (used by tests to stay hermetic — no `security` call).
    pub fn without_keychain(mut self) -> Self {
        self.allow_keychain = false;
        self
    }

    /// Set the total-size guard for exported bundles.
    pub fn with_max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// Resolve the source home directory for the given platform.
    fn source_home(&self, _platform: Platform) -> Option<PathBuf> {
        if let Some(dir) = &self.home_override {
            return Some(dir.clone());
        }
        self.host.home_dir()
    }
}

/// Read the Claude Code OAuth credentials at runtime.
///
/// Prefers the macOS login Keychain (`security find-generic-password -s
/// "Claude Code-credentials" -w`), which is where the current CLI stores them,
/// and falls back to `$HOME/.claude/.credentials.json` when the Keychain lookup
/// fails (non-macOS hosts, older installs, or a denied prompt). The returned
/// bytes are the JSON blob `{"claudeAiOauth":{accessToken,refreshToken,...}}`.
pub fn read_claude_credentials(host: &dyn HostEffects) -> io::Result<Vec<u8>> {
    read_claude_credentials_inner(host, true, host.home_dir())
}

/// Inner form of [`read_claude_credentials`] with the Keychain lookup and home
/// directory injected, so tests can exercise the on-disk fallback path without
/// ever invoking `security` against real credentials.
fn read_claude_credentials_inner(
    host: &dyn HostEffects,
    allow_keychain: bool,
    home: Option<PathBuf>,
) -> io::Result<Vec<u8>> {
    if allow_keychain && let Some(bytes) = read_keychain_credentials(host) {
        return Ok(bytes);
    }
    let home = home.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "could not resolve home directory")
    })?;
    std::fs::read(credentials_fallback_path(&home))
}

/// `$HOME/.claude/.credentials.json` — the on-disk credential fallback.
fn credentials_fallback_path(home: &Path) -> PathBuf {
    home.join(CREDENTIALS_FILE)
}

/// Read the OAuth blob from the macOS login Keychain, returning `None` on any
/// failure (non-macOS, item absent, or a denied Keychain prompt). The `-w` flag
/// prints only the secret to stdout; the trailing newline `security` appends is
/// trimmed.
fn read_keychain_credentials(host: &dyn HostEffects) -> Option<Vec<u8>> {
    let output = host
        .run(
            &HostCommand::new(EffectKind::KeychainRead, "security").args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-w",
            ]),
        )
        .ok()?;
    if !output.success {
        return None;
    }
    let mut bytes = output.stdout;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    (!bytes.is_empty()).then_some(bytes)
}

/// Count the immediate subdirectories of `$HOME/.claude/projects` — Claude Code
/// stores one directory per project, each holding that project's `*.jsonl`
/// transcripts. `None` when the directory is missing so the consent UI falls
/// back to the byte size.
fn count_project_dirs(projects_dir: &Path) -> Option<u64> {
    let entries = std::fs::read_dir(projects_dir).ok()?;
    let count = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .count();
    Some(count as u64)
}

/// Build a [`ManifestItem`], turning an optional `(count, noun)` into the split
/// `count`/`count_noun` fields.
fn manifest_item(
    label: &str,
    rel_path: &str,
    est_bytes: u64,
    sensitive: bool,
    default_checked: bool,
    count: Option<(u64, &str)>,
) -> ManifestItem {
    let (count, count_noun) = match count {
        Some((n, noun)) => (Some(n), Some(noun.to_string())),
        None => (None, None),
    };
    ManifestItem {
        label: label.to_string(),
        rel_path: rel_path.to_string(),
        est_bytes,
        count,
        count_noun,
        sensitive,
        default_checked,
    }
}

/// Copy a file into the bundle, tolerating a read failure by skipping it rather
/// than aborting the whole transfer.
fn add_file_best_effort<W: Write>(
    writer: &mut BundleWriter<W>,
    disk: &Path,
    rel: &str,
    mode: u32,
) -> Result<()> {
    match std::fs::read(disk) {
        Ok(bytes) => writer.add_bytes(rel, mode, &bytes),
        Err(_) => Ok(()),
    }
}

/// Recursively add a directory's files into the bundle under `bundle_prefix`.
fn add_dir_recursive<W: Write>(
    writer: &mut BundleWriter<W>,
    disk: &Path,
    bundle_prefix: &str,
) -> Result<()> {
    let entries = match std::fs::read_dir(disk) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_prefix = format!("{bundle_prefix}/{name}");
        if path.is_dir() {
            add_dir_recursive(writer, &path, &child_prefix)?;
        } else if path.is_file() {
            add_file_best_effort(writer, &path, &child_prefix, 0o600)?;
        }
    }
    Ok(())
}

impl ExportProvider for ClaudeCodeProvider {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn host(&self) -> &dyn HostEffects {
        &*self.host
    }

    fn display_name(&self) -> &str {
        "Claude Code"
    }

    fn platform_supported(&self, platform: Platform) -> bool {
        matches!(platform, Platform::MacOS | Platform::Linux)
    }

    fn install_probe(&self) -> Option<crate::InstallProbe> {
        Some(crate::InstallProbe::on_path("claude"))
    }

    fn matches(&self, app: &AppRef) -> bool {
        CLAUDE_CODE_APP_IDS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&app.app_id))
    }

    fn app_ids(&self) -> &[&str] {
        CLAUDE_CODE_APP_IDS
    }

    fn manifest(
        &self,
        app: &AppRef,
        _window: Option<&WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest> {
        let home = self
            .source_home(app.platform)
            .ok_or_else(|| TeleportError::Provider("could not resolve home directory".into()))?;

        let mut items = Vec::new();

        // Logged-in session (default checked, sensitive). Estimated from the
        // fallback file when present; the Keychain is not probed here so the
        // manifest never triggers a credential prompt.
        items.push(manifest_item(
            "Logged-in session",
            CRED_REL,
            file_len(&credentials_fallback_path(&home)),
            true,
            true,
            None,
        ));

        // Config (default checked, not sensitive).
        items.push(manifest_item(
            "Config",
            CONFIG_REL,
            file_len(&home.join(CONFIG_FILE)),
            false,
            true,
            None,
        ));

        // Conversations (sensitive, NOT default checked) — only offered for a
        // full-profile transfer, matching Chrome's scope gating.
        if scope == TransferScope::FullProfile {
            let projects_dir = home.join(PROJECTS_DIR);
            items.push(manifest_item(
                "Conversations",
                &format!("{PROJECTS_PREFIX}/"),
                dir_len(&projects_dir),
                true,
                false,
                count_project_dirs(&projects_dir).map(|n| (n, "projects")),
            ));
        }

        let notes = vec![
            "The logged-in session includes an OAuth token that grants access to your Claude account.".to_string(),
            "Conversations carry your project transcripts and are unchecked by default.".to_string(),
        ];

        let total_est_bytes = items.iter().map(|item| item.est_bytes).sum();
        Ok(TransferManifest {
            provider_id: self.id().to_string(),
            app_display_name: app.display_name.clone(),
            scope,
            items,
            total_est_bytes,
            notes,
        })
    }

    fn capture_selected(
        &self,
        app: &AppRef,
        scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()> {
        let home = self
            .source_home(app.platform)
            .ok_or_else(|| TeleportError::Provider("could not resolve home directory".into()))?;

        let mut writer = BundleWriter::with_limit(
            out,
            self.id(),
            app.display_name.clone(),
            scope,
            self.max_total_bytes,
        );

        // `None` means "everything the scope implies"; a set restricts capture
        // to the checked `rel_path`s (as reported by `manifest`).
        let wants = |rel: &str| include.is_none_or(|set| set.contains(rel));

        // Logged-in session: Keychain first, on-disk fallback second. Skipped
        // (rather than fatal) when neither yields anything, mirroring the
        // best-effort capture of the other items.
        if wants(CRED_REL)
            && let Ok(bytes) =
                read_claude_credentials_inner(&*self.host, self.allow_keychain, Some(home.clone()))
        {
            writer.add_bytes(CRED_REL, 0o600, &bytes)?;
        }

        // Config, with the host's machine-specific state stripped out.
        if wants(CONFIG_REL) {
            let disk = home.join(CONFIG_FILE);
            if disk.is_file() {
                match std::fs::read(&disk)
                    .ok()
                    .and_then(|b| sanitize_claude_config(&b))
                {
                    Some(clean) => writer.add_bytes(CONFIG_REL, 0o600, &clean)?,
                    None => add_file_best_effort(&mut writer, &disk, CONFIG_REL, 0o600)?,
                }
            }
        }

        // Conversations (full profile only). The manifest advertises the
        // directory as `claude/projects/`, so that is the selection key.
        if scope == TransferScope::FullProfile && wants(&format!("{PROJECTS_PREFIX}/")) {
            let projects_dir = home.join(PROJECTS_DIR);
            if projects_dir.is_dir() {
                add_dir_recursive(&mut writer, &projects_dir, PROJECTS_PREFIX)?;
            }
        }

        writer.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleReader;
    use crate::host::FakeHost;
    use std::io::Cursor;

    /// Build a fake `$HOME` with credentials, config, and two project dirs.
    fn fake_home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(
            claude.join(".credentials.json"),
            br#"{"claudeAiOauth":{"accessToken":"fixture-access"}}"#,
        )
        .unwrap();
        std::fs::write(home.join(".claude.json"), br#"{"onboarding":true}"#).unwrap();
        let projects = claude.join("projects");
        for name in ["-Users-x-proj-a", "-Users-x-proj-b"] {
            let proj = projects.join(name);
            std::fs::create_dir_all(&proj).unwrap();
            std::fs::write(proj.join("session.jsonl"), b"{\"type\":\"user\"}\n").unwrap();
        }
        dir
    }

    fn app() -> AppRef {
        AppRef {
            app_id: "claude-code".into(),
            display_name: "Claude Code".into(),
            platform: Platform::MacOS,
        }
    }

    fn provider_for(home: &Path) -> ClaudeCodeProvider {
        ClaudeCodeProvider::new().without_keychain().with_home(home)
    }

    #[test]
    fn matches_expected_ids() {
        let provider = ClaudeCodeProvider::new();
        for id in [
            "claude-code",
            "Claude Code",
            "com.anthropic.claude-code",
            "claude",
        ] {
            assert!(provider.matches(&AppRef {
                app_id: id.into(),
                display_name: "x".into(),
                platform: Platform::Linux,
            }));
        }
        assert!(!provider.matches(&AppRef {
            app_id: "com.google.Chrome".into(),
            display_name: "Chrome".into(),
            platform: Platform::MacOS,
        }));
    }

    #[test]
    fn manifest_shape_and_defaults() {
        let home = fake_home();
        let provider = provider_for(home.path());
        let manifest = provider
            .manifest(&app(), None, TransferScope::FullProfile)
            .unwrap();
        assert_eq!(manifest.provider_id, "claude-code");

        let session = manifest
            .items
            .iter()
            .find(|i| i.label == "Logged-in session")
            .unwrap();
        assert_eq!(session.rel_path, CRED_REL);
        assert!(session.sensitive);
        assert!(session.default_checked);

        let config = manifest.items.iter().find(|i| i.label == "Config").unwrap();
        assert_eq!(config.rel_path, CONFIG_REL);
        assert!(!config.sensitive);
        assert!(config.default_checked);

        let convos = manifest
            .items
            .iter()
            .find(|i| i.label == "Conversations")
            .unwrap();
        assert_eq!(convos.rel_path, format!("{PROJECTS_PREFIX}/"));
        assert!(convos.sensitive);
        assert!(!convos.default_checked, "conversations must be opt-in");
        assert_eq!(convos.count, Some(2));
        assert_eq!(convos.count_noun.as_deref(), Some("projects"));
    }

    #[test]
    fn manifest_tabs_scope_omits_conversations() {
        let home = fake_home();
        let provider = provider_for(home.path());
        let manifest = provider
            .manifest(&app(), None, TransferScope::TabsOnly)
            .unwrap();
        assert!(
            manifest
                .items
                .iter()
                .any(|i| i.label == "Logged-in session")
        );
        assert!(manifest.items.iter().any(|i| i.label == "Config"));
        assert!(!manifest.items.iter().any(|i| i.label == "Conversations"));
    }

    #[test]
    fn credentials_fallback_reads_the_on_disk_file() {
        // Hermetic: keychain disabled, HOME pointed at a fixture dir. This
        // exercises exactly the runtime fallback branch without invoking
        // `security` against real credentials.
        let home = fake_home();
        let bytes =
            read_claude_credentials_inner(&FakeHost::new(), false, Some(home.path().to_path_buf()))
                .unwrap();
        assert_eq!(
            bytes,
            br#"{"claudeAiOauth":{"accessToken":"fixture-access"}}"#
        );

        // Missing file surfaces an error (so export can skip it).
        let empty = tempfile::tempdir().unwrap();
        assert!(
            read_claude_credentials_inner(
                &FakeHost::new(),
                false,
                Some(empty.path().to_path_buf())
            )
            .is_err()
        );
    }

    #[test]
    fn full_export_packs_credentials_config_and_transcripts() {
        let home = fake_home();
        let provider = provider_for(home.path());

        let mut bundle = Vec::new();
        provider
            .export(&app(), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        assert_eq!(reader.header().provider_id, "claude-code");
        let entries = reader.read_all().unwrap();
        let cred = entries.iter().find(|e| e.rel_path == CRED_REL).unwrap();
        assert_eq!(
            cred.bytes,
            br#"{"claudeAiOauth":{"accessToken":"fixture-access"}}"#
        );
        assert_eq!(cred.mode & 0o777, 0o600);
        assert!(entries.iter().any(|e| e.rel_path == CONFIG_REL));
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.rel_path.starts_with(PROJECTS_PREFIX))
                .count(),
            2
        );
    }

    /// With the Keychain allowed, credentials come from `security` through the
    /// injected host (macOS), never from the real Keychain.
    #[test]
    fn keychain_credentials_go_through_the_injected_host() {
        use crate::host::HostOutput;
        let host = FakeHost::new().with_responder(|_| Ok(HostOutput::ok("{\"k\":1}\n")));
        let bytes = read_claude_credentials_inner(&host, true, None).unwrap();
        assert_eq!(bytes, br#"{"k":1}"#);
        assert_eq!(host.calls_of(EffectKind::KeychainRead).len(), 1);
    }

    #[test]
    fn export_selected_writes_only_the_checked_items() {
        let home = fake_home();
        let provider = provider_for(home.path());

        // Check only the config item.
        let include: HashSet<String> = [CONFIG_REL.to_string()].into_iter().collect();
        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app(),
                TransferScope::FullProfile,
                Some(&include),
                &mut bundle,
            )
            .unwrap();

        let paths: Vec<_> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(
            paths,
            vec![CONFIG_REL.to_string()],
            "only config: {paths:?}"
        );
    }

    #[test]
    fn export_selected_conversations_only_packs_project_files() {
        let home = fake_home();
        let provider = provider_for(home.path());

        let include: HashSet<String> = [format!("{PROJECTS_PREFIX}/")].into_iter().collect();
        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app(),
                TransferScope::FullProfile,
                Some(&include),
                &mut bundle,
            )
            .unwrap();

        let paths: Vec<_> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert!(
            !paths.iter().any(|p| p == CRED_REL),
            "no credentials: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p == CONFIG_REL),
            "no config: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .all(|p| p.starts_with(&format!("{PROJECTS_PREFIX}/"))),
            "only project files: {paths:?}"
        );
        assert_eq!(paths.len(), 2, "two project transcripts: {paths:?}");
    }

    #[test]
    fn export_selected_none_matches_full_export() {
        let home = fake_home();
        let provider = provider_for(home.path());

        let mut a = Vec::new();
        provider
            .export(&app(), TransferScope::FullProfile, &mut a)
            .unwrap();
        let mut b = Vec::new();
        provider
            .export_selected(&app(), TransferScope::FullProfile, None, &mut b)
            .unwrap();

        let paths = |bytes: Vec<u8>| -> Vec<String> {
            BundleReader::open(Cursor::new(bytes))
                .unwrap()
                .header()
                .entries
                .iter()
                .map(|e| e.rel_path.clone())
                .collect()
        };
        assert_eq!(paths(a), paths(b));
    }
}

/// Strip host-specific state from `~/.claude.json` before it travels.
///
/// Two things in that file are meaningless -- and actively harmful -- on
/// another machine:
///
/// * `projects` is keyed by ABSOLUTE host paths (`/Users/<someone>/repo`). They
///   name directories the destination does not have, and they carry the shape of
///   the operator's filesystem into every Space.
/// * `mcpServers` point at binaries and scripts by host path. A destination
///   agent inherits servers it cannot start, and -- worse -- believes it already
///   has Unity/Spaces tooling, so it does not look for the Space's own.
///
/// Everything else (auth, preferences, onboarding state) is what the teleport is
/// actually for and is preserved. Returns None if the file is not JSON we
/// understand, so the caller can fall back to copying it verbatim.
fn sanitize_claude_config(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let obj = v.as_object_mut()?;
    obj.remove("mcpServers");
    if let Some(projects) = obj.get_mut("projects").and_then(|p| p.as_object_mut()) {
        projects.retain(|key, _| !key.starts_with('/'));
    }
    serde_json::to_vec(&v).ok()
}

#[cfg(test)]
mod claude_config_sanitize_tests {
    use super::*;

    #[test]
    fn drops_host_paths_and_mcp_servers_but_keeps_the_rest() {
        let raw = br#"{
            "oauthAccount": {"emailAddress": "a@b.c"},
            "theme": "dark",
            "mcpServers": {"unity-mcp": {"command": "/Users/someone/bin/x"}},
            "projects": {
                "/Users/someone/repo": {"history": [1]},
                "relative-key": {"keep": true}
            }
        }"#;
        let out = sanitize_claude_config(raw).expect("valid json");
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();

        // The credential and preferences -- the point of the teleport -- survive.
        assert_eq!(v["oauthAccount"]["emailAddress"], "a@b.c");
        assert_eq!(v["theme"], "dark");

        // Host-specific state does not.
        assert!(v.get("mcpServers").is_none(), "mcpServers must not travel");
        let projects = v["projects"].as_object().unwrap();
        assert!(
            !projects.contains_key("/Users/someone/repo"),
            "absolute host paths must be dropped"
        );
        assert!(
            projects.contains_key("relative-key"),
            "non-path keys are kept"
        );
    }

    #[test]
    fn non_json_falls_back_to_verbatim_copy() {
        assert!(sanitize_claude_config(b"not json").is_none());
    }
}
