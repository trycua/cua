// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Teleport an app…": every host app, classified by what teleport can do
//! with it in a Space.
//!
//! Classification reads two tables and nothing else:
//!
//! - the pinned, checksum-verified install manifest
//!   (`cua_agents::installables`): can the app be installed in the Space?
//! - the export provider registry ([`crate::ExportRegistry`]): can its
//!   signed-in state move?
//!
//! plus [`targets.json`](TARGETS), which ties host identifiers to both and
//! says how to launch the app in the Space.
//!
//! | Level | Meaning | Moves |
//! |---|---|---|
//! | [`Capability::Full`] | a provider imports its state | app, app + files (when it opens files), app + signed-in state |
//! | [`Capability::InstallOnly`] | installed in the Space, opens empty or with files | app, app + files |
//! | [`Capability::Unsupported`] | shown disabled, with [`CatalogEntry::reason`] | nothing |

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::apps::HostApp;
use crate::Platform;
use crate::layout::{chrome, firefox};
use crate::registry::ProviderInfo;

/// The target table, as shipped.
pub const TARGETS: &str = include_str!("targets.json");

/// How to start an app in the Space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    /// Binary: linked into `~/.cua/bin` by the installer, or on `PATH`.
    pub bin: String,
    /// Arguments before any files (trusted, from this table; `$HOME`
    /// expands in the Space).
    #[serde(default)]
    pub args: Vec<String>,
    /// A terminal program (started in a terminal window).
    #[serde(default)]
    pub terminal: bool,
}

/// One row of [`TARGETS`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Display name.
    pub name: String,
    /// Host identifiers: bundle ids and desktop ids match case-insensitively,
    /// display names exactly.
    #[serde(rename = "match")]
    pub matches: Vec<String>,
    /// Installable id in the install manifest.
    #[serde(default)]
    pub install: Option<String>,
    /// cua's Linux Space images ship the app.
    #[serde(default)]
    pub image: bool,
    /// Teleport provider id (state import).
    #[serde(default)]
    pub provider: Option<String>,
    /// How to start it in the Space.
    #[serde(default)]
    pub launch: Option<Launch>,
    /// It takes files or folders as arguments.
    #[serde(default)]
    pub opens_files: bool,
}

#[derive(Deserialize)]
struct TargetFile {
    targets: BTreeMap<String, Target>,
}

/// Every target, by id.
pub fn targets() -> &'static BTreeMap<String, Target> {
    static T: OnceLock<BTreeMap<String, Target>> = OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str::<TargetFile>(TARGETS)
            .expect("targets.json is valid (checked by tests)")
            .targets
    })
}

/// What teleport can do with an app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// State import is supported (a provider), on top of installing.
    Full,
    /// The app is installed in the Space and opens empty or with chosen files.
    InstallOnly,
    /// Shown disabled, with a reason.
    Unsupported,
}

impl Capability {
    /// Wire spelling (`full`, `install_only`, `unsupported`).
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Full => "full",
            Capability::InstallOnly => "install_only",
            Capability::Unsupported => "unsupported",
        }
    }
}

/// What a teleport moves. Every plan picks exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveKind {
    /// The app only: installed (or already there) and launched, empty.
    AppOnly,
    /// The app plus the chosen files or folders, opened in it.
    AppWithFiles,
    /// The app plus its signed-in state, through a provider (consent lists
    /// every path and secret).
    AppWithState,
}

impl MoveKind {
    /// Wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            MoveKind::AppOnly => "app_only",
            MoveKind::AppWithFiles => "app_with_files",
            MoveKind::AppWithState => "app_with_state",
        }
    }

    /// Parses the wire spelling.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "app_only" | "app" => Some(MoveKind::AppOnly),
            "app_with_files" | "files" => Some(MoveKind::AppWithFiles),
            "app_with_state" | "state" => Some(MoveKind::AppWithState),
            _ => None,
        }
    }
}

/// Credential-shaped state a provider leaves out of the signed-in state move
/// by default, which a person opts into one group at a time. Each item of a
/// group is a secret in the plan's consent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveGroup {
    /// The session cookies: what keeps the app signed in.
    SignIns,
    /// Saved passwords.
    Passwords,
    /// Browsing history.
    History,
}

impl SensitiveGroup {
    /// Wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SensitiveGroup::SignIns => "sign_ins",
            SensitiveGroup::Passwords => "passwords",
            SensitiveGroup::History => "history",
        }
    }

    /// Every group, in UI order.
    pub const ALL: [SensitiveGroup; 3] = [
        SensitiveGroup::SignIns,
        SensitiveGroup::Passwords,
        SensitiveGroup::History,
    ];

    /// The groups `provider_id` offers, in UI order: those with an item in
    /// [`OPT_IN_ITEMS`]. A provider not listed offers none.
    pub fn offered_by(provider_id: &str) -> Vec<SensitiveGroup> {
        Self::ALL
            .into_iter()
            .filter(|g| {
                OPT_IN_ITEMS
                    .iter()
                    .any(|(p, _, group)| *p == provider_id && group == g)
            })
            .collect()
    }

    /// The group of `provider_id`'s manifest item `rel_path` (matched on its
    /// file name), if any.
    pub fn of_item(provider_id: &str, rel_path: &str) -> Option<SensitiveGroup> {
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        OPT_IN_ITEMS
            .iter()
            .find(|(p, file, _)| *p == provider_id && *file == name)
            .map(|(_, _, g)| *g)
    }
}

/// The one list of opt-in items: `(provider id, profile file name, group)`.
/// Each is sensitive and left out by default; the picker offers its group
/// as a checkbox, and each provider's tests assert its manifest matches
/// this list exactly ([`crate::ux::testing::assert_opt_ins_match`]).
///
/// Browsers are listed: their cookies sign in to every site, so even "Keep
/// me signed in" is a choice, and saved passwords and history are separate
/// ones. The single-app providers (Slack, Discord, Unity Hub, Steam,
/// WhatsApp, Claude Code) are not: their session *is* the signed-in state
/// move, and they have no saved passwords or browsing history.
pub const OPT_IN_ITEMS: &[(&str, &str, SensitiveGroup)] = &[
    (chrome::ID, "Cookies", SensitiveGroup::SignIns),
    (chrome::ID, "Login Data", SensitiveGroup::Passwords),
    (chrome::ID, "History", SensitiveGroup::History),
    (firefox::ID, "cookies.sqlite", SensitiveGroup::SignIns),
    (firefox::ID, "logins.json", SensitiveGroup::Passwords),
    (firefox::ID, "key4.db", SensitiveGroup::Passwords),
    (firefox::ID, "places.sqlite", SensitiveGroup::History),
];

/// Where the app comes from in the Space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallSource {
    /// A pinned, checksum-verified item of the install manifest.
    Manifest {
        /// Installable id.
        id: String,
        /// Pinned version.
        version: String,
        /// Its license, for the record.
        license: String,
        /// CPUs it is published for (`aarch64`, `x86_64`).
        arches: Vec<String>,
    },
    /// cua's Linux Space images ship it.
    Image,
    /// Nothing installs it: the Space must already have it.
    Space,
}

/// One row of "Teleport an app…".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Stable id: the target id, else the provider id, else the host app id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The host app (bundle, `.desktop`, shortcut), when one was found.
    pub host_path: Option<String>,
    /// The host app id (bundle id, desktop id).
    pub host_app_id: Option<String>,
    /// Host app version.
    pub version: Option<String>,
    /// Host icon reference (see [`HostApp::icon`]).
    pub icon: Option<String>,
    /// What teleport can do.
    pub capability: Capability,
    /// Why it is unsupported, or a caveat for a supported app.
    pub reason: Option<String>,
    /// The moves offered, in UI order. Empty when unsupported.
    pub moves: Vec<MoveKind>,
    /// Provider id, when state can move.
    pub provider_id: Option<String>,
    /// The sensitive groups the signed-in state move offers as opt-ins
    /// (see [`SensitiveGroup::offered_by`]). Empty without that move.
    #[serde(default)]
    pub sensitive_groups: Vec<SensitiveGroup>,
    /// Where the app comes from in the Space.
    pub install: Option<InstallSource>,
    /// How it starts in the Space.
    pub launch: Option<Launch>,
    /// Last teleported (Unix ms), from the recents file.
    pub last_used_ms: Option<u64>,
}

impl CatalogEntry {
    /// Whether the picker enables it.
    pub fn is_enabled(&self) -> bool {
        self.capability != Capability::Unsupported
    }

    /// Whether `m` is offered.
    pub fn offers(&self, m: MoveKind) -> bool {
        self.moves.contains(&m)
    }
}

/// What the Space looks like, when the caller knows (the catalog narrows
/// to it; `None` fields mean "any").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetHint {
    /// The Space's OS. Install-manifest items are Linux builds.
    pub os: Option<Platform>,
    /// The Space's CPU family (`aarch64`, `x86_64`).
    pub arch: Option<String>,
}

/// One export provider, as the catalog sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderView {
    /// Static description.
    pub info: ProviderInfo,
    /// Whether its app looks installed on this host (for provider apps that
    /// are not bundles, such as the Claude Code CLI).
    pub installed_here: bool,
}

/// The catalog: `apps` (from [`super::apps::enumerate`]) classified against
/// `providers` and the install manifest, plus provider apps that are not
/// bundles but look installed. Sorted: recents first, then enabled before
/// disabled, then by name.
pub fn classify_all(
    apps: &[HostApp],
    providers: &[ProviderView],
    host: Platform,
    hint: &TargetHint,
    recents: &BTreeMap<String, u64>,
) -> Vec<CatalogEntry> {
    let mut out: Vec<CatalogEntry> = apps
        .iter()
        .map(|a| classify(a, providers, host, hint))
        .collect();
    // Provider apps with no bundle (CLIs): offered when installed here.
    for p in providers {
        if !p.installed_here
            || out
                .iter()
                .any(|e| e.provider_id.as_deref() == Some(&p.info.id))
        {
            continue;
        }
        let is_cli = p
            .info
            .install_probe
            .as_ref()
            .is_some_and(|probe| probe.on_path);
        if !is_cli {
            continue;
        }
        let app = HostApp {
            name: p.info.display_name.clone(),
            app_id: Some(p.info.id.clone()),
            path: Default::default(),
            version: None,
            icon: None,
            platform: host,
        };
        let mut e = classify(&app, providers, host, hint);
        e.host_path = None;
        out.push(e);
    }
    // One row per id (two bundles of one app collapse to the first).
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e.id.clone()));
    for e in &mut out {
        e.last_used_ms = recents.get(&e.id).copied();
    }
    sort(&mut out);
    out
}

/// Recents first (newest first), then enabled before disabled, then Full
/// before InstallOnly, then by name.
pub fn sort(entries: &mut [CatalogEntry]) {
    entries.sort_by(|a, b| {
        b.last_used_ms
            .is_some()
            .cmp(&a.last_used_ms.is_some())
            .then(b.last_used_ms.cmp(&a.last_used_ms))
            .then(a.capability.cmp(&b.capability))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn eq_id(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether `app` is the thing `aliases` name: its app id matches any alias
/// case-insensitively, or its display name matches one exactly (so the
/// "Claude" desktop app is not the `claude` CLI).
fn app_matches(app: &HostApp, aliases: &[String]) -> bool {
    aliases
        .iter()
        .any(|alias| app.app_id.as_deref().is_some_and(|id| eq_id(id, alias)) || app.name == *alias)
}

/// Classifies one host app.
pub fn classify(
    app: &HostApp,
    providers: &[ProviderView],
    host: Platform,
    hint: &TargetHint,
) -> CatalogEntry {
    let target = targets().iter().find(|(_, t)| app_matches(app, &t.matches));
    let provider = match target.and_then(|(_, t)| t.provider.as_deref()) {
        Some(pid) => providers.iter().find(|p| p.info.id == pid),
        None => providers.iter().find(|p| {
            let mut aliases = p.info.app_ids.clone();
            aliases.push(p.info.id.clone());
            app_matches(app, &aliases)
        }),
    };
    let id = target
        .map(|(id, _)| id.clone())
        .or_else(|| provider.map(|p| p.info.id.clone()))
        .or_else(|| app.app_id.clone())
        .unwrap_or_else(|| slug(&app.name));
    let name = target
        .filter(|_| app.name.is_empty())
        .map(|(_, t)| t.name.clone())
        .unwrap_or_else(|| app.name.clone());

    let space_os = hint.os.unwrap_or(Platform::Linux);
    let install = target.and_then(|(_, t)| install_source(t));
    // Install-manifest builds and image apps are Linux Space facts.
    let install_here = match (&install, space_os) {
        (Some(InstallSource::Manifest { arches, .. }), Platform::Linux) => match &hint.arch {
            Some(arch) if !arches.iter().any(|a| a == arch) => Err(format!(
                "{} is not published for {arch} Linux in the install manifest",
                target.map(|(_, t)| t.name.as_str()).unwrap_or(&name)
            )),
            _ => Ok(true),
        },
        (Some(InstallSource::Image), Platform::Linux) => Ok(true),
        // cua's other images do not promise the app: the Space must have it.
        (Some(InstallSource::Image), _) => Ok(false),
        (Some(_), os) => Err(format!(
            "the install manifest covers Linux Spaces; this Space runs {}",
            os_name(os)
        )),
        (None, _) => Ok(false),
    };
    let provider_ok = provider.filter(|p| supported_on(&p.info, host));
    let opens_files = target.is_some_and(|(_, t)| t.opens_files);
    let launch = target.and_then(|(_, t)| t.launch.clone());

    let mut entry = CatalogEntry {
        id,
        name,
        host_path: (!app.path.as_os_str().is_empty())
            .then(|| app.path.to_string_lossy().into_owned()),
        host_app_id: app.app_id.clone(),
        version: app.version.clone(),
        icon: app.icon.clone(),
        capability: Capability::Unsupported,
        reason: None,
        moves: vec![],
        provider_id: provider_ok.map(|p| p.info.id.clone()),
        sensitive_groups: vec![],
        install: install.clone(),
        launch,
        last_used_ms: None,
    };

    match (provider_ok, &install_here) {
        (Some(_), Ok(installable)) => {
            entry.capability = Capability::Full;
            if *installable {
                entry.moves.push(MoveKind::AppOnly);
                if opens_files {
                    entry.moves.push(MoveKind::AppWithFiles);
                }
            } else {
                entry.install = Some(InstallSource::Space);
                entry.reason = Some(match space_os {
                    Platform::Linux => format!(
                        "{} must already be in the Space: the install manifest has no Linux build of it",
                        entry.name
                    ),
                    // cua installs apps in Linux Spaces only: a macOS or
                    // Windows Space brings its own.
                    os => format!(
                        "{} must already be in the Space: cua installs apps only in Linux Spaces, and this one runs {}",
                        entry.name,
                        os_name(os)
                    ),
                });
            }
            entry.moves.push(MoveKind::AppWithState);
            entry.sensitive_groups = entry
                .provider_id
                .as_deref()
                .map(SensitiveGroup::offered_by)
                .unwrap_or_default();
        }
        (Some(_), Err(why)) => {
            // State could move, but the app cannot be brought up there.
            entry.reason = Some(why.clone());
            entry.provider_id = None;
        }
        (None, Ok(true)) => {
            entry.capability = Capability::InstallOnly;
            entry.moves.push(MoveKind::AppOnly);
            if opens_files {
                entry.moves.push(MoveKind::AppWithFiles);
            }
        }
        (None, Err(why)) => entry.reason = Some(why.clone()),
        (None, Ok(false)) => {
            entry.reason = Some(match provider {
                Some(p) => format!(
                    "the {} provider does not export on {}",
                    p.info.display_name,
                    os_name(host)
                ),
                None => {
                    "no Linux build in the install manifest and no teleport provider for its state"
                        .into()
                }
            });
        }
    }
    entry
}

fn install_source(t: &Target) -> Option<InstallSource> {
    if let Some(id) = &t.install
        && let Some(it) = cua_agents::installables::get(id)
    {
        return Some(InstallSource::Manifest {
            id: id.clone(),
            version: it.version.clone(),
            license: it.license.clone(),
            arches: it.arches().into_iter().map(str::to_string).collect(),
        });
    }
    t.image.then_some(InstallSource::Image)
}

fn supported_on(p: &ProviderInfo, host: Platform) -> bool {
    match host {
        Platform::MacOS => p.macos,
        Platform::Linux => p.linux,
        Platform::Windows => p.windows,
    }
}

fn os_name(p: Platform) -> &'static str {
    match p {
        Platform::MacOS => "macOS",
        Platform::Linux => "Linux",
        Platform::Windows => "Windows",
    }
}

/// Lowercase slug ("Visual Studio Code" -> "visual-studio-code").
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Entries matching `query` (case-insensitive substring of the name, id or
/// host app id; every whitespace-separated word must match). A blank query
/// keeps everything. Order is preserved.
pub fn search<'a>(entries: &'a [CatalogEntry], query: &str) -> Vec<&'a CatalogEntry> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    entries
        .iter()
        .filter(|e| {
            let hay = format!(
                "{} {} {}",
                e.name.to_lowercase(),
                e.id.to_lowercase(),
                e.host_app_id.as_deref().unwrap_or("").to_lowercase()
            );
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ux::testing::{fixture_app, providers};

    fn app(name: &str, id: &str) -> HostApp {
        HostApp {
            name: name.into(),
            app_id: Some(id.into()),
            path: format!("/fixture/{name}.app").into(),
            version: None,
            icon: None,
            platform: Platform::MacOS,
        }
    }

    #[test]
    fn the_target_table_parses_and_names_real_installables_and_providers() {
        let ids = crate::layout::PROVIDER_IDS;
        for (id, t) in targets() {
            if let Some(i) = &t.install {
                assert!(cua_agents::installables::get(i).is_some(), "{id}: {i}");
            }
            if let Some(p) = &t.provider {
                assert!(ids.contains(&p.as_str()), "{id}: {p}");
            }
            assert!(
                t.install.is_some() || t.image || t.provider.is_some(),
                "{id}"
            );
            assert!(!t.matches.is_empty(), "{id}");
        }
    }

    #[test]
    fn levels_follow_the_manifest_and_the_providers() {
        let p = providers();
        let hint = TargetHint::default();
        let vs = classify(
            &app("Visual Studio Code", "com.microsoft.VSCode"),
            &p,
            Platform::MacOS,
            &hint,
        );
        assert_eq!(vs.id, "vscode");
        assert_eq!(vs.capability, Capability::InstallOnly);
        assert_eq!(vs.moves, [MoveKind::AppOnly, MoveKind::AppWithFiles]);
        assert!(
            matches!(vs.install, Some(InstallSource::Manifest { ref id, .. }) if id == "vscode")
        );
        assert_eq!(vs.launch.as_ref().unwrap().bin, "code");

        let ff = classify(
            &app("Firefox", "org.mozilla.firefox"),
            &p,
            Platform::MacOS,
            &hint,
        );
        assert_eq!(ff.capability, Capability::Full);
        assert_eq!(ff.provider_id.as_deref(), Some("firefox"));
        assert_eq!(
            ff.moves,
            [
                MoveKind::AppOnly,
                MoveKind::AppWithFiles,
                MoveKind::AppWithState
            ]
        );
        assert_eq!(ff.install, Some(InstallSource::Image));

        // A provider app nothing installs: state only, with the caveat.
        let chrome = classify(
            &app("Google Chrome", "com.google.Chrome"),
            &p,
            Platform::MacOS,
            &hint,
        );
        assert_eq!(chrome.capability, Capability::Full);
        assert_eq!(chrome.moves, [MoveKind::AppWithState]);
        assert_eq!(chrome.install, Some(InstallSource::Space));
        assert!(
            chrome
                .reason
                .as_deref()
                .unwrap()
                .contains("must already be in the Space")
        );

        // In a macOS Space the caveat names the Space's OS, not Linux.
        let on_mac = classify(
            &app("Google Chrome", "com.google.Chrome"),
            &p,
            Platform::MacOS,
            &TargetHint {
                os: Some(Platform::MacOS),
                arch: Some("aarch64".into()),
            },
        );
        assert_eq!(
            on_mac.reason.as_deref(),
            Some(
                "Google Chrome must already be in the Space: cua installs apps only in Linux \
                 Spaces, and this one runs macOS"
            )
        );

        // A provider app with no target row (Slack): Full through the provider.
        let slack = classify(
            &app("Slack", "com.tinyspeck.slackmacgap"),
            &p,
            Platform::MacOS,
            &hint,
        );
        assert_eq!(slack.id, "slack");
        assert_eq!(slack.capability, Capability::Full);
        assert_eq!(slack.moves, [MoveKind::AppWithState]);

        let safari = classify(
            &app("Safari", "com.apple.Safari"),
            &p,
            Platform::MacOS,
            &hint,
        );
        assert_eq!(safari.capability, Capability::Unsupported);
        assert!(safari.moves.is_empty());
        assert!(
            safari
                .reason
                .as_deref()
                .unwrap()
                .contains("install manifest")
        );
        assert_eq!(safari.id, "com.apple.Safari");
    }

    #[test]
    fn the_claude_desktop_app_is_not_the_claude_cli() {
        let p = providers();
        let e = classify(
            &app("Claude", "com.anthropic.claudefordesktop"),
            &p,
            Platform::MacOS,
            &TargetHint::default(),
        );
        assert_eq!(e.capability, Capability::Unsupported);
        assert!(e.provider_id.is_none());
    }

    #[test]
    fn the_space_architecture_and_os_narrow_the_catalog() {
        let p = providers();
        let blender = app("Blender", "org.blenderfoundation.blender");
        let arm = TargetHint {
            os: Some(Platform::Linux),
            arch: Some("aarch64".into()),
        };
        let e = classify(&blender, &p, Platform::MacOS, &arm);
        assert_eq!(e.capability, Capability::Unsupported);
        assert_eq!(
            e.reason.as_deref(),
            Some("Blender is not published for aarch64 Linux in the install manifest")
        );
        let x86 = TargetHint {
            arch: Some("x86_64".into()),
            ..arm.clone()
        };
        assert_eq!(
            classify(&blender, &p, Platform::MacOS, &x86).capability,
            Capability::InstallOnly
        );
        let mac = TargetHint {
            os: Some(Platform::MacOS),
            arch: None,
        };
        let e = classify(
            &app("Visual Studio Code", "com.microsoft.VSCode"),
            &p,
            Platform::MacOS,
            &mac,
        );
        assert_eq!(e.capability, Capability::Unsupported);
        assert!(e.reason.unwrap().contains("covers Linux Spaces"));
    }

    #[test]
    fn classify_all_adds_installed_cli_providers_and_sorts_recents_first() {
        let root = tempfile::tempdir().unwrap();
        fixture_app(
            root.path(),
            "Visual Studio Code",
            "com.microsoft.VSCode",
            "Code",
            false,
        );
        fixture_app(root.path(), "Safari", "com.apple.Safari", "Safari", false);
        fixture_app(
            root.path(),
            "Firefox",
            "org.mozilla.firefox",
            "Firefox",
            false,
        );
        let apps = super::super::apps::enumerate(&[root.path().to_path_buf()], Platform::MacOS);
        let mut p = providers();
        for v in &mut p {
            v.installed_here = v.info.id == "claude-code";
        }
        let mut recents = BTreeMap::new();
        recents.insert("vscode".to_string(), 1_000u64);
        let all = classify_all(&apps, &p, Platform::MacOS, &TargetHint::default(), &recents);
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            ["vscode", "claude-code", "firefox", "com.apple.Safari"]
        );
        assert_eq!(all[0].last_used_ms, Some(1_000));
        let claude = &all[1];
        assert_eq!(claude.capability, Capability::Full);
        assert!(claude.host_path.is_none());
        assert!(
            matches!(claude.install, Some(InstallSource::Manifest { ref id, .. }) if id == "claude-code")
        );

        let hits: Vec<&str> = search(&all, "studio")
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(hits, ["vscode"]);
        assert_eq!(search(&all, "  ").len(), 4);
        assert_eq!(search(&all, "mozilla fire").len(), 1);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Visual Studio Code"), "visual-studio-code");
        assert_eq!(slug(" A--B "), "a-b");
    }
}
