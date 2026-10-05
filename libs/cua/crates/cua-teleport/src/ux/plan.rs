// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `plan`: exactly what a teleport will do and move, before anything does.
//!
//! A [`TeleportPlan`] is built from a catalog entry, the Space's facts, the
//! chosen move and (for state) the provider's manifest. It lists:
//!
//! - the steps: install (pinned and verified), send files, import state,
//!   launch;
//! - the consent items: every host path, state item and secret that leaves
//!   this machine, and every install, with sizes.
//!
//! Only [`TeleportPlan::approve`] turns a plan into an [`ApprovedPlan`], the
//! one thing a run accepts, and it refuses a plan with a secret unless the
//! caller acknowledges secrets.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::UxError;
use super::catalog::{Capability, CatalogEntry, InstallSource, MoveKind, SensitiveGroup};
use crate::{Platform, TransferManifest, TransferScope};

/// Where sent files land, under the Space user's `~/Downloads`.
pub const FILES_SUBDIR: &str = "Teleported";
/// Walk bound for sizing chosen folders.
pub const MAX_WALK_ENTRIES: u64 = 200_000;

/// What the Space is (from its capabilities and `uname`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceFacts {
    /// Space id.
    pub space_id: String,
    /// Its OS.
    pub os: Platform,
    /// CPU family (`aarch64`, `x86_64`).
    pub arch: String,
    /// The Space user's home.
    pub home: String,
    /// Provider ids it can import (`teleport.<id>` features).
    pub importers: Vec<String>,
}

/// The options a UI collects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOptions {
    /// What moves.
    pub moves: MoveKind,
    /// Host files or folders ([`MoveKind::AppWithFiles`]).
    #[serde(default)]
    pub files: Vec<String>,
    /// Provider manifest `rel_path`s ([`MoveKind::AppWithState`]); `None`
    /// is the provider's default selection.
    #[serde(default)]
    pub state_items: Option<Vec<String>>,
    /// With `state_items` `None`: also select the items of these
    /// [`SensitiveGroup`]s, which the provider leaves out by default
    /// ([`SensitiveGroup::SignIns`] keeps the app signed in). Each is listed
    /// as a secret in the consent, so the plan needs the acknowledgement.
    #[serde(default)]
    pub sensitive_groups: Vec<SensitiveGroup>,
    /// State scope.
    #[serde(default = "default_scope")]
    pub scope: TransferScope,
    /// Launch the app when done (default true).
    #[serde(default = "yes")]
    pub launch: bool,
}

fn default_scope() -> TransferScope {
    TransferScope::FullProfile
}
fn yes() -> bool {
    true
}

impl PlanOptions {
    /// `moves` with defaults.
    pub fn new(moves: MoveKind) -> Self {
        Self {
            moves,
            files: vec![],
            state_items: None,
            sensitive_groups: vec![],
            scope: TransferScope::FullProfile,
            launch: true,
        }
    }
}

/// One chosen host path, sized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    /// Absolute host path.
    pub path: String,
    /// A folder.
    pub is_dir: bool,
    /// Total bytes of regular files.
    pub bytes: u64,
    /// Regular files (1 for a file).
    pub files: u64,
}

/// Sizes chosen paths (symlinks are not followed; a folder walk stops at
/// [`MAX_WALK_ENTRIES`]).
pub fn stat_paths(paths: &[String]) -> Result<Vec<FileStat>, UxError> {
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            if !path.is_absolute() {
                return Err(UxError::Invalid(format!("{p:?} is not an absolute path")));
            }
            let meta = std::fs::symlink_metadata(path)
                .map_err(|e| UxError::Invalid(format!("{p}: {e}")))?;
            if meta.is_dir() {
                let (bytes, files) = walk_size(path);
                Ok(FileStat {
                    path: p.clone(),
                    is_dir: true,
                    bytes,
                    files,
                })
            } else if meta.is_file() {
                Ok(FileStat {
                    path: p.clone(),
                    is_dir: false,
                    bytes: meta.len(),
                    files: 1,
                })
            } else {
                Err(UxError::Invalid(format!("{p} is not a file or folder")))
            }
        })
        .collect()
}

fn walk_size(root: &Path) -> (u64, u64) {
    let mut stack = vec![root.to_path_buf()];
    let (mut bytes, mut files, mut seen) = (0u64, 0u64, 0u64);
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > MAX_WALK_ENTRIES {
                return (bytes, files);
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                bytes += m.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// One step of a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanStep {
    /// Install pinned, verified items (dependencies first).
    Install {
        /// Installable ids, in install order.
        ids: Vec<String>,
    },
    /// Send host files or folders into `~/Downloads/<subdir>`.
    SendFiles {
        /// Host paths.
        paths: Vec<String>,
        /// Subdirectory of `~/Downloads`.
        subdir: String,
    },
    /// Import the provider's selected state (the Space relaunches the app).
    ImportState {
        /// Provider id.
        provider_id: String,
        /// Scope.
        scope: TransferScope,
        /// Manifest `rel_path`s.
        items: Vec<String>,
    },
    /// Start the app in the Space.
    Launch {
        /// Binary.
        bin: String,
        /// Arguments (trusted table args, then guest file paths).
        args: Vec<String>,
        /// Guest paths opened (also in `args`).
        files: Vec<String>,
        /// A terminal program.
        terminal: bool,
    },
}

impl PlanStep {
    /// Short name (`install`, `files`, `state`, `launch`).
    pub fn name(&self) -> &'static str {
        match self {
            PlanStep::Install { .. } => "install",
            PlanStep::SendFiles { .. } => "files",
            PlanStep::ImportState { .. } => "state",
            PlanStep::Launch { .. } => "launch",
        }
    }
}

/// What a consent item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentKind {
    /// An install into the Space (nothing leaves this machine).
    Install,
    /// A host file that leaves this machine.
    File,
    /// A host folder that leaves this machine.
    Folder,
    /// App state that leaves this machine.
    State,
    /// A secret (cookies, tokens, logins) that leaves this machine.
    Secret,
}

/// One line of the consent screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentItem {
    /// Kind.
    pub kind: ConsentKind,
    /// Stable key: the installable id, host path, or manifest `rel_path`.
    pub key: String,
    /// Label.
    pub label: String,
    /// Detail line (version and checksum, destination, count).
    pub detail: String,
    /// Bytes that leave this machine (0 for installs).
    pub bytes: u64,
    /// Credentials, cookies, tokens or transcripts.
    pub sensitive: bool,
}

/// Exactly what a teleport will do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeleportPlan {
    /// The app.
    pub app: CatalogEntry,
    /// Target Space id.
    pub space_id: String,
    /// What moves.
    pub moves: MoveKind,
    /// Steps, in run order.
    pub steps: Vec<PlanStep>,
    /// Every install, path and secret, for the consent screen.
    pub consent: Vec<ConsentItem>,
    /// Any consent item is sensitive.
    pub sensitive: bool,
    /// Bytes that leave this machine.
    pub total_bytes: u64,
    /// Caveats to show.
    pub warnings: Vec<String>,
    /// Whether an [`MoveKind::AppWithState`] move keeps its captured session
    /// sealed in the Cua Keyvault after delivery, for reuse without asking
    /// again. Set from [`Consent::save_to_keyvault`] by [`Self::approve`];
    /// `false` (the review sheet's unchecked default) forgets the captured
    /// item right after this one delivery, exactly as before this field
    /// existed. Meaningless (and ignored) when nothing sensitive moves.
    #[serde(default)]
    pub save_to_keyvault: bool,
    /// This Space is reached through `cua-relay` and reported no sealed-
    /// delivery key: a secret this plan sends would cross the relay in the
    /// clear (S1). `false` for a direct connection, a Fleet Gateway, or a
    /// relay-routed Space whose image already seals. The caller computing
    /// the plan sets this (`plan::build` leaves it `false`; it has no
    /// network access to check). Defaults to `false` on deserializing an
    /// older plan JSON (never retroactively flagged).
    #[serde(default)]
    pub relay_unsealed: bool,
    /// The sites (registrable domains) whose cookies this plan sends, from
    /// the review's per-domain choice ([`Consent::cookie_domains`]); `None`
    /// sends every cookie in the selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_domains: Option<Vec<String>>,
    /// Send these saved Keyvault items (ids) instead of reading the live
    /// app ([`Consent::from_vault`]): no fresh capture, so the host's
    /// Keychain is never asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_vault: Option<Vec<String>>,
    /// Also send the saved passwords ([`Consent::include_passwords`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_passwords: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Whether a manifest key is the browser's cookie store.
fn is_cookie_key(key: &str) -> bool {
    let name = key.rsplit('/').next().unwrap_or(key);
    name.eq_ignore_ascii_case("cookies")
}

/// Consent, as a UI collects it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    /// The user confirmed the consent screen.
    pub approved: bool,
    /// The user saw and accepted the secrets.
    pub acknowledge_sensitive: bool,
    /// "Save to Keyvault": keep the captured session sealed in the user's
    /// own Cua Keyvault after this delivery, instead of forgetting it.
    #[serde(default)]
    pub save_to_keyvault: bool,
    /// The user saw and accepted [`TeleportPlan::relay_unsealed`]'s warning
    /// (S1).
    #[serde(default)]
    pub acknowledge_relay_plaintext: bool,
    /// The review's per-domain choice: the registrable domains whose cookies
    /// to send. `None` keeps every cookie the selection holds; `Some` sends
    /// only these (and, when empty, no cookies at all).
    #[serde(default)]
    pub cookie_domains: Option<Vec<String>>,
    /// Consent items (their keys) the user turned off in the review. They
    /// are dropped from the plan: not listed, not sent, not counted.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Send these saved Keyvault items (ids) instead of reading the live
    /// app. Only an app-state move uses it.
    #[serde(default)]
    pub from_vault: Option<Vec<String>>,
    /// Also send the browser's saved passwords (of the chosen sites): only
    /// when the user ticked them in the review, and only with
    /// `acknowledge_sensitive`. They are re-encrypted for the destination
    /// browser's own key.
    #[serde(default)]
    pub include_passwords: bool,
}

/// A plan the user approved. Only [`TeleportPlan::approve`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovedPlan(TeleportPlan);

impl ApprovedPlan {
    /// The plan.
    pub fn plan(&self) -> &TeleportPlan {
        &self.0
    }
}

impl TeleportPlan {
    /// Mints the approval a run needs. Refuses without `approved`, a plan
    /// carrying a secret without `acknowledge_sensitive`, and (while
    /// [`crate::RELAY_SEALING_ENFORCED`] is on) a relay-unsealed plan
    /// without `acknowledge_relay_plaintext` (S1).
    pub fn approve(mut self, consent: Consent) -> Result<ApprovedPlan, UxError> {
        if !consent.approved {
            return Err(UxError::NotApproved("the teleport was not approved".into()));
        }
        if self.sensitive && !consent.acknowledge_sensitive {
            let secrets: Vec<&str> = self
                .consent
                .iter()
                .filter(|c| c.sensitive)
                .map(|c| c.label.as_str())
                .collect();
            return Err(UxError::NotApproved(format!(
                "these secrets need an explicit acknowledgement: {}",
                secrets.join(", ")
            )));
        }
        if consent.include_passwords && !consent.acknowledge_sensitive {
            return Err(UxError::NotApproved(
                "saved passwords need an explicit acknowledgement".into(),
            ));
        }
        if self.relay_unsealed
            && crate::RELAY_SEALING_ENFORCED
            && !consent.acknowledge_relay_plaintext
        {
            return Err(UxError::NotApproved(format!(
                "{}; acknowledge_relay_plaintext to send anyway",
                crate::RELAY_UNSEALED_WARNING
            )));
        }
        // What the user turned off in the review leaves the plan: first the
        // items they excluded, then, when they chose domains, the cookie
        // store itself if no domain was chosen (nothing to read).
        let mut drop_keys: Vec<String> = consent.exclude.clone();
        if consent
            .cookie_domains
            .as_ref()
            .is_some_and(|d| d.is_empty())
        {
            drop_keys.extend(
                self.consent
                    .iter()
                    .filter(|c| is_cookie_key(&c.key))
                    .map(|c| c.key.clone()),
            );
        }
        if !drop_keys.is_empty() {
            for step in &mut self.steps {
                if let PlanStep::ImportState { items, .. } = step {
                    items.retain(|i| !drop_keys.contains(i));
                }
            }
            self.consent.retain(|c| !drop_keys.contains(&c.key));
            self.total_bytes = self.consent.iter().map(|c| c.bytes).sum();
            self.sensitive = self.consent.iter().any(|c| c.sensitive);
        }
        self.cookie_domains = consent.cookie_domains.clone();
        self.from_vault = consent.from_vault.clone();
        self.include_passwords = consent.include_passwords;
        self.save_to_keyvault = self.sensitive && consent.save_to_keyvault;
        Ok(ApprovedPlan(self))
    }
}

/// Builds the plan. `manifest` is the provider's (required for
/// [`MoveKind::AppWithState`]); `files` are the sized chosen paths.
pub fn build(
    entry: &CatalogEntry,
    facts: &SpaceFacts,
    options: &PlanOptions,
    manifest: Option<&TransferManifest>,
    files: &[FileStat],
) -> Result<TeleportPlan, UxError> {
    if entry.capability == Capability::Unsupported {
        return Err(UxError::Unsupported(format!(
            "{} cannot be teleported: {}",
            entry.name,
            entry.reason.as_deref().unwrap_or("unsupported")
        )));
    }
    if !entry.offers(options.moves) {
        return Err(UxError::Invalid(format!(
            "{} offers {}; not {}",
            entry.name,
            entry
                .moves
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            options.moves.as_str()
        )));
    }
    let mut steps = vec![];
    let mut consent = vec![];
    let mut warnings = vec![];
    if let Some(r) = &entry.reason {
        warnings.push(r.clone());
    }

    // 1. Install.
    match &entry.install {
        Some(InstallSource::Manifest { id, .. }) => {
            if facts.os != Platform::Linux {
                return Err(UxError::Unsupported(format!(
                    "{} installs from the Linux install manifest; this Space is not Linux",
                    entry.name
                )));
            }
            let ids = cua_agents::installables::resolve(&[id.as_str()])
                .map_err(|e| UxError::Invalid(e.to_string()))?;
            for i in &ids {
                let it = cua_agents::installables::get(i).expect("resolved");
                let verify = match &it.how {
                    cua_agents::installables::Source::Archive { archives, .. } => {
                        match archives.get(&facts.arch) {
                            Some(a) => format!("sha256 {}", a.sha256),
                            None => {
                                return Err(UxError::Unsupported(format!(
                                    "{} {} is not published for {}",
                                    it.name, it.version, facts.arch
                                )));
                            }
                        }
                    }
                    cua_agents::installables::Source::Npm { .. } => "npm sha512 integrity".into(),
                    cua_agents::installables::Source::GitUv { commit, .. } => {
                        format!("git commit {commit}, uv.lock hashes")
                    }
                };
                consent.push(ConsentItem {
                    kind: ConsentKind::Install,
                    key: i.clone(),
                    label: format!("Install {} {}", it.name, it.version),
                    detail: format!("pinned, verified by {verify} ({})", it.license),
                    bytes: 0,
                    sensitive: false,
                });
            }
            steps.push(PlanStep::Install { ids });
        }
        Some(InstallSource::Image) | Some(InstallSource::Space) | None => {}
    }

    // 2. Files.
    let mut guest_files = vec![];
    if options.moves == MoveKind::AppWithFiles {
        if options.files.is_empty() {
            return Err(UxError::Invalid(
                "choose at least one file or folder".into(),
            ));
        }
        let dest = format!(
            "{}/Downloads/{FILES_SUBDIR}",
            facts.home.trim_end_matches('/')
        );
        for p in &options.files {
            let st = files.iter().find(|f| &f.path == p).ok_or_else(|| {
                UxError::Invalid(format!("{p} was not sized (call stat_paths first)"))
            })?;
            let name = Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone());
            let guest = format!("{dest}/{name}");
            consent.push(ConsentItem {
                kind: if st.is_dir {
                    ConsentKind::Folder
                } else {
                    ConsentKind::File
                },
                key: p.clone(),
                label: p.clone(),
                detail: if st.is_dir {
                    format!("{} files, to {guest}", st.files)
                } else {
                    format!("to {guest}")
                },
                bytes: st.bytes,
                sensitive: false,
            });
            guest_files.push(guest);
        }
        steps.push(PlanStep::SendFiles {
            paths: options.files.clone(),
            subdir: FILES_SUBDIR.into(),
        });
    } else if !options.files.is_empty() {
        return Err(UxError::Invalid(format!(
            "files are only sent with app_with_files (got {})",
            options.moves.as_str()
        )));
    }

    // 3. State.
    if options.moves == MoveKind::AppWithState {
        let provider = entry
            .provider_id
            .clone()
            .ok_or_else(|| UxError::Unsupported(format!("{} has no state provider", entry.name)))?;
        if !facts.importers.iter().any(|i| i == &provider) {
            return Err(UxError::Unsupported(format!(
                "this Space cannot import {provider} state (no teleport.{provider} feature)"
            )));
        }
        let m = manifest.ok_or_else(|| {
            UxError::Invalid(format!(
                "{provider}: the provider manifest is required for state"
            ))
        })?;
        let selected: Vec<&crate::ManifestItem> = match &options.state_items {
            None => m
                .items
                .iter()
                .filter(|i| {
                    i.default_checked
                        || (i.sensitive
                            && SensitiveGroup::of_item(&provider, &i.rel_path)
                                .is_some_and(|g| options.sensitive_groups.contains(&g)))
                })
                .collect(),
            Some(keys) => {
                let mut out = vec![];
                for k in keys {
                    out.push(m.items.iter().find(|i| &i.rel_path == k).ok_or_else(|| {
                        UxError::Invalid(format!("{k:?} is not in the {provider} manifest"))
                    })?);
                }
                out
            }
        };
        if selected.is_empty() {
            return Err(UxError::Invalid("select at least one state item".into()));
        }
        for i in &selected {
            let count = match (i.count, &i.count_noun) {
                (Some(n), Some(noun)) => Some(format!("{n} {noun}")),
                _ => None,
            };
            // Where it lands in the Space (the receiver's per-OS layout), not
            // the bundle's canonical path; bundle-internal items show only
            // their count.
            let place = crate::layout::landing_path(&provider, &i.rel_path, facts.os)
                .map(|p| format!("~/{p}"));
            let detail = match (count, place) {
                (Some(c), Some(p)) => format!("{c}, {p}"),
                (Some(c), None) => c,
                (None, Some(p)) => p,
                (None, None) => i.rel_path.clone(),
            };
            consent.push(ConsentItem {
                kind: if i.sensitive {
                    ConsentKind::Secret
                } else {
                    ConsentKind::State
                },
                key: i.rel_path.clone(),
                label: i.label.clone(),
                detail,
                bytes: i.est_bytes,
                sensitive: i.sensitive,
            });
        }
        warnings.extend(m.notes.iter().cloned());
        steps.push(PlanStep::ImportState {
            provider_id: provider,
            scope: options.scope,
            items: selected.iter().map(|i| i.rel_path.clone()).collect(),
        });
    }

    // 4. Launch (a state import relaunches the app itself).
    if options.launch
        && options.moves != MoveKind::AppWithState
        && let Some(l) = &entry.launch
    {
        let mut args = l.args.clone();
        args.extend(guest_files.iter().cloned());
        steps.push(PlanStep::Launch {
            bin: l.bin.clone(),
            args,
            files: guest_files,
            terminal: l.terminal,
        });
    }

    let sensitive = consent.iter().any(|c| c.sensitive);
    let total_bytes = consent.iter().map(|c| c.bytes).sum();
    Ok(TeleportPlan {
        app: entry.clone(),
        space_id: facts.space_id.clone(),
        moves: options.moves,
        steps,
        consent,
        sensitive,
        total_bytes,
        warnings,
        save_to_keyvault: false,
        relay_unsealed: false,
        cookie_domains: None,
        from_vault: None,
        include_passwords: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ManifestItem;
    use crate::ux::apps::HostApp;
    use crate::ux::catalog::{TargetHint, classify};
    use crate::ux::testing::providers;

    fn entry(name: &str, id: &str) -> CatalogEntry {
        classify(
            &HostApp {
                name: name.into(),
                app_id: Some(id.into()),
                path: format!("/fixture/{name}.app").into(),
                version: None,
                icon: None,
                platform: Platform::MacOS,
            },
            &providers(),
            Platform::MacOS,
            &TargetHint::default(),
        )
    }

    fn facts(arch: &str) -> SpaceFacts {
        SpaceFacts {
            space_id: "space://direct/127.0.0.1:3211".into(),
            os: Platform::Linux,
            arch: arch.into(),
            home: "/root".into(),
            importers: vec!["firefox".into(), "claude-code".into()],
        }
    }

    #[test]
    fn app_with_files_installs_sends_and_opens_them() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("project");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(proj.join("README.md"), "hello").unwrap();
        let note = dir.path().join("note.txt");
        std::fs::write(&note, "12345").unwrap();
        let paths = vec![
            proj.to_string_lossy().into_owned(),
            note.to_string_lossy().into_owned(),
        ];
        let stats = stat_paths(&paths).unwrap();
        assert_eq!((stats[0].files, stats[0].bytes), (2, 18));
        let mut o = PlanOptions::new(MoveKind::AppWithFiles);
        o.files = paths.clone();
        let p = build(
            &entry("Visual Studio Code", "com.microsoft.VSCode"),
            &facts("aarch64"),
            &o,
            None,
            &stats,
        )
        .unwrap();
        let names: Vec<&str> = p.steps.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["install", "files", "launch"]);
        assert!(matches!(&p.steps[0], PlanStep::Install { ids } if ids == &["vscode"]));
        let PlanStep::Launch {
            bin, args, files, ..
        } = &p.steps[2]
        else {
            panic!()
        };
        assert_eq!(bin, "code");
        assert_eq!(
            files,
            &[
                "/root/Downloads/Teleported/project",
                "/root/Downloads/Teleported/note.txt"
            ]
        );
        assert!(args.ends_with(files));
        assert_eq!(args[0], "--no-sandbox");
        // Consent: the install with its pinned checksum, then each host path.
        let kinds: Vec<ConsentKind> = p.consent.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [ConsentKind::Install, ConsentKind::Folder, ConsentKind::File]
        );
        assert!(
            p.consent[0]
                .detail
                .starts_with("pinned, verified by sha256 0cfabe56")
        );
        assert_eq!(p.consent[1].key, paths[0]);
        assert_eq!(p.total_bytes, 23);
        assert!(!p.sensitive);
        assert!(
            p.clone()
                .approve(Consent {
                    approved: true,
                    acknowledge_sensitive: false,
                    ..Default::default()
                })
                .is_ok()
        );
        assert!(p.approve(Consent::default()).is_err());
    }

    fn chrome_manifest() -> TransferManifest {
        let item = |label: &str, rel: &str, sensitive: bool, checked: bool| ManifestItem {
            label: label.into(),
            rel_path: rel.into(),
            est_bytes: 10,
            sensitive,
            default_checked: checked,
            count: None,
            count_noun: None,
        };
        TransferManifest {
            provider_id: "chrome".into(),
            app_display_name: "Google Chrome".into(),
            scope: TransferScope::FullProfile,
            items: vec![
                ManifestItem {
                    count: Some(1),
                    count_noun: Some("tabs".into()),
                    ..item("Open tabs", "tabs.json", false, true)
                },
                item(
                    "Cookies",
                    ".config/google-chrome/Default/Cookies",
                    true,
                    false,
                ),
                item(
                    "Login Data",
                    ".config/google-chrome/Default/Login Data",
                    true,
                    false,
                ),
                item(
                    "History",
                    ".config/google-chrome/Default/History",
                    true,
                    false,
                ),
                item(
                    "Preferences",
                    ".config/google-chrome/Default/Preferences",
                    false,
                    true,
                ),
            ],
            total_est_bytes: 50,
            notes: vec![],
        }
    }

    #[test]
    fn sensitive_groups_opt_in_one_at_a_time_and_paths_follow_the_space_os() {
        let chrome = entry("Google Chrome", "com.google.Chrome");
        assert_eq!(chrome.provider_id.as_deref(), Some("chrome"));
        assert_eq!(
            chrome.sensitive_groups,
            [
                SensitiveGroup::SignIns,
                SensitiveGroup::Passwords,
                SensitiveGroup::History
            ]
        );
        let m = chrome_manifest();
        let mac = SpaceFacts {
            os: Platform::MacOS,
            home: "/Users/lume".into(),
            importers: vec!["chrome".into()],
            ..facts("aarch64")
        };
        let keys = |o: &PlanOptions| -> Vec<String> {
            build(&chrome, &mac, o, Some(&m), &[])
                .unwrap()
                .consent
                .iter()
                .map(|c| c.key.rsplit('/').next().unwrap().to_string())
                .collect()
        };
        // Nothing credential-shaped by default.
        let mut o = PlanOptions::new(MoveKind::AppWithState);
        assert_eq!(keys(&o), ["tabs.json", "Preferences"]);
        // "Keep me signed in" is the cookies, and only the cookies.
        o.sensitive_groups = vec![SensitiveGroup::SignIns];
        assert_eq!(keys(&o), ["tabs.json", "Cookies", "Preferences"]);
        o.sensitive_groups = vec![SensitiveGroup::Passwords, SensitiveGroup::History];
        assert_eq!(
            keys(&o),
            ["tabs.json", "Login Data", "History", "Preferences"]
        );

        o.sensitive_groups = vec![SensitiveGroup::SignIns];
        let p = build(&chrome, &mac, &o, Some(&m), &[]).unwrap();
        assert!(p.sensitive);
        let detail = |k: &str| {
            p.consent
                .iter()
                .find(|c| c.key.ends_with(k))
                .unwrap()
                .detail
                .clone()
        };
        // Where each lands in a macOS Space, not the bundle's Linux layout.
        assert_eq!(
            detail("Cookies"),
            "~/Library/Application Support/Google/Chrome/Default/Cookies"
        );
        // The tab list is bundle-internal: its count only.
        assert_eq!(detail("tabs.json"), "1 tabs");
        let linux = SpaceFacts {
            importers: vec!["chrome".into()],
            ..facts("aarch64")
        };
        let p = build(&chrome, &linux, &o, Some(&m), &[]).unwrap();
        let cookies = p
            .consent
            .iter()
            .find(|c| c.key.ends_with("Cookies"))
            .unwrap();
        assert_eq!(cookies.detail, "~/.config/google-chrome/Default/Cookies");
    }

    #[test]
    fn unpublished_architectures_and_unsupported_moves_are_refused() {
        let blender = entry("Blender", "org.blenderfoundation.blender");
        let e = build(
            &blender,
            &facts("aarch64"),
            &PlanOptions::new(MoveKind::AppOnly),
            None,
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(e, UxError::Unsupported(ref m) if m.contains("not published for aarch64")),
            "{e}"
        );
        let ok = build(
            &blender,
            &facts("x86_64"),
            &PlanOptions::new(MoveKind::AppOnly),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(ok.consent.len(), 1);
        let e = build(
            &blender,
            &facts("x86_64"),
            &PlanOptions::new(MoveKind::AppWithState),
            None,
            &[],
        )
        .unwrap_err();
        assert!(matches!(e, UxError::Invalid(_)));
        let safari = entry("Safari", "com.apple.Safari");
        assert!(matches!(
            build(
                &safari,
                &facts("x86_64"),
                &PlanOptions::new(MoveKind::AppOnly),
                None,
                &[]
            ),
            Err(UxError::Unsupported(_))
        ));
        let mut o = PlanOptions::new(MoveKind::AppWithFiles);
        assert!(build(&blender, &facts("x86_64"), &o, None, &[]).is_err());
        o.files = vec!["/nope".into()];
        assert!(build(&blender, &facts("x86_64"), &o, None, &[]).is_err());
        assert!(stat_paths(&["relative".into()]).is_err());
    }

    fn manifest() -> TransferManifest {
        TransferManifest {
            provider_id: "claude-code".into(),
            app_display_name: "Claude Code".into(),
            scope: TransferScope::FullProfile,
            items: vec![
                ManifestItem {
                    label: "Logged-in session".into(),
                    rel_path: "claude/.credentials.json".into(),
                    est_bytes: 1024,
                    sensitive: true,
                    default_checked: true,
                    count: None,
                    count_noun: None,
                },
                ManifestItem {
                    label: "Settings".into(),
                    rel_path: "claude/settings.json".into(),
                    est_bytes: 10,
                    sensitive: false,
                    default_checked: true,
                    count: None,
                    count_noun: None,
                },
                ManifestItem {
                    label: "Conversation transcripts".into(),
                    rel_path: "claude/projects/".into(),
                    est_bytes: 900,
                    sensitive: true,
                    default_checked: false,
                    count: Some(3),
                    count_noun: Some("projects".into()),
                },
            ],
            total_est_bytes: 1934,
            notes: vec!["The logged-in session carries an OAuth token.".into()],
        }
    }

    #[test]
    fn state_lists_exactly_the_selected_items_and_secrets_need_acknowledgement() {
        let cc = entry("Claude Code", "claude-code");
        let m = manifest();
        let p = build(
            &cc,
            &facts("aarch64"),
            &PlanOptions::new(MoveKind::AppWithState),
            Some(&m),
            &[],
        )
        .unwrap();
        let names: Vec<&str> = p.steps.iter().map(|s| s.name()).collect();
        // A state import relaunches the app itself: no separate launch.
        assert_eq!(names, ["install", "state"]);
        let keys: Vec<&str> = p.consent.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "node",
                "claude-code",
                "claude/.credentials.json",
                "claude/settings.json"
            ]
        );
        assert_eq!(p.consent[2].kind, ConsentKind::Secret);
        assert!(p.sensitive);
        assert_eq!(p.total_bytes, 1034);
        let e = p
            .clone()
            .approve(Consent {
                approved: true,
                acknowledge_sensitive: false,
                ..Default::default()
            })
            .unwrap_err();
        assert!(e.to_string().contains("Logged-in session"));
        assert!(
            p.approve(Consent {
                approved: true,
                acknowledge_sensitive: true,
                ..Default::default()
            })
            .is_ok()
        );

        // A group the provider does not offer selects nothing more.
        let mut o = PlanOptions::new(MoveKind::AppWithState);
        o.sensitive_groups = vec![SensitiveGroup::SignIns, SensitiveGroup::History];
        let more = build(&cc, &facts("aarch64"), &o, Some(&m), &[]).unwrap();
        assert_eq!(more.consent.len(), 4);

        let mut o = PlanOptions::new(MoveKind::AppWithState);
        o.state_items = Some(vec!["claude/projects/".into()]);
        let p = build(&cc, &facts("aarch64"), &o, Some(&m), &[]).unwrap();
        assert_eq!(
            p.consent.last().unwrap().detail,
            "3 projects, ~/.claude/projects/"
        );
        o.state_items = Some(vec!["../etc/passwd".into()]);
        assert!(build(&cc, &facts("aarch64"), &o, Some(&m), &[]).is_err());
        o.state_items = Some(vec![]);
        assert!(build(&cc, &facts("aarch64"), &o, Some(&m), &[]).is_err());
        let mut no_importer = facts("aarch64");
        no_importer.importers.clear();
        assert!(matches!(
            build(
                &cc,
                &no_importer,
                &PlanOptions::new(MoveKind::AppWithState),
                Some(&m),
                &[]
            ),
            Err(UxError::Unsupported(_))
        ));
    }

    /// `save_to_keyvault` reaches the approved plan only when something
    /// sensitive actually moved; a caller cannot set it on a plan with
    /// nothing a Keyvault would seal (regression for the review sheet's
    /// "Save to Keyvault" checkbox, gap 6).
    #[test]
    fn save_to_keyvault_only_sticks_to_a_sensitive_approved_plan() {
        let cc = entry("Claude Code", "claude-code");
        let m = manifest();
        let sensitive_plan = build(
            &cc,
            &facts("aarch64"),
            &PlanOptions::new(MoveKind::AppWithState),
            Some(&m),
            &[],
        )
        .unwrap();
        assert!(sensitive_plan.sensitive);
        assert!(!sensitive_plan.save_to_keyvault);
        let approved = sensitive_plan
            .approve(Consent {
                approved: true,
                acknowledge_sensitive: true,
                save_to_keyvault: true,
                acknowledge_relay_plaintext: false,
                ..Default::default()
            })
            .unwrap();
        assert!(approved.plan().save_to_keyvault);

        let app_only_plan = build(
            &cc,
            &facts("aarch64"),
            &PlanOptions::new(MoveKind::AppOnly),
            Some(&m),
            &[],
        )
        .unwrap();
        assert!(!app_only_plan.sensitive);
        let approved = app_only_plan
            .approve(Consent {
                approved: true,
                acknowledge_sensitive: false,
                save_to_keyvault: true,
                acknowledge_relay_plaintext: false,
                ..Default::default()
            })
            .unwrap();
        assert!(!approved.plan().save_to_keyvault);
    }

    /// The review's choices shape what is approved: excluded items leave the
    /// plan (and its totals), an empty domain choice drops the cookie store,
    /// and the domain list and the vault source ride on the approved plan.
    #[test]
    fn review_choices_shape_the_approved_plan() {
        let cc = entry("Google Chrome", "com.google.Chrome");
        let m = chrome_manifest();
        let mac = SpaceFacts {
            os: Platform::MacOS,
            home: "/Users/lume".into(),
            importers: vec!["chrome".into()],
            ..facts("aarch64")
        };
        let plan = |opts: &PlanOptions| build(&cc, &mac, opts, Some(&m), &[]);
        let mut o = PlanOptions::new(MoveKind::AppWithState);
        o.sensitive_groups = vec![SensitiveGroup::SignIns];
        let p = plan(&o).unwrap();
        let keys: Vec<String> = p.consent.iter().map(|c| c.key.clone()).collect();
        let cookie_key = keys.iter().find(|k| k.ends_with("Cookies")).cloned();
        let ok = Consent {
            approved: true,
            acknowledge_sensitive: true,
            ..Default::default()
        };
        // Excluding an item drops it everywhere, and the totals follow.
        let victim = keys.last().unwrap().clone();
        let before = p.total_bytes;
        let a = p
            .clone()
            .approve(Consent {
                exclude: vec![victim.clone()],
                ..ok.clone()
            })
            .unwrap();
        assert!(!a.plan().consent.iter().any(|c| c.key == victim));
        assert!(a.plan().total_bytes < before);
        for step in &a.plan().steps {
            if let PlanStep::ImportState { items, .. } = step {
                assert!(!items.contains(&victim));
            }
        }
        // Domains ride along; an empty choice drops the cookie store itself.
        let a = p
            .clone()
            .approve(Consent {
                cookie_domains: Some(vec!["github.com".into()]),
                ..ok.clone()
            })
            .unwrap();
        assert_eq!(
            a.plan().cookie_domains.as_deref(),
            Some(&["github.com".to_string()][..])
        );
        if let Some(ck) = cookie_key {
            let none = p
                .clone()
                .approve(Consent {
                    cookie_domains: Some(vec![]),
                    ..ok.clone()
                })
                .unwrap();
            assert!(!none.plan().consent.iter().any(|c| c.key == ck));
            assert!(
                a.plan().consent.iter().any(|c| c.key == ck),
                "a chosen domain keeps the store"
            );
        }
        // The vault source is carried, never invented.
        let v = p
            .clone()
            .approve(Consent {
                from_vault: Some(vec!["i1".into()]),
                ..ok.clone()
            })
            .unwrap();
        assert_eq!(
            v.plan().from_vault.as_deref(),
            Some(&["i1".to_string()][..])
        );
        assert!(p.approve(ok).unwrap().plan().from_vault.is_none());
    }
}
