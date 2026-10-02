// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-app layout descriptors, shared by the sender and the receiver.
//!
//! For each built-in app this module says, as pure data:
//!
//! - which files and directories of the app's profile make up its session
//!   (the sender reads these from the source machine);
//! - the canonical bundle path each one travels under;
//! - where each bundle path lands on a destination platform, relative to the
//!   destination home (the receiver writes there), and how the app is
//!   relaunched ([`LaunchSpec`]).
//!
//! Every function here is pure: it maps names and platforms to relative paths
//! and never touches a filesystem. Callers join the results onto a home
//! directory they resolved themselves.

use crate::types::{LaunchSpec, Platform, WindowRestore};

/// Every built-in provider id, in registry order. The receiver can import
/// exactly these; the sender can export exactly these.
pub const PROVIDER_IDS: &[&str] = &[
    chrome::ID,
    firefox::ID,
    electron::SLACK.id,
    electron::DISCORD.id,
    electron::UNITY_HUB.id,
    steam::ID,
    whatsapp::ID,
    claude_code::ID,
];

/// A launch spec with no windows to restore.
fn plain_launch(program: impl Into<String>, args: Vec<String>) -> LaunchSpec {
    LaunchSpec {
        program: program.into(),
        args,
        env: Vec::new(),
        cwd: None,
        restore_windows: vec![WindowRestore {
            title: None,
            urls: Vec::new(),
        }],
    }
}

/// Google Chrome / Chromium.
///
/// The bundle always uses the canonical (Linux) user-data layout
/// (`.config/google-chrome/<profile>/…`); the receiver remaps it onto the
/// destination platform's real user-data dir, so a macOS guest receives the
/// profile where macOS Chrome reads it.
pub mod chrome {
    use super::Platform;

    /// Provider id.
    pub const ID: &str = "chrome";
    /// Display name.
    pub const DISPLAY: &str = "Google Chrome / Chromium";

    /// Application identifiers the sender matches.
    pub const APP_IDS: &[&str] = &[
        "com.google.Chrome",
        "com.google.Chrome.canary",
        "chromium",
        "org.chromium.Chromium",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        // macOS app / window-owner names. A consent UI resolving a dragged window
        // sees `kCGWindowOwnerName` ("Google Chrome"), not a bundle id, so without
        // these an obviously-teleportable window reads as unsupported.
        "Google Chrome",
        "Google Chrome Canary",
    ];

    /// Curated files copied for a full-profile transfer, with sensitivity flags.
    /// Caches, GPU, and code-cache directories are intentionally excluded.
    pub const PROFILE_FILES: &[(&str, bool)] = &[
        ("Cookies", true),
        ("Login Data", true),
        ("History", true),
        ("Preferences", false),
        ("Bookmarks", false),
        ("Web Data", false),
    ];

    /// Curated directories copied recursively for a full-profile transfer.
    ///
    /// `Sessions/` holds modern Chrome's SNSS session store (`Session_*`/`Tabs_*`),
    /// which is what `--restore-last-session` reads on the destination; older Chrome
    /// used the top-level [`SESSION_FILES`], so both are carried.
    pub const PROFILE_DIRS: &[(&str, bool)] = &[
        ("Local Storage", false),
        ("Sessions", false),
        ("Session Storage", false),
    ];

    /// Legacy top-level session state files (best-effort raw copy; SNSS binary
    /// format). Modern Chrome keeps these inside the `Sessions/` directory instead.
    pub const SESSION_FILES: &[&str] = &[
        "Current Session",
        "Current Tabs",
        "Last Session",
        "Last Tabs",
    ];

    /// Bundle path of the normalized tab list.
    pub const TABS_JSON: &str = "tabs.json";

    /// Canonical (Linux) user-data dir, relative to the home.
    pub const LINUX_USER_DATA_DIR: &str = ".config/google-chrome";

    /// Chrome's user-data dir on `platform`, relative to that platform's home.
    /// This is where a normal (no `--user-data-dir`) Chrome launch reads its
    /// profile, so materializing here means a plain relaunch picks up the
    /// transferred session. The sender resolves its source profile under it.
    pub fn user_data_dir_for(platform: Platform) -> &'static str {
        match platform {
            Platform::MacOS => "Library/Application Support/Google/Chrome",
            Platform::Linux => LINUX_USER_DATA_DIR,
            Platform::Windows => "AppData/Local/Google/Chrome/User Data",
        }
    }

    /// The cookie store of a Chromium-family profile directory. Chrome 96
    /// moved it to `Network/Cookies`; current Chrome (154) reads `Cookies` in
    /// the profile root again, so neither location is privileged. When both
    /// exist (a profile that crossed versions) the one written most recently
    /// wins, counting its `-wal` file, since a live Chrome's latest rows sit
    /// there; a tie goes to the root file. With one, that one; with neither,
    /// the root path (what current Chrome would create). Applies to every
    /// profile and every Chromium-family browser.
    pub fn cookies_store(profile_dir: &std::path::Path) -> std::path::PathBuf {
        let network = profile_dir.join("Network").join("Cookies");
        let root = profile_dir.join("Cookies");
        match (network.is_file(), root.is_file()) {
            (true, true) => {
                if last_written(&network) > last_written(&root) {
                    network
                } else {
                    root
                }
            }
            (true, false) => network,
            _ => root,
        }
    }

    /// Newest mtime of a SQLite database and its `-wal` sidecar.
    fn last_written(db: &std::path::Path) -> Option<std::time::SystemTime> {
        let mut wal = db.as_os_str().to_owned();
        wal.push("-wal");
        [db.to_path_buf(), std::path::PathBuf::from(wal)]
            .iter()
            .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok())
            .max()
    }

    /// Bundle path of a profile-relative file (canonical Linux layout).
    pub fn dest_rel_path(profile: &str, rel: &str) -> String {
        format!("{LINUX_USER_DATA_DIR}/{profile}/{rel}")
    }

    /// Remap a bundle path (canonical Linux layout) to a destination-home-relative
    /// path under `platform`'s user-data dir. Paths outside the canonical prefix
    /// pass through unchanged so nothing is silently dropped.
    pub fn remap_to_platform(rel_path: &str, platform: Platform) -> String {
        match rel_path.strip_prefix(&format!("{LINUX_USER_DATA_DIR}/")) {
            Some(within) => format!("{}/{}", user_data_dir_for(platform), within),
            None => rel_path.to_string(),
        }
    }

    /// The Chrome program on `platform`. macOS goes through the app bundle's
    /// inner binary, which accepts `--user-data-dir` and URL arguments; Linux
    /// resolves `google-chrome` on `PATH`.
    pub fn launch_program_for(platform: Platform) -> String {
        match platform {
            Platform::MacOS => {
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()
            }
            Platform::Linux => "google-chrome".to_string(),
            Platform::Windows => {
                r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string()
            }
        }
    }

    /// Where the app is installed on a macOS source (install probe).
    pub const MACOS_APP: &str = "/Applications/Google Chrome.app";
}

/// Firefox and Firefox-family browsers.
pub mod firefox {
    use super::Platform;

    /// Provider id.
    pub const ID: &str = "firefox";
    /// Display name.
    pub const DISPLAY: &str = "Firefox";

    /// Application identifiers the sender matches (plus anything containing
    /// "firefox").
    pub const APP_IDS: &[&str] = &[
        "org.mozilla.firefox",
        "org.mozilla.firefoxdeveloperedition",
        "firefox",
        "firefox-esr",
        "Firefox",
    ];

    /// Canonical bundle prefix for profile contents.
    pub const BUNDLE_PREFIX: &str = "firefox";

    /// The destination profile directory name, under `<root>/Profiles/`.
    pub const DEST_PROFILE: &str = "cua.default-release";

    /// Curated profile files, with sensitivity flags. Caches are excluded.
    pub const PROFILE_FILES: &[(&str, bool)] = &[
        ("cookies.sqlite", true),
        ("key4.db", true),
        ("logins.json", true),
        ("cert9.db", false),
        // History (with the bookmarks Firefox keeps in the same database):
        // private, and an opt-in like Chrome's History.
        ("places.sqlite", true),
        ("favicons.sqlite", false),
        ("permissions.sqlite", false),
        ("webappsstore.sqlite", false),
        ("prefs.js", false),
        ("sessionstore.jsonlz4", false),
        ("extensions.json", false),
        ("addonStartup.json.lz4", false),
        ("extension-preferences.json", false),
        ("handlers.json", false),
        ("search.json.mozlz4", false),
        ("containers.json", false),
        ("xulstore.json", false),
    ];

    /// Curated profile directories copied recursively.
    pub const PROFILE_DIRS: &[(&str, bool)] = &[
        ("storage", false),
        ("sessionstore-backups", false),
        ("extensions", false),
        ("browser-extension-data", false),
    ];

    /// Bundle path of the normalized open-tab URL list: the deterministic thing
    /// the destination reopens, independent of Firefox's version-sensitive
    /// session restore. The selection key for "Open tabs".
    pub const TABS_JSON: &str = "firefox/tabs.json";

    /// `user.js` written into the imported profile: suppress onboarding, restore
    /// the previous session, and follow the system proxy (for the hotspot).
    pub const ONBOARDING_USER_JS: &str = r#"// Written by Cua Spaces teleport — skip onboarding, restore session.
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("startup.homepage_override_url", "");
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.messaging-system.whatsNewPanel.enabled", false);
user_pref("trailhead.firstrun.didSeeAboutWelcome", true);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.shell.didSkipDefaultBrowserCheckOnFirstRun", true);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("datareporting.policy.firstRunURL", "");
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("browser.startup.page", 3);
user_pref("browser.sessionstore.resume_from_crash", true);
user_pref("browser.tabs.warnOnClose", false);
user_pref("browser.urlbar.searchTips.test.enabled", false);
user_pref("browser.urlbar.tipShownCount.searchTip_onboard", 999);
user_pref("browser.urlbar.tipShownCount.searchTip_redirect", 999);
user_pref("browser.bookmarks.restore_default_bookmarks", false);
user_pref("browser.toolbars.bookmarks.visibility", "never");
user_pref("browser.warnOnQuit", false);
user_pref("browser.rights.3.shown", true);
user_pref("browser.startup.upgradeDialog.version", 999);
user_pref("network.proxy.type", 5);
user_pref("network.proxy.socks_remote_dns", true);
"#;

    /// Firefox's user-data root on `platform`, relative to the home.
    pub fn root_for(platform: Platform) -> &'static str {
        match platform {
            Platform::MacOS => "Library/Application Support/Firefox",
            Platform::Linux => ".mozilla/firefox",
            Platform::Windows => "AppData/Roaming/Mozilla/Firefox",
        }
    }

    /// The Firefox program on `platform`.
    pub fn launch_program_for(platform: Platform) -> String {
        match platform {
            Platform::MacOS => "/Applications/Firefox.app/Contents/MacOS/firefox".to_string(),
            Platform::Linux => "firefox".to_string(),
            Platform::Windows => r"C:\Program Files\Mozilla Firefox\firefox.exe".to_string(),
        }
    }

    /// A `profiles.ini` that makes the imported profile the default.
    pub fn default_profiles_ini() -> String {
        format!(
            "[Profile0]\nName=cua\nIsRelative=1\nPath=Profiles/{DEST_PROFILE}\nDefault=1\n\n[General]\nStartWithLastProfile=1\nVersion=2\n"
        )
    }

    /// Where the app is installed on a macOS source (install probe).
    pub const MACOS_APP: &str = "/Applications/Firefox.app";
}

/// Electron / Chromium apps (Slack, Discord, Unity Hub).
///
/// They keep a logged-in session in a Chromium profile: the auth token in
/// `Local Storage/leveldb`, session cookies in `Cookies`, and — on macOS —
/// those are encrypted with an app-specific "<App> Safe Storage" key in the
/// login Keychain. One [`ElectronApp`] descriptor drives both halves.
pub mod electron {
    use super::{LaunchSpec, Platform, plain_launch};

    /// Static description of one Electron/Chromium app.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ElectronApp {
        /// Stable provider id (bundle header + registry lookup), e.g. "slack".
        pub id: &'static str,
        /// Human name for consent UIs and `open -a`, e.g. "Slack".
        pub display: &'static str,
        /// App identifiers this provider matches (bundle ids + names, any case).
        pub app_ids: &'static [&'static str],
        /// Profile directory name under the platform app-support root
        /// (macOS `~/Library/Application Support/<dir>`, Linux `~/.config/<dir>`,
        /// Windows `%APPDATA%/<dir>`).
        pub support_dir: &'static str,
        /// macOS login-Keychain services this app's session depends on. Usually the
        /// "<App> Safe Storage" key that decrypts its cookies; some apps (Unity Hub)
        /// ALSO keep the actual auth token in a separate service (e.g. "unity" /
        /// account "auth-tokens:<id>"), so this is a list — every matching item is
        /// carried and reinstalled on the destination.
        pub keychain_services: &'static [&'static str],
        /// Extra profile files/dirs to carry beyond the standard set (e.g. Slack's
        /// `storage`).
        pub extra: &'static [&'static str],
        /// Extra files/dirs to carry that live in `$HOME`, NOT under the app-support
        /// profile dir — e.g. Unity Hub's `~/.plastic4`, which holds the Unity
        /// Version Control (Plastic) client credentials. Captured under the
        /// [`HOME_PREFIX`] bundle prefix and restored relative to the destination's
        /// home.
        pub home_extra: &'static [&'static str],
    }

    impl ElectronApp {
        /// Where the app is installed on a macOS host (install probe, and the
        /// app trusted for its Keychain items on the destination).
        pub fn macos_app(&self) -> String {
            format!("/Applications/{}.app", self.display)
        }
    }

    /// Slack.
    pub const SLACK: ElectronApp = ElectronApp {
        id: "slack",
        display: "Slack",
        app_ids: &["com.tinyspeck.slackmacgap", "slack", "Slack"],
        support_dir: "Slack",
        keychain_services: &["Slack Safe Storage"],
        extra: &["storage"],
        home_extra: &[],
    };

    /// Discord.
    pub const DISCORD: ElectronApp = ElectronApp {
        id: "discord",
        display: "Discord",
        app_ids: &["com.hnc.Discord", "discord", "Discord"],
        support_dir: "discord",
        // Discord names its Keychain item with a lowercase 'd'.
        keychain_services: &["discord Safe Storage"],
        extra: &[],
        home_extra: &[],
    };

    /// Unity Hub.
    ///
    /// Login lives in accounts.db plus the "unity" Keychain item (service
    /// "unity", account "auth-tokens:<foreignKey>") that its
    /// KeyringTokenManager reads.
    ///
    /// The first-run / "get set up" marker files are deliberately NOT carried:
    /// they are UI state, not identity, and they are whatever the SOURCE machine
    /// happens to have. Teleporting them imported unfinished onboarding into the
    /// destination (a "Unity CLI is now available!" modal covering a freshly
    /// teleported, signed-in Hub where nothing can click it away). The
    /// destination owns its own onboarding state.
    pub const UNITY_HUB: ElectronApp = ElectronApp {
        id: "unity-hub",
        display: "Unity Hub",
        app_ids: &["com.unity3d.unityhub", "unityhub", "unity-hub", "Unity Hub"],
        support_dir: "UnityHub",
        keychain_services: &["UnityHub Safe Storage", "unity"],
        extra: &[
            "accounts.db",
            "accounts.db-shm",
            "accounts.db-wal",
            "hubInfo.json",
            "hubConfig.json",
            "user-settings.json",
            "editors.json",
        ],
        // Unity Version Control (Plastic) client credentials: listing and cloning
        // a UVCS repo needs this profile. Individual config files, not the
        // `.plastic4` directory: that also holds `logs/`, which reaches hundreds
        // of MB. `plastic.workspaces` is deliberately NOT carried — it maps
        // workspaces to *host* paths.
        home_extra: &[
            ".plastic4/tokens.conf",
            ".plastic4/client.conf",
            ".plastic4/unityorgs.conf",
            ".plastic4/cloudregions.conf",
            ".plastic4/profiles.conf",
            // The Unity Editor entitlement: a signed-in Hub is not a licensed
            // Editor, and a destination with no license cannot open a project.
            // SENSITIVE and seat-limited (Unity Personal licenses per machine),
            // but carried by default because a Space without it cannot open a
            // project at all; the consent UI marks it sensitive.
            "Library/Unity/licenses/UnityEntitlementLicense.xml",
        ],
    };

    /// The built-in Electron apps, in registry order.
    pub const APPS: &[ElectronApp] = &[SLACK, DISCORD, UNITY_HUB];

    /// The built-in Electron app with provider id `id`.
    pub fn app(id: &str) -> Option<ElectronApp> {
        APPS.iter().copied().find(|app| app.id == id)
    }

    /// Standard Chromium profile files that carry the logged-in session. The
    /// `-wal`/`-shm` sidecars are included so a Cookies DB captured while the app
    /// runs (WAL mode) still carries its most recent writes.
    pub const PROFILE_FILES: &[(&str, bool)] = &[
        ("Cookies", true),
        ("Cookies-journal", true),
        ("Cookies-wal", true),
        ("Cookies-shm", true),
        ("Network/Cookies", true),
        ("Network/Cookies-wal", true),
        ("Network/Cookies-shm", true),
        ("Local State", true),
        ("Preferences", false),
    ];

    /// Standard Chromium profile directories carrying session/token state.
    pub const PROFILE_DIRS: &[(&str, bool)] = &[
        ("Local Storage", true),
        ("Session Storage", true),
        ("IndexedDB", true),
    ];

    /// Canonical bundle prefix for profile contents.
    pub const BUNDLE_PREFIX: &str = "electron";

    /// Bundle prefix for [`ElectronApp::home_extra`] payloads.
    pub const HOME_PREFIX: &str = "home";

    /// Bundle path of a profile-relative name.
    pub fn rel(name: &str) -> String {
        format!("{BUNDLE_PREFIX}/{name}")
    }

    /// Bundle path of a `$HOME`-relative payload.
    pub fn home_rel(name: &str) -> String {
        format!("{HOME_PREFIX}/{name}")
    }

    /// The platform app-support root that holds per-app profile dirs, relative
    /// to the home.
    pub fn app_support_root(platform: Platform) -> &'static str {
        match platform {
            Platform::MacOS => "Library/Application Support",
            Platform::Linux => ".config",
            Platform::Windows => "AppData/Roaming",
        }
    }

    /// The app's profile dir on `platform`, relative to the home.
    pub fn profile_dir(app: &ElectronApp, platform: Platform) -> String {
        format!("{}/{}", app_support_root(platform), app.support_dir)
    }

    /// Where a bundle path lands on `platform`, relative to the destination
    /// home: `home/…` payloads relative to the home itself, `electron/…`
    /// profile contents in the profile dir. `None` for anything else.
    pub fn remap(app: &ElectronApp, rel_path: &str, platform: Platform) -> Option<String> {
        if let Some(within) = rel_path.strip_prefix(&format!("{HOME_PREFIX}/")) {
            return Some(within.to_string());
        }
        rel_path
            .strip_prefix(&format!("{BUNDLE_PREFIX}/"))
            .map(|within| format!("{}/{within}", profile_dir(app, platform)))
    }

    /// `pgrep -f`/`pkill -f` pattern matching a running instance on `platform`
    /// (the executable path inside the bundle on macOS, so unrelated processes
    /// that merely mention the name are left alone). `None` where there is no
    /// such mechanism (Windows).
    pub fn process_pattern(platform: Platform, display: &str) -> Option<String> {
        match platform {
            Platform::MacOS => Some(format!("{display}.app/Contents/MacOS/")),
            Platform::Linux => Some(display.to_lowercase()),
            Platform::Windows => None,
        }
    }

    /// Launch spec: on macOS open the app bundle by name (`open -a <App>`), on
    /// Linux/Windows exec the binary.
    pub fn launch_spec_for(platform: Platform, display: &str) -> LaunchSpec {
        match platform {
            Platform::MacOS => plain_launch("open", vec!["-a".to_string(), display.to_string()]),
            Platform::Linux => plain_launch(display.to_lowercase(), Vec::new()),
            Platform::Windows => plain_launch(format!("{display}.exe"), Vec::new()),
        }
    }
}

/// Steam (native client).
///
/// Its "stay logged in" state lives in a few Valve KeyValues files plus any
/// `ssfn*` sentry files that mark a machine as Steam-Guard-trusted.
pub mod steam {
    use super::{LaunchSpec, Platform, plain_launch};

    /// Provider id.
    pub const ID: &str = "steam";
    /// Display name.
    pub const DISPLAY: &str = "Steam";
    /// Application identifiers the sender matches.
    pub const APP_IDS: &[&str] = &["com.valvesoftware.steam", "steam", "Steam"];
    /// Canonical (root-relative) bundle prefix.
    pub const BUNDLE_PREFIX: &str = "steam";
    /// Curated login files, relative to the Steam root. All are sensitive.
    pub const LOGIN_FILES: &[&str] =
        &["config/config.vdf", "config/loginusers.vdf", "registry.vdf"];
    /// Prefix of the Steam Guard sentry files in the Steam root.
    pub const SENTRY_PREFIX: &str = "ssfn";

    /// Steam's root on `platform`, relative to the home.
    pub fn root_for(platform: Platform) -> &'static str {
        match platform {
            Platform::MacOS => "Library/Application Support/Steam",
            // Modern Linux Steam keeps the config under ~/.steam/steam.
            Platform::Linux => ".steam/steam",
            Platform::Windows => "AppData/Local/Steam",
        }
    }

    /// Where a bundle path lands on `platform`, relative to the home.
    pub fn remap(rel_path: &str, platform: Platform) -> Option<String> {
        rel_path
            .strip_prefix(&format!("{BUNDLE_PREFIX}/"))
            .map(|within| format!("{}/{within}", root_for(platform)))
    }

    /// Launch spec on `platform`.
    pub fn launch_spec_for(platform: Platform) -> LaunchSpec {
        match platform {
            Platform::MacOS => plain_launch("open", vec!["-a".to_string(), "Steam".to_string()]),
            Platform::Linux => plain_launch("steam", Vec::new()),
            Platform::Windows => {
                plain_launch(r"C:\Program Files (x86)\Steam\steam.exe", Vec::new())
            }
        }
    }

    /// Where the app is installed on a macOS source (install probe).
    pub const MACOS_APP: &str = "/Applications/Steam.app";
}

/// WhatsApp Desktop (macOS native, sandboxed).
///
/// The linked-device identity and message store live in App Group
/// containers; each root is copied recursively and restored to the same
/// home-relative path.
pub mod whatsapp {
    use super::{LaunchSpec, plain_launch};

    /// Provider id.
    pub const ID: &str = "whatsapp";
    /// Display name.
    pub const DISPLAY: &str = "WhatsApp";
    /// Application identifiers the sender matches.
    pub const APP_IDS: &[&str] = &["net.whatsapp.WhatsApp", "whatsapp", "WhatsApp"];

    /// (bundle prefix, home-relative root).
    pub const ROOTS: &[(&str, &str)] = &[
        (
            "group-shared",
            "Library/Group Containers/group.net.whatsapp.WhatsApp.shared",
        ),
        (
            "group-private",
            "Library/Group Containers/group.net.whatsapp.WhatsApp.private",
        ),
        (
            "container",
            "Library/Containers/net.whatsapp.WhatsApp/Data/Library/Application Support/net.whatsapp.WhatsApp",
        ),
    ];

    /// Where a bundle path (`<prefix>/<rest>`) lands, relative to the home.
    pub fn remap(rel_path: &str) -> Option<String> {
        ROOTS.iter().find_map(|(prefix, root)| {
            rel_path
                .strip_prefix(&format!("{prefix}/"))
                .map(|rest| format!("{root}/{rest}"))
        })
    }

    /// Launch spec (macOS only).
    pub fn launch_spec() -> LaunchSpec {
        plain_launch("open", vec!["-a".to_string(), "WhatsApp".to_string()])
    }

    /// Where the app is installed on a macOS source (install probe).
    pub const MACOS_APP: &str = "/Applications/WhatsApp.app";
}

/// The Claude Code CLI.
pub mod claude_code {
    use super::LaunchSpec;

    /// Provider id.
    pub const ID: &str = "claude-code";
    /// Display name.
    pub const DISPLAY: &str = "Claude Code";
    /// Application identifiers the sender matches.
    pub const APP_IDS: &[&str] = &[
        "claude-code",
        "Claude Code",
        "com.anthropic.claude-code",
        "claude",
    ];
    /// macOS Keychain service holding the OAuth credentials on the source.
    pub const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
    /// Bundle path of the OAuth credential blob.
    pub const CRED_REL: &str = "claude/.credentials.json";
    /// Bundle path of the top-level config (`~/.claude.json`).
    pub const CONFIG_REL: &str = "claude/.claude.json";
    /// Bundle prefix for the per-project conversation transcripts.
    pub const PROJECTS_PREFIX: &str = "claude/projects";
    /// Home-relative on-disk credentials (non-macOS and older installs).
    pub const CREDENTIALS_FILE: &str = ".claude/.credentials.json";
    /// Home-relative config file.
    pub const CONFIG_FILE: &str = ".claude.json";
    /// Home-relative transcripts directory.
    pub const PROJECTS_DIR: &str = ".claude/projects";

    /// Where a bundle path lands, relative to the home:
    ///
    /// - `claude/.credentials.json` → `.claude/.credentials.json`
    /// - `claude/.claude.json`      → `.claude.json`
    /// - `claude/projects/...`      → `.claude/projects/...`
    pub fn remap(rel_path: &str) -> Option<String> {
        if rel_path == CRED_REL {
            return Some(CREDENTIALS_FILE.to_string());
        }
        if rel_path == CONFIG_REL {
            return Some(CONFIG_FILE.to_string());
        }
        rel_path
            .strip_prefix(&format!("{PROJECTS_PREFIX}/"))
            .map(|rest| format!("{PROJECTS_DIR}/{rest}"))
    }

    /// Whether a bundle path is the credential blob (always written 0600).
    pub fn is_credentials(rel_path: &str) -> bool {
        rel_path == CRED_REL
    }

    /// A launch spec that opens a terminal running `claude` on the guest
    /// desktop (`DISPLAY=:1`), preferring `xfce4-terminal` over `xterm`. The
    /// launcher installs Claude Code on demand and runs under `--hold`, so a
    /// failed install or launch stays visible instead of flashing shut.
    pub fn launch_terminal_spec() -> LaunchSpec {
        let script = "cat > /tmp/cua-claude-launch.sh <<'EOS'\n\
#!/bin/bash\n\
export PATH=\"$HOME/.cua/bin:$HOME/.local/bin:/usr/local/bin:$PATH\"\n\
if ! command -v claude >/dev/null 2>&1; then\n\
  echo 'Claude Code is not installed in this sandbox. The sender installs it (pinned, verified) before teleport; or run: cua agent ensure <sandbox> claude-code'\n\
  exec bash\n\
fi\n\
exec claude\n\
EOS\n\
chmod +x /tmp/cua-claude-launch.sh\n\
xfce4-terminal --hold --title='Claude Code' -e /tmp/cua-claude-launch.sh \
        || xterm -hold -title 'Claude Code' -e /tmp/cua-claude-launch.sh";
        LaunchSpec {
            program: "bash".to_string(),
            args: vec!["-lc".to_string(), script.to_string()],
            env: vec![("DISPLAY".to_string(), ":1".to_string())],
            cwd: None,
            restore_windows: Vec::new(),
        }
    }
}

/// Where bundle path `rel_path` of provider `provider_id` lands on a
/// `platform` destination, relative to the destination home, the same place
/// the receiver writes it. `None` for bundle-internal entries that are not a
/// file in the home (a tab list, the carried keychain item) and for
/// providers or paths this layout does not know.
///
/// Consent screens show this, so a macOS Space lists
/// `Library/Application Support/Google/Chrome/…`, not the canonical Linux
/// layout the bundle travels in.
pub fn landing_path(provider_id: &str, rel_path: &str, platform: Platform) -> Option<String> {
    let canonical = |prefix: &str| rel_path.starts_with(&format!("{prefix}/"));
    match provider_id {
        chrome::ID if canonical(chrome::LINUX_USER_DATA_DIR) => {
            Some(chrome::remap_to_platform(rel_path, platform))
        }
        firefox::ID => rel_path
            .strip_prefix(&format!("{}/", firefox::BUNDLE_PREFIX))
            .filter(|_| rel_path != firefox::TABS_JSON)
            .map(|within| {
                format!(
                    "{}/Profiles/{}/{within}",
                    firefox::root_for(platform),
                    firefox::DEST_PROFILE
                )
            }),
        steam::ID => steam::remap(rel_path, platform),
        whatsapp::ID => whatsapp::remap(rel_path),
        claude_code::ID => claude_code::remap(rel_path),
        id => electron::app(id).and_then(|app| electron::remap(&app, rel_path, platform)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_are_unique_and_stable() {
        let mut ids = PROVIDER_IDS.to_vec();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), PROVIDER_IDS.len());
        assert_eq!(
            PROVIDER_IDS,
            [
                "chrome",
                "firefox",
                "slack",
                "discord",
                "unity-hub",
                "steam",
                "whatsapp",
                "claude-code"
            ]
        );
    }

    #[test]
    fn chrome_remaps_the_canonical_layout_per_platform() {
        let rel = chrome::dest_rel_path("Default", "Cookies");
        assert_eq!(rel, ".config/google-chrome/Default/Cookies");
        assert_eq!(
            chrome::remap_to_platform(&rel, Platform::MacOS),
            "Library/Application Support/Google/Chrome/Default/Cookies"
        );
        assert_eq!(chrome::remap_to_platform(&rel, Platform::Linux), rel);
        assert_eq!(
            chrome::remap_to_platform(&rel, Platform::Windows),
            "AppData/Local/Google/Chrome/User Data/Default/Cookies"
        );
        // Unexpected paths pass through untouched.
        assert_eq!(
            chrome::remap_to_platform("tabs.json", Platform::MacOS),
            "tabs.json"
        );
        assert!(chrome::launch_program_for(Platform::MacOS).ends_with("MacOS/Google Chrome"));
    }

    #[test]
    fn landing_paths_follow_the_destination_platform() {
        let cookies = chrome::dest_rel_path("Default", "Cookies");
        assert_eq!(
            landing_path(chrome::ID, &cookies, Platform::MacOS).as_deref(),
            Some("Library/Application Support/Google/Chrome/Default/Cookies")
        );
        assert_eq!(
            landing_path(chrome::ID, &cookies, Platform::Linux).as_deref(),
            Some(".config/google-chrome/Default/Cookies")
        );
        assert_eq!(
            landing_path(chrome::ID, &cookies, Platform::Windows).as_deref(),
            Some("AppData/Local/Google/Chrome/User Data/Default/Cookies")
        );
        // Bundle-internal entries land nowhere in the home.
        assert_eq!(
            landing_path(chrome::ID, chrome::TABS_JSON, Platform::MacOS),
            None
        );
        assert_eq!(
            landing_path(chrome::ID, crate::keychain::KEYCHAIN_ENTRY, Platform::MacOS),
            None
        );
        assert_eq!(
            landing_path(firefox::ID, "firefox/cookies.sqlite", Platform::MacOS).as_deref(),
            Some("Library/Application Support/Firefox/Profiles/cua.default-release/cookies.sqlite")
        );
        assert_eq!(
            landing_path(firefox::ID, firefox::TABS_JSON, Platform::MacOS),
            None
        );
        assert_eq!(
            landing_path(claude_code::ID, "claude/projects/", Platform::Linux).as_deref(),
            Some(".claude/projects/")
        );
        assert_eq!(
            landing_path(electron::SLACK.id, "electron/Cookies", Platform::MacOS),
            electron::remap(&electron::SLACK, "electron/Cookies", Platform::MacOS)
        );
        assert_eq!(landing_path("nope", "x/y", Platform::MacOS), None);
    }

    #[test]
    fn firefox_layout_per_platform() {
        assert_eq!(firefox::root_for(Platform::Linux), ".mozilla/firefox");
        assert_eq!(
            firefox::root_for(Platform::MacOS),
            "Library/Application Support/Firefox"
        );
        assert!(firefox::default_profiles_ini().contains("Path=Profiles/cua.default-release"));
        assert!(firefox::TABS_JSON.starts_with(firefox::BUNDLE_PREFIX));
    }

    #[test]
    fn electron_remaps_profile_and_home_payloads() {
        let hub = electron::app("unity-hub").unwrap();
        assert_eq!(
            electron::remap(&hub, "electron/accounts.db", Platform::MacOS).as_deref(),
            Some("Library/Application Support/UnityHub/accounts.db")
        );
        assert_eq!(
            electron::remap(&hub, "electron/Cookies", Platform::Linux).as_deref(),
            Some(".config/UnityHub/Cookies")
        );
        assert_eq!(
            electron::remap(&hub, "home/.plastic4/tokens.conf", Platform::Linux).as_deref(),
            Some(".plastic4/tokens.conf")
        );
        assert_eq!(
            electron::remap(&hub, "keychain.json", Platform::MacOS),
            None
        );
        let spec = electron::launch_spec_for(Platform::MacOS, hub.display);
        assert_eq!(
            (spec.program.as_str(), spec.args.as_slice()),
            ("open", &["-a".to_string(), "Unity Hub".to_string()][..])
        );
        assert_eq!(
            electron::launch_spec_for(Platform::Linux, "Slack").program,
            "slack"
        );
        assert_eq!(
            electron::process_pattern(Platform::MacOS, "Slack").as_deref(),
            Some("Slack.app/Contents/MacOS/")
        );
        assert_eq!(electron::process_pattern(Platform::Windows, "Slack"), None);
        assert_eq!(hub.macos_app(), "/Applications/Unity Hub.app");
    }

    #[test]
    fn steam_whatsapp_and_claude_remaps() {
        assert_eq!(
            steam::remap("steam/config/config.vdf", Platform::Linux).as_deref(),
            Some(".steam/steam/config/config.vdf")
        );
        assert_eq!(steam::remap("other", Platform::Linux), None);
        assert_eq!(
            whatsapp::remap("group-private/a/b").as_deref(),
            Some("Library/Group Containers/group.net.whatsapp.WhatsApp.private/a/b")
        );
        assert_eq!(whatsapp::remap("nope/x"), None);
        assert_eq!(
            claude_code::remap("claude/.credentials.json").as_deref(),
            Some(".claude/.credentials.json")
        );
        assert_eq!(
            claude_code::remap("claude/.claude.json").as_deref(),
            Some(".claude.json")
        );
        assert_eq!(
            claude_code::remap("claude/projects/p/1.jsonl").as_deref(),
            Some(".claude/projects/p/1.jsonl")
        );
        assert_eq!(claude_code::remap("claude/other"), None);
        assert_eq!(
            claude_code::launch_terminal_spec().env,
            [("DISPLAY".to_string(), ":1".to_string())]
        );
    }
}
