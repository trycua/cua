//! Which Cua Spaces build this is: the released app (`stable`) or a locally
//! built one (`local`, "Cua Spaces Local"). CUA-1776.
//!
//! The two run on one Mac at the same time, so nothing the Local build
//! writes, registers, listens on or stores may be shared with the Stable
//! install. [`CuaChannel`] is the one place that owns every name that has to
//! differ; nothing else hard-codes one. (Same idea as `bundle.rs` in Cua
//! Driver, which keeps `cua-driver` and `cua-driver-local` apart.)
//!
//! | Name                  | Stable                                        | Local                                            |
//! |-----------------------|-----------------------------------------------|--------------------------------------------------|
//! | Bundle id             | `com.trycua.spaces.macos`                     | `com.trycua.spaces.macos.local`                  |
//! | App                   | `/Applications/Cua Spaces.app`                | `~/Applications/Cua Spaces Local.app`            |
//! | CLI                   | `~/.local/bin/cua`                            | `~/.local/bin/cua-local`                         |
//! | Cua home              | `~/.cua` (`envs/staging` below it on staging) | `~/.cua-local` (`envs/prod` below it on prod)    |
//! | Default environment   | prod                                          | staging (`CUA_ENV=prod` opts in)                 |
//! | Sign-in Keychain      | `ai.cua.session` (`.staging`)                 | `ai.cua.session.local` (`.staging`)              |
//! | Legacy Keychain       | `run.cua.ai` (`staging.run.cua.ai`), moved once | none: Local never had one, nothing is moved    |
//! | App helper            | `com.trycua.spaces.helper`                    | `com.trycua.spaces.local.helper`                 |
//! | Helper spec           | `~/.cua/helper/service.json`                  | `~/.cua-local/helper/service.json`               |
//! | Plain host runner     | `com.trycua.spacesd.host`                     | `com.trycua.spacesd.host.local`                  |
//! | spacesd ports         | 3211 TCP, 3212 QUIC, mesh UDP 3212            | 3221 TCP, 3222 QUIC, mesh UDP 3212 (shared)      |
//! | URL scheme            | `cua://`                                      | `cua-local://`                                   |
//! | Cua Volume            | `~/Cua Volume`                                | `~/Cua Volume Local`                             |
//! | Lume                  | `lume`, `com.trycua.lume_daemon`, `:7777`, `~/.lume` | `lume-local`, `com.trycua.lume_daemon.local`, `:7787`, `~/.lume-local` |
//! | App data              | `~/Library/Application Support/com.trycua.spaces.macos` | `.../com.trycua.spaces.macos.local`    |
//! | Agent MCP server      | `cua` (launches `cua mcp`)                    | `cua-local` (launches `cua-local mcp`)           |
//! | Agent skills          | `cua-spaces`, `cua-volume`, ...               | `cua-spaces-local`, `cua-volume-local`, ...      |
//!
//! **Bundle ids.** Stable stays `com.trycua.spaces.macos`. Local is that id
//! plus `.local`, so a future iOS app is `com.trycua.spaces.ios` (and
//! `.ios.local`) in the same scheme. The `.macos.local` id is a dot extension
//! of Stable's, so compare ids whole, never by prefix (`starts_with`, a shell
//! `*id*` pattern): a prefix test would take Local for Stable. Local's first
//! builds used `com.trycua.spaces.local` ([`CuaChannel::legacy_bundle_id`]);
//! `install-local.sh` and `uninstall-local.sh` remove what that id left behind.
//! The launchd ids (helper, host runner, Lume daemon) did not change.
//!
//! # Resolution
//!
//! [`CuaChannel::current`] picks the channel in this order:
//!
//! 1. the `CUA_CHANNEL` environment variable (`stable` or `local`);
//! 2. the value compiled in with `CUA_DEFAULT_CHANNEL` (a Local build is
//!    compiled with `local`), like `CUA_DEFAULT_ENV` for [`CuaEnv`];
//! 3. the executable path: inside `Cua Spaces Local.app`, or an executable
//!    named `cua-local`, means Local;
//! 4. otherwise Stable.
//!
//! The app sets `CUA_CHANNEL` before anything starts, so the helper,
//! spacesd, the daemon and the bundled `cua` all inherit it and agree.
//!
//! # Environment
//!
//! Local defaults to the staging deployment, so a Local build never signs
//! in to, or registers a host with, production by accident. `CUA_ENV=prod`
//! is an explicit opt-in. See [`CuaChannel::default_env`] and
//! [`CuaChannel::current_env`].
//!
//! The environment follows the channel everywhere: [`CuaEnv::build_default`],
//! [`CuaEnv::current`], `Endpoints::from_lookup` (which reads `CUA_CHANNEL`
//! from its lookup, so tests stay hermetic) and [`crate::cua_home`] all go
//! through this module. The home follows both:
//!
//! | Channel | Environment | Home                      |
//! |---------|-------------|---------------------------|
//! | Stable  | prod        | `~/.cua`                  |
//! | Stable  | staging     | `~/.cua/envs/staging`     |
//! | Local   | staging     | `~/.cua-local`            |
//! | Local   | prod        | `~/.cua-local/envs/prod`  |
//!
//! Each channel keeps its own folder for the environment it is made for
//! (Stable for prod, Local for staging) and puts the other one below it
//! (Stable's staging, Local's prod), so a Local build that opts in to
//! production never shares a session, registry or socket with its staging
//! state, or with Stable. The app pins `CUA_HOME` to it. Each of the four
//! also has its own Keychain service ([`CuaChannel::keychain_service`]).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::env::{CUA_ENV_VAR, CuaEnv};

/// The environment variable that selects the channel.
pub const CUA_CHANNEL_VAR: &str = "CUA_CHANNEL";

/// The file name of the Stable CLI.
const STABLE_CLI_NAME: &str = "cua";

/// The file name of the Local CLI.
const LOCAL_CLI_NAME: &str = "cua-local";

/// How a hint starts a command: a backtick, then `cua` and a space.
const CLI_HINT_PREFIX: &str = "`cua ";

/// The directory name of the Local app bundle.
const LOCAL_APP_DIR_NAME: &str = "Cua Spaces Local.app";

/// The name Local registers the cua MCP server under in coding agents.
const LOCAL_MCP_SERVER_NAME: &str = "cua-local";

/// What Local adds to a bundled skill's name in coding agents' skills folders.
const LOCAL_SKILL_SUFFIX: &str = "-local";

/// The fixed TCP and UDP ports of a channel's `cua-spacesd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ports {
    /// The spacesd HTTP/WebSocket port (`--port`).
    pub tcp: u16,
    /// The spacesd QUIC port (`--quic-port`, UDP).
    pub quic: u16,
    /// The mesh media UDP port (`cua-mesh`'s `MEDIA_UDP_PORT`), 3212 for
    /// every channel, by design. It is a port of the mesh's own in-process
    /// network, not a socket of the operating system, so a Stable and a
    /// Local host on one Mac never compete for it; and it must match what
    /// the peers dial, which guest Spaces and older cua-spacesd builds
    /// fix at 3212. (`--mesh-udp-port` stays an optional override.)
    pub mesh_udp: u16,
}

/// A Cua Spaces build channel: the released app or a local build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CuaChannel {
    /// The released app and its CLI (the default).
    #[default]
    Stable,
    /// A locally built "Cua Spaces Local", beside the Stable install.
    Local,
}

impl CuaChannel {
    /// Both channels.
    pub const ALL: [CuaChannel; 2] = [Self::Stable, Self::Local];

    // ----- parsing and resolution ------------------------------------------

    /// Parses a `CUA_CHANNEL` value: `stable` or `local` (any case,
    /// surrounding space ignored). Anything else, empty included, is `None`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Self::Stable),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    /// The value compiled in with `CUA_DEFAULT_CHANNEL` (a Local build is
    /// compiled with `local`); `None` when the build set none or an unknown
    /// value.
    pub fn build_default() -> Option<Self> {
        option_env!("CUA_DEFAULT_CHANNEL").and_then(Self::parse)
    }

    /// Whether `path` is a Local executable: it is named `cua-local`
    /// (`cua-local.exe`), or it sits inside `Cua Spaces Local.app`. File
    /// names are compared case-insensitively on Windows, where
    /// `CUA-LOCAL.EXE` is the same file.
    pub fn path_is_local(path: &Path) -> bool {
        Self::path_is_local_with(path, cfg!(windows))
    }

    /// [`CuaChannel::path_is_local`], with the case rule given (for tests).
    fn path_is_local_with(path: &Path, ignore_case: bool) -> bool {
        let fold = |s: &str| {
            if ignore_case {
                s.to_ascii_lowercase()
            } else {
                s.to_string()
            }
        };
        let name = fold(
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
        );
        let cli = fold(LOCAL_CLI_NAME);
        let app = fold(LOCAL_APP_DIR_NAME);
        name == cli
            || name == format!("{cli}.exe")
            || path
                .components()
                .any(|c| c.as_os_str().to_str().is_some_and(|c| fold(c) == app))
    }

    /// The resolution order, with its three inputs given (for tests and for
    /// callers that carry their own environment):
    ///
    /// 1. `env_value`, the `CUA_CHANNEL` value: when set (not blank) it
    ///    decides, and an unknown value is an error naming the accepted
    ///    ones;
    /// 2. `build_default`, the compile-time `CUA_DEFAULT_CHANNEL` value
    ///    (an unknown value is ignored);
    /// 3. `exe_paths`, the paths the running executable is known by
    ///    (`current_exe()`, that with symlinks resolved, and the invocation
    ///    path `argv[0]`): Local if any is [`CuaChannel::path_is_local`];
    /// 4. else Stable.
    pub fn resolve(
        env_value: Option<&str>,
        build_default: Option<&str>,
        exe_paths: &[PathBuf],
    ) -> Result<Self, String> {
        if let Some(v) = env_value.filter(|v| !v.trim().is_empty()) {
            return Self::parse(v).ok_or_else(|| {
                format!("{CUA_CHANNEL_VAR}={v:?} is not a Cua channel (stable or local)")
            });
        }
        if let Some(c) = build_default.and_then(Self::parse) {
            return Ok(c);
        }
        if exe_paths.iter().any(|p| Self::path_is_local(p)) {
            return Ok(Self::Local);
        }
        Ok(Self::Stable)
    }

    /// The channel of this process, or an error for an unknown
    /// `CUA_CHANNEL` value. See [`CuaChannel::resolve`].
    pub fn try_current() -> Result<Self, String> {
        Self::resolve(
            std::env::var(CUA_CHANNEL_VAR).ok().as_deref(),
            option_env!("CUA_DEFAULT_CHANNEL"),
            &current_exe_paths(),
        )
    }

    /// The channel of this process for a given `CUA_CHANNEL` value (`None`:
    /// unset), the compile-time default and this executable's path. An
    /// unknown value is ignored, as in [`CuaChannel::current`]. For callers
    /// that read their variables through a lookup (`Endpoints::from_lookup`).
    pub fn from_env_value(env_value: Option<&str>) -> Self {
        let build_default = option_env!("CUA_DEFAULT_CHANNEL");
        let exe_paths = current_exe_paths();
        Self::resolve(env_value, build_default, &exe_paths)
            .or_else(|_| Self::resolve(None, build_default, &exe_paths))
            .unwrap_or_default()
    }

    /// The channel of this process. An unknown `CUA_CHANNEL` value is
    /// ignored, so the compile-time default and the executable path still
    /// apply: a typo never moves a Local build onto Stable's names.
    pub fn current() -> Self {
        Self::from_env_value(std::env::var(CUA_CHANNEL_VAR).ok().as_deref())
    }

    /// `stable` or `local`: the value of `CUA_CHANNEL`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Local => "local",
        }
    }

    /// The `channel` telemetry property: `prod` or `local`. Telemetry only,
    /// never a gate (clients are open source and can claim anything).
    pub fn telemetry_channel(self) -> &'static str {
        match self {
            Self::Stable => "prod",
            Self::Local => "local",
        }
    }

    /// Whether this is the Local channel.
    pub fn is_local(self) -> bool {
        self == Self::Local
    }

    // ----- environment -----------------------------------------------------

    /// The deployment when `CUA_ENV` is not set: Stable keeps
    /// [`CuaEnv::compiled_default`] (prod, or staging for a beta build);
    /// Local is always staging.
    pub fn default_env(self) -> CuaEnv {
        match self {
            Self::Stable => CuaEnv::compiled_default(),
            Self::Local => CuaEnv::Staging,
        }
    }

    /// The deployment for a `CUA_ENV` value (`None`: unset).
    ///
    /// Stable is the rule `CuaEnv::try_from_value` always had: unset is
    /// [`CuaEnv::compiled_default`], blank is prod. Local treats unset and
    /// blank as the default (staging); anything else must be a known
    /// environment, so `prod` is an explicit opt-in and a typo is an error
    /// rather than production.
    pub fn env_from_value(self, value: Option<&str>) -> Result<CuaEnv, String> {
        match self {
            Self::Stable => CuaEnv::stable_from_value(value),
            Self::Local => match value.filter(|v| !v.trim().is_empty()) {
                None => Ok(self.default_env()),
                Some(v) => CuaEnv::parse(v).ok_or_else(|| {
                    format!("{CUA_ENV_VAR}={v:?} is not a Cua environment (prod or staging)")
                }),
            },
        }
    }

    /// [`CuaChannel::env_from_value`] with an unknown value answered rather
    /// than refused: production for Stable (what `CuaEnv::current` always
    /// did), staging for Local, so a typo never reaches production.
    pub fn env_or_fallback(self, value: Option<&str>) -> CuaEnv {
        self.env_from_value(value).unwrap_or(match self {
            Self::Stable => CuaEnv::Prod,
            Self::Local => CuaEnv::Staging,
        })
    }

    /// The deployment of this process: [`CuaChannel::current`] and `CUA_ENV`,
    /// or an error for an unknown `CUA_ENV` value.
    pub fn try_current_env() -> Result<CuaEnv, String> {
        Self::current().env_from_value(std::env::var(CUA_ENV_VAR).ok().as_deref())
    }

    /// The deployment of this process. An unknown `CUA_ENV` value falls back
    /// as [`CuaChannel::env_or_fallback`] says (never to production for
    /// Local).
    pub fn current_env() -> CuaEnv {
        Self::current().env_or_fallback(std::env::var(CUA_ENV_VAR).ok().as_deref())
    }

    // ----- the app ---------------------------------------------------------

    /// The bundle identifier, which TCC grants and the app's data are keyed
    /// to. Local's is Stable's plus `.local`, which makes it a dot extension
    /// of Stable's: compare whole ids, never by prefix.
    pub fn bundle_id(self) -> &'static str {
        match self {
            Self::Stable => "com.trycua.spaces.macos",
            Self::Local => "com.trycua.spaces.macos.local",
        }
    }

    /// The bundle identifier this channel used before the current one, which
    /// the installers move away from: Local's first builds were
    /// `com.trycua.spaces.local`. Stable has none. Nothing but the install
    /// and uninstall scripts reads it: they remove the old app and what it
    /// left in `~/Library`.
    pub fn legacy_bundle_id(self) -> Option<&'static str> {
        match self {
            Self::Stable => None,
            Self::Local => Some("com.trycua.spaces.local"),
        }
    }

    /// The app's display name (without `.app`).
    pub fn app_name(self) -> &'static str {
        match self {
            Self::Stable => "Cua Spaces",
            Self::Local => "Cua Spaces Local",
        }
    }

    /// Where the app is installed: `/Applications/Cua Spaces.app`, or
    /// `~/Applications/Cua Spaces Local.app` (never `/Applications`).
    pub fn app_path(self) -> PathBuf {
        match self {
            Self::Stable => PathBuf::from("/Applications").join("Cua Spaces.app"),
            Self::Local => user_home().join("Applications").join(LOCAL_APP_DIR_NAME),
        }
    }

    /// The app's folder name below `~/Library/Application Support`.
    pub fn app_support_dir_name(self) -> &'static str {
        // The same as the bundle id (macOS convention), but a separate name
        // so a later rename of one can move the folder.
        match self {
            Self::Stable => "com.trycua.spaces.macos",
            Self::Local => "com.trycua.spaces.macos.local",
        }
    }

    /// `~/Library/Application Support/<app_support_dir_name>` (settings,
    /// onboarding state).
    pub fn app_support_dir(self) -> PathBuf {
        user_home()
            .join("Library")
            .join("Application Support")
            .join(self.app_support_dir_name())
    }

    /// The URL scheme the app registers (`cua`, `cua-local`), without `://`.
    pub fn url_scheme(self) -> &'static str {
        match self {
            Self::Stable => "cua",
            Self::Local => "cua-local",
        }
    }

    /// The link that opens the Keyvault page of this channel's app:
    /// `cua://keyvault`, or `cua-local://keyvault` for Local. Every link a
    /// Rust crate hands out to the app goes through [`CuaChannel::url_scheme`],
    /// so a Local build never opens the released app.
    pub fn keyvault_url(self) -> String {
        format!("{}://keyvault", self.url_scheme())
    }

    /// [`CuaChannel::keyvault_url`] for this process's channel.
    pub fn current_keyvault_url() -> String {
        Self::current().keyvault_url()
    }

    // ----- the cua home, sign-in and the CLI -------------------------------

    /// The channel's cua home: `~/.cua` or `~/.cua-local`. This is the base:
    /// `CUA_HOME` overrides it, and the environment that is not the
    /// channel's own moves the default below it ([`CuaChannel::env_home_dir`]).
    pub fn home_dir(self) -> PathBuf {
        user_home().join(self.home_dir_name())
    }

    /// The folder name of the channel's cua home below the user's home:
    /// `.cua` or `.cua-local`.
    pub fn home_dir_name(self) -> &'static str {
        match self {
            Self::Stable => ".cua",
            Self::Local => ".cua-local",
        }
    }

    /// The folder below [`CuaChannel::home_dir`] that `env` uses, or `None`
    /// for the home itself. Stable keeps `~/.cua` for production and puts
    /// staging in `envs/staging` ([`CuaEnv::home_subdir`], as it always
    /// has); Local keeps `~/.cua-local` for staging, its default, and puts
    /// production in `envs/prod`. So the four (channel, environment) pairs
    /// never share a home, and no Local home is inside Stable's.
    pub fn home_subdir(self, env: CuaEnv) -> Option<&'static str> {
        match (self, env) {
            (Self::Stable, env) => env.home_subdir(),
            (Self::Local, CuaEnv::Staging) => None,
            (Self::Local, CuaEnv::Prod) => Some("envs/prod"),
        }
    }

    /// The cua home of this channel for `env` when `CUA_HOME` is not set:
    /// [`CuaChannel::home_dir`] plus [`CuaChannel::home_subdir`]
    /// (`~/.cua`, `~/.cua/envs/staging`, `~/.cua-local`,
    /// `~/.cua-local/envs/prod`).
    pub fn env_home_dir(self, env: CuaEnv) -> PathBuf {
        let base = self.home_dir();
        match self.home_subdir(env) {
            Some(sub) => base.join(sub),
            None => base,
        }
    }

    /// The OS credential vault service the session and the device key are
    /// stored under, for `env`. Stable's are [`CuaEnv::keyring_service`];
    /// Local adds `.local` before the `.staging` suffix. One service per
    /// (channel, environment) pair, so a Local build on staging never reads
    /// or replaces the sign-in of a Stable app on staging.
    pub fn keychain_service(self, env: CuaEnv) -> &'static str {
        match (self, env) {
            (Self::Stable, CuaEnv::Prod) => "ai.cua.session",
            (Self::Stable, CuaEnv::Staging) => "ai.cua.session.staging",
            (Self::Local, CuaEnv::Prod) => "ai.cua.session.local",
            (Self::Local, CuaEnv::Staging) => "ai.cua.session.local.staging",
        }
    }

    /// The service older builds stored the Stable session under, which a
    /// build moves an item out of once ([`CuaEnv::legacy_keyring_service`]).
    /// `None` for Local: it is new, has no legacy items, and must never
    /// read, move or delete anything under a name Stable owns.
    pub fn legacy_keychain_service(self, env: CuaEnv) -> Option<&'static str> {
        match self {
            Self::Stable => Some(env.legacy_keyring_service()),
            Self::Local => None,
        }
    }

    /// The channel and deployment of this process, resolved together (one
    /// look at the executable path): [`CuaChannel::current`] and
    /// [`CuaChannel::current_env`].
    pub fn current_with_env() -> (Self, CuaEnv) {
        let channel = Self::current();
        let env = channel.env_or_fallback(std::env::var(CUA_ENV_VAR).ok().as_deref());
        (channel, env)
    }

    /// The Keychain service of this process: [`CuaChannel::keychain_service`]
    /// for [`CuaChannel::current_with_env`].
    pub fn current_keychain_service() -> &'static str {
        let (channel, env) = Self::current_with_env();
        channel.keychain_service(env)
    }

    /// The legacy Keychain service of this process, if its channel has one:
    /// [`CuaChannel::legacy_keychain_service`] for
    /// [`CuaChannel::current_with_env`].
    pub fn current_legacy_keychain_service() -> Option<&'static str> {
        let (channel, env) = Self::current_with_env();
        channel.legacy_keychain_service(env)
    }

    /// The CLI's file name: `cua` or `cua-local`.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Stable => STABLE_CLI_NAME,
            Self::Local => LOCAL_CLI_NAME,
        }
    }

    /// `text` with every command it tells the reader to run named for this
    /// channel's CLI: a backticked `` `cua <verb> ...` `` becomes
    /// `` `cua-local <verb> ...` `` on Local, so a hint such as "run `cua host
    /// setup`" does not send a Local user to the Stable CLI. Stable's text is
    /// returned untouched (the same borrowed string), so its output stays
    /// byte-identical. Use it on hints and error messages, never on data: only
    /// a backticked command that starts with `cua ` is rewritten, which is how
    /// every hint spells one.
    pub fn localize_cli_hints(self, text: &str) -> std::borrow::Cow<'_, str> {
        let name = self.cli_name();
        if name == STABLE_CLI_NAME || !text.contains(CLI_HINT_PREFIX) {
            return std::borrow::Cow::Borrowed(text);
        }
        std::borrow::Cow::Owned(text.replace(CLI_HINT_PREFIX, &format!("`{name} ")))
    }

    /// Where the first run installs the CLI: `~/.local/bin/<cli_name>`.
    pub fn cli_path(self) -> PathBuf {
        user_home().join(".local").join("bin").join(self.cli_name())
    }

    // ----- coding agents: the MCP server and the skills --------------------

    /// The name the cua MCP server is registered under in a coding agent's
    /// config (Claude Code, Codex, ...): `cua`, or `cua-local` for Local, so
    /// the two channels never write the same entry. It launches
    /// [`CuaChannel::cli_name`] (`cua mcp`, `cua-local mcp`).
    pub fn mcp_server_name(self) -> &'static str {
        match self {
            Self::Stable => "cua",
            Self::Local => LOCAL_MCP_SERVER_NAME,
        }
    }

    /// The suffix a bundled skill's name takes in a coding agent's skills
    /// folder: none for Stable, `-local` for Local.
    pub fn skill_suffix(self) -> &'static str {
        match self {
            Self::Stable => "",
            Self::Local => LOCAL_SKILL_SUFFIX,
        }
    }

    /// The name the bundled skill `bundled` (`cua-spaces`) is installed
    /// under, as its folder and its `name`: `cua-spaces` for Stable,
    /// `cua-spaces-local` for Local. Local's copies sit beside Stable's and
    /// never replace them.
    pub fn skill_install_name(self, bundled: &str) -> String {
        format!("{bundled}{}", self.skill_suffix())
    }

    // ----- host: helper, runner, ports -------------------------------------

    /// The launchd label of the app helper (the login item that runs
    /// spacesd in the user's session).
    pub fn helper_label(self) -> &'static str {
        match self {
            Self::Stable => "com.trycua.spaces.helper",
            Self::Local => "com.trycua.spaces.local.helper",
        }
    }

    /// What the app helper runs: `<home_dir>/helper/service.json`.
    pub fn helper_spec_path(self) -> PathBuf {
        self.home_dir().join("helper").join("service.json")
    }

    /// The launchd label of the plain host runner (`cua host setup` without
    /// the app).
    pub fn host_label(self) -> &'static str {
        match self {
            Self::Stable => "com.trycua.spacesd.host",
            Self::Local => "com.trycua.spacesd.host.local",
        }
    }

    /// The spacesd TCP and QUIC ports, and the mesh media UDP port (the
    /// same for both channels, see [`Ports::mesh_udp`]).
    pub fn ports(self) -> Ports {
        match self {
            Self::Stable => Ports {
                tcp: 3211,
                quic: 3212,
                mesh_udp: 3212,
            },
            Self::Local => Ports {
                tcp: 3221,
                quic: 3222,
                mesh_udp: 3212,
            },
        }
    }

    // ----- Cua Volume ------------------------------------------------------

    /// The Cua Volume's name (and its macOS folder name).
    pub fn volume_name(self) -> &'static str {
        match self {
            Self::Stable => "Cua Volume",
            Self::Local => "Cua Volume Local",
        }
    }

    /// Where the Cua Volume mounts on macOS: `~/<volume_name>`.
    pub fn volume_mount_path(self) -> PathBuf {
        user_home().join(self.volume_name())
    }

    // ----- Lume ------------------------------------------------------------

    /// The Lume server's port.
    pub fn lume_port(self) -> u16 {
        match self {
            Self::Stable => 7777,
            Self::Local => 7787,
        }
    }

    /// The Lume server's URL (`LUME_API`'s default).
    pub fn lume_api(self) -> &'static str {
        match self {
            Self::Stable => "http://127.0.0.1:7777",
            Self::Local => "http://127.0.0.1:7787",
        }
    }

    /// Where Lume keeps its VMs: `~/.lume` or `~/.lume-local`.
    pub fn lume_dir(self) -> PathBuf {
        user_home().join(match self {
            Self::Stable => ".lume",
            Self::Local => ".lume-local",
        })
    }

    /// The launchd label of the Lume daemon.
    pub fn lume_daemon_label(self) -> &'static str {
        match self {
            Self::Stable => "com.trycua.lume_daemon",
            Self::Local => "com.trycua.lume_daemon.local",
        }
    }

    /// The Lume binary's file name: `lume` or `lume-local`.
    pub fn lume_bin_name(self) -> &'static str {
        match self {
            Self::Stable => "lume",
            Self::Local => "lume-local",
        }
    }
}

/// The paths this executable is known by, for [`CuaChannel::resolve`]:
///
/// - `current_exe()`, which is the file itself on Linux (the kernel resolves
///   symlinks), so a `cua-local` link to `target/debug/cua` loses its name;
/// - that path with symlinks resolved (`~/.local/bin/cua-local` may link
///   into the app bundle);
/// - the invocation path, `argv[0]` (a bare name resolved through `PATH`),
///   which keeps the name the process was started under, links included.
fn current_exe_paths() -> Vec<PathBuf> {
    let argv0 = std::env::args_os().next();
    let invoked = invocation_path(argv0.as_deref(), std::env::var_os("PATH").as_deref());
    let exe = std::env::current_exe().ok();
    let real = exe.as_ref().and_then(|e| e.canonicalize().ok());
    collect_exe_paths(exe, real, invoked)
}

/// The distinct paths among the three (see [`current_exe_paths`]).
fn collect_exe_paths(
    exe: Option<PathBuf>,
    real: Option<PathBuf>,
    invoked: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for p in [exe, real, invoked].into_iter().flatten() {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

/// Where the process was started from: `argv[0]` when it has a directory
/// part; a bare name is looked up in `path_var` like a shell does (the first
/// directory with a file of that name), and stays as the bare name when none
/// has one. `None` for an empty `argv[0]`.
fn invocation_path(argv0: Option<&OsStr>, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let argv0 = argv0.filter(|a| !a.is_empty())?;
    let arg = Path::new(argv0);
    if arg.components().count() > 1 {
        return Some(arg.to_path_buf());
    }
    let names: &[&str] = if cfg!(windows) { &["", ".exe"] } else { &[""] };
    let found = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .find_map(|dir| {
            names.iter().find_map(|ext| {
                let mut file = argv0.to_os_string();
                file.push(ext);
                Some(dir.join(file)).filter(|p| p.is_file())
            })
        });
    Some(found.unwrap_or_else(|| arg.to_path_buf()))
}

/// The user's home directory (`$HOME`, else `%USERPROFILE%`), the same
/// lookup [`crate::cua_home`] uses.
fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(p: &[&str]) -> Vec<PathBuf> {
        p.iter().map(PathBuf::from).collect()
    }

    const LOCAL_EXE: &str = "/Users/ada/Applications/Cua Spaces Local.app/Contents/MacOS/cua";
    const STABLE_EXE: &str = "/Applications/Cua Spaces.app/Contents/MacOS/cua";

    #[test]
    fn local_hints_name_the_local_cli_and_stable_hints_are_untouched() {
        let hint = "run `cua host setup` (or `cua auth login --json`); `cua` and cua-local stay";
        let stable = CuaChannel::Stable.localize_cli_hints(hint);
        assert!(matches!(stable, std::borrow::Cow::Borrowed(_)));
        assert_eq!(stable, hint);
        assert_eq!(
            CuaChannel::Local.localize_cli_hints(hint),
            "run `cua-local host setup` (or `cua-local auth login --json`); `cua` and cua-local stay"
        );
        // Already Local, no command, or not a backticked command: unchanged.
        for t in [
            "run `cua-local host setup`",
            "",
            "cua host setup",
            "ships next to `cua`",
        ] {
            assert_eq!(CuaChannel::Local.localize_cli_hints(t), t);
        }
    }

    #[test]
    fn parses_channel_names() {
        assert_eq!(CuaChannel::parse("stable"), Some(CuaChannel::Stable));
        assert_eq!(CuaChannel::parse(" LOCAL "), Some(CuaChannel::Local));
        assert_eq!(CuaChannel::parse(""), None);
        assert_eq!(CuaChannel::parse("prod"), None);
        assert_eq!(CuaChannel::parse("dev"), None);
        for c in CuaChannel::ALL {
            assert_eq!(CuaChannel::parse(c.name()), Some(c));
        }
    }

    #[test]
    fn executable_path_selects_local() {
        for p in [
            LOCAL_EXE,
            "/Users/ada/Applications/Cua Spaces Local.app/Contents/MacOS/cua-spacesd",
            "/Users/ada/.local/bin/cua-local",
            "C:/tools/cua-local.exe",
            "cua-local",
        ] {
            assert!(CuaChannel::path_is_local(Path::new(p)), "{p}");
        }
        for p in [
            STABLE_EXE,
            "/Users/ada/.local/bin/cua",
            "/usr/local/bin/cua-spacesd",
            // Only a whole path component or file name counts.
            "/Users/ada/Cua Spaces Local.app.zip/cua",
            "/Users/ada/my-cua-local/cua",
            "/Users/ada/.local/bin/cua-local-old",
            "/Users/ada/repo/target/debug/deps/cua_home-1a2b3c",
        ] {
            assert!(!CuaChannel::path_is_local(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn windows_names_are_case_insensitive() {
        // Windows compares file names without case: `CUA-LOCAL.EXE` is
        // `cua-local.exe`.
        for p in [
            "C:/tools/CUA-LOCAL.EXE",
            "C:/tools/Cua-Local.Exe",
            "C:/tools/cua-local.exe",
            "C:/tools/CUA-LOCAL",
            "C:/Users/ada/Applications/CUA SPACES LOCAL.APP/cua.exe",
        ] {
            assert!(CuaChannel::path_is_local_with(Path::new(p), true), "{p}");
        }
        for p in [
            "C:/tools/CUA.EXE",
            "C:/tools/CUA-LOCAL-OLD.EXE",
            "C:/tools/cua-local.exe.bak",
            "C:/Users/ada/Cua Spaces.app/cua.exe",
        ] {
            assert!(!CuaChannel::path_is_local_with(Path::new(p), true), "{p}");
        }
        // Elsewhere file names are case-sensitive.
        assert!(CuaChannel::path_is_local_with(
            Path::new("/bin/cua-local"),
            false
        ));
        for p in [
            "/bin/CUA-LOCAL",
            "/bin/Cua-Local.exe",
            "/x/CUA SPACES LOCAL.APP/cua",
        ] {
            assert!(!CuaChannel::path_is_local_with(Path::new(p), false), "{p}");
        }
        // This platform's rule is the one `path_is_local` applies.
        assert_eq!(
            CuaChannel::path_is_local(Path::new("/bin/CUA-LOCAL")),
            cfg!(windows)
        );
    }

    #[test]
    fn a_link_named_cua_local_stays_local_when_current_exe_resolves_it() {
        // Linux: `current_exe()` is the file, `target/debug/cua`, not the
        // `cua-local` link the process was started through.
        let real = PathBuf::from("/work/target/debug/cua");
        let link = PathBuf::from("/home/ada/.local/bin/cua-local");
        let known = collect_exe_paths(Some(real.clone()), Some(real.clone()), Some(link.clone()));
        assert_eq!(known, vec![real.clone(), link.clone()]);
        assert_eq!(
            CuaChannel::resolve(None, None, &known),
            Ok(CuaChannel::Local)
        );
        // Without the invocation path the name is lost.
        let lost = collect_exe_paths(Some(real.clone()), Some(real.clone()), None);
        assert_eq!(lost, vec![real.clone()]);
        assert_eq!(
            CuaChannel::resolve(None, None, &lost),
            Ok(CuaChannel::Stable)
        );
        // A Stable binary started as `cua` stays Stable.
        let stable = collect_exe_paths(Some(real.clone()), None, Some(PathBuf::from("cua")));
        assert_eq!(
            CuaChannel::resolve(None, None, &stable),
            Ok(CuaChannel::Stable)
        );
    }

    #[test]
    fn the_invocation_path_is_argv0_and_a_bare_name_is_looked_up_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let bin = dir.path().join(if cfg!(windows) {
            "cua-local.exe"
        } else {
            "cua-local"
        });
        std::fs::write(&bin, b"").unwrap();
        let path = std::env::join_paths([other.path(), dir.path()]).unwrap();

        // A bare name: the first PATH directory that has the file.
        let found = invocation_path(Some(OsStr::new("cua-local")), Some(path.as_os_str()));
        assert_eq!(found, Some(bin.clone()));
        // Not on PATH, or no PATH: the bare name stays (its file name still
        // says Local).
        let lost = invocation_path(
            Some(OsStr::new("cua-local")),
            Some(other.path().as_os_str()),
        );
        assert_eq!(lost, Some(PathBuf::from("cua-local")));
        assert_eq!(
            invocation_path(Some(OsStr::new("cua-local")), None),
            Some(PathBuf::from("cua-local"))
        );
        // A name with a directory part is used as it is, PATH or not.
        assert_eq!(
            invocation_path(Some(OsStr::new("./bin/cua-local")), Some(path.as_os_str())),
            Some(PathBuf::from("./bin/cua-local"))
        );
        assert_eq!(
            invocation_path(Some(bin.as_os_str()), None),
            Some(bin.clone())
        );
        // Nothing to go on.
        assert_eq!(
            invocation_path(Some(OsStr::new("")), Some(path.as_os_str())),
            None
        );
        assert_eq!(invocation_path(None, Some(path.as_os_str())), None);
    }

    #[test]
    fn nothing_set_is_stable() {
        let r = |env: Option<&str>, build: Option<&str>, exe: &[&str]| {
            CuaChannel::resolve(env, build, &paths(exe))
        };
        assert_eq!(r(None, None, &[]), Ok(CuaChannel::Stable));
        assert_eq!(r(None, None, &[STABLE_EXE]), Ok(CuaChannel::Stable));
        assert_eq!(r(Some(""), Some(""), &[STABLE_EXE]), Ok(CuaChannel::Stable));
    }

    #[test]
    fn resolution_order_is_env_then_build_default_then_path() {
        let r = |env: Option<&str>, build: Option<&str>, exe: &[&str]| {
            CuaChannel::resolve(env, build, &paths(exe))
        };
        use CuaChannel::{Local, Stable};

        // 3. The path alone.
        assert_eq!(r(None, None, &[LOCAL_EXE]), Ok(Local));
        // Either known path of the executable counts (a symlink's name, or
        // the file it resolves to).
        assert_eq!(
            r(None, None, &["/x/target/debug/cua", "/x/bin/cua-local"]),
            Ok(Local)
        );

        // 2. The compile-time default beats the path, both ways.
        assert_eq!(r(None, Some("local"), &[STABLE_EXE]), Ok(Local));
        assert_eq!(r(None, Some("stable"), &[LOCAL_EXE]), Ok(Stable));
        // An unknown compile-time value is ignored, the path decides.
        assert_eq!(r(None, Some("dev"), &[LOCAL_EXE]), Ok(Local));
        assert_eq!(r(None, Some("dev"), &[STABLE_EXE]), Ok(Stable));

        // 1. CUA_CHANNEL beats the compile-time default and the path.
        assert_eq!(r(Some("stable"), Some("local"), &[LOCAL_EXE]), Ok(Stable));
        assert_eq!(r(Some("local"), Some("stable"), &[STABLE_EXE]), Ok(Local));
        assert_eq!(r(Some(" Local "), None, &[STABLE_EXE]), Ok(Local));
        // Blank means unset.
        assert_eq!(r(Some("  "), Some("local"), &[]), Ok(Local));
        assert_eq!(r(Some(""), None, &[LOCAL_EXE]), Ok(Local));
    }

    #[test]
    fn unknown_channel_value_is_an_error_not_stable() {
        let err =
            CuaChannel::resolve(Some("dev"), Some("local"), &paths(&[LOCAL_EXE])).unwrap_err();
        assert!(
            err.contains("CUA_CHANNEL") && err.contains("stable") && err.contains("local"),
            "{err}"
        );
    }

    #[test]
    fn this_test_binary_is_stable() {
        // A cargo test binary is not a Local executable, and the test
        // environment does not set CUA_CHANNEL.
        if std::env::var_os(CUA_CHANNEL_VAR).is_none() && CuaChannel::build_default().is_none() {
            assert_eq!(CuaChannel::current(), CuaChannel::Stable);
        }
    }

    #[test]
    fn local_defaults_to_staging_and_prod_is_an_opt_in() {
        use CuaChannel::Local;
        // Unset and blank: staging.
        assert_eq!(Local.default_env(), CuaEnv::Staging);
        assert_eq!(Local.env_from_value(None), Ok(CuaEnv::Staging));
        assert_eq!(Local.env_from_value(Some("")), Ok(CuaEnv::Staging));
        assert_eq!(Local.env_from_value(Some("  ")), Ok(CuaEnv::Staging));
        // CUA_ENV set: it wins, prod included.
        assert_eq!(Local.env_from_value(Some("prod")), Ok(CuaEnv::Prod));
        assert_eq!(Local.env_from_value(Some("Production")), Ok(CuaEnv::Prod));
        assert_eq!(Local.env_from_value(Some("staging")), Ok(CuaEnv::Staging));
        // A typo is an error, never production.
        let err = Local.env_from_value(Some("prd")).unwrap_err();
        assert!(err.contains("CUA_ENV") && err.contains("staging"), "{err}");
    }

    #[test]
    fn an_unknown_environment_is_answered_by_channel() {
        use CuaChannel::{Local, Stable};
        for v in [Some("prd"), Some("stagign"), Some("dev")] {
            assert_eq!(Stable.env_or_fallback(v), CuaEnv::Prod, "{v:?}");
            assert_eq!(Local.env_or_fallback(v), CuaEnv::Staging, "{v:?}");
        }
        // A known or unset value is exactly `env_from_value`.
        for v in [None, Some(""), Some("prod"), Some("staging")] {
            assert_eq!(Local.env_or_fallback(v), Local.env_from_value(v).unwrap());
            assert_eq!(Stable.env_or_fallback(v), Stable.env_from_value(v).unwrap());
        }
    }

    #[test]
    fn stable_environment_is_unchanged() {
        use CuaChannel::Stable;
        assert_eq!(Stable.default_env(), CuaEnv::compiled_default());
        for v in [None, Some(""), Some("prod"), Some("staging"), Some("dev")] {
            assert_eq!(
                Stable.env_from_value(v),
                CuaEnv::stable_from_value(v),
                "{v:?}"
            );
        }
    }

    #[test]
    fn stable_names_are_todays_names() {
        let s = CuaChannel::Stable;
        // The Stable bundle id is renamed to com.trycua.spaces in a later PR.
        assert_eq!(s.bundle_id(), "com.trycua.spaces.macos");
        assert_eq!(s.legacy_bundle_id(), None);
        assert_eq!(s.app_name(), "Cua Spaces");
        assert_eq!(s.app_path(), Path::new("/Applications/Cua Spaces.app"));
        assert_eq!(s.app_support_dir_name(), "com.trycua.spaces.macos");
        assert_eq!(s.cli_name(), "cua");
        assert_eq!(s.cli_path(), user_home().join(".local/bin/cua"));
        assert_eq!(s.home_dir(), user_home().join(".cua"));
        assert_eq!(
            s.helper_spec_path(),
            user_home().join(".cua/helper/service.json")
        );
        assert_eq!(s.helper_label(), "com.trycua.spaces.helper");
        assert_eq!(s.host_label(), "com.trycua.spacesd.host");
        assert_eq!(
            s.ports(),
            Ports {
                tcp: 3211,
                quic: 3212,
                mesh_udp: 3212
            }
        );
        assert_eq!(s.url_scheme(), "cua");
        assert_eq!(s.volume_name(), "Cua Volume");
        assert_eq!(s.volume_mount_path(), user_home().join("Cua Volume"));
        assert_eq!(s.lume_api(), "http://127.0.0.1:7777");
        assert_eq!(s.lume_dir(), user_home().join(".lume"));
        assert_eq!(s.lume_daemon_label(), "com.trycua.lume_daemon");
        assert_eq!(s.lume_bin_name(), "lume");
        assert_eq!(s.telemetry_channel(), "prod");
        for env in [CuaEnv::Prod, CuaEnv::Staging] {
            assert_eq!(s.keychain_service(env), env.keyring_service());
            assert_eq!(
                s.legacy_keychain_service(env),
                Some(env.legacy_keyring_service())
            );
            assert_eq!(s.home_subdir(env), env.home_subdir());
        }
        assert_eq!(s.home_dir_name(), ".cua");
        assert_eq!(s.env_home_dir(CuaEnv::Prod), user_home().join(".cua"));
        assert_eq!(
            s.env_home_dir(CuaEnv::Staging),
            user_home().join(".cua/envs/staging")
        );
    }

    #[test]
    fn local_names_are_the_designed_names() {
        let l = CuaChannel::Local;
        assert_eq!(l.bundle_id(), "com.trycua.spaces.macos.local");
        assert_eq!(l.legacy_bundle_id(), Some("com.trycua.spaces.local"));
        assert_eq!(l.app_name(), "Cua Spaces Local");
        assert_eq!(
            l.app_path(),
            user_home().join("Applications/Cua Spaces Local.app")
        );
        assert_eq!(l.app_support_dir_name(), "com.trycua.spaces.macos.local");
        assert_eq!(
            l.app_support_dir(),
            user_home().join("Library/Application Support/com.trycua.spaces.macos.local")
        );
        assert_eq!(l.cli_name(), "cua-local");
        assert_eq!(l.cli_path(), user_home().join(".local/bin/cua-local"));
        assert_eq!(l.home_dir(), user_home().join(".cua-local"));
        assert_eq!(
            l.helper_spec_path(),
            user_home().join(".cua-local/helper/service.json")
        );
        assert_eq!(l.helper_label(), "com.trycua.spaces.local.helper");
        assert_eq!(l.host_label(), "com.trycua.spacesd.host.local");
        assert_eq!(
            l.ports(),
            Ports {
                tcp: 3221,
                quic: 3222,
                mesh_udp: 3212
            }
        );
        assert_eq!(l.url_scheme(), "cua-local");
        assert_eq!(l.volume_name(), "Cua Volume Local");
        assert_eq!(l.volume_mount_path(), user_home().join("Cua Volume Local"));
        assert_eq!(l.lume_api(), "http://127.0.0.1:7787");
        assert_eq!(l.lume_dir(), user_home().join(".lume-local"));
        assert_eq!(l.lume_daemon_label(), "com.trycua.lume_daemon.local");
        assert_eq!(l.lume_bin_name(), "lume-local");
        assert_eq!(l.telemetry_channel(), "local");
        assert_eq!(l.keychain_service(CuaEnv::Prod), "ai.cua.session.local");
        assert_eq!(
            l.keychain_service(CuaEnv::Staging),
            "ai.cua.session.local.staging"
        );
        // Local is new: no legacy Keychain service to move from.
        for env in [CuaEnv::Prod, CuaEnv::Staging] {
            assert_eq!(l.legacy_keychain_service(env), None);
        }
    }

    #[test]
    fn the_keyvault_link_follows_the_channel_scheme() {
        assert_eq!(CuaChannel::Stable.keyvault_url(), "cua://keyvault");
        assert_eq!(CuaChannel::Local.keyvault_url(), "cua-local://keyvault");
        for c in CuaChannel::ALL {
            assert!(
                c.keyvault_url()
                    .starts_with(&format!("{}://", c.url_scheme()))
            );
        }
        // This test binary is Stable unless the environment says otherwise.
        if std::env::var_os(CUA_CHANNEL_VAR).is_none() && CuaChannel::build_default().is_none() {
            assert_eq!(CuaChannel::current_keyvault_url(), "cua://keyvault");
        }
    }

    #[test]
    fn local_prod_has_a_home_of_its_own_below_local() {
        let l = CuaChannel::Local;
        assert_eq!(l.home_dir_name(), ".cua-local");
        // Local's default environment is staging: the home itself.
        assert_eq!(l.home_subdir(CuaEnv::Staging), None);
        assert_eq!(
            l.env_home_dir(CuaEnv::Staging),
            user_home().join(".cua-local")
        );
        // Production is the opt-in, one level down, as Stable's staging is.
        assert_eq!(l.home_subdir(CuaEnv::Prod), Some("envs/prod"));
        assert_eq!(
            l.env_home_dir(CuaEnv::Prod),
            user_home().join(".cua-local/envs/prod")
        );
        // The channel base is unchanged by the environment (the helper's
        // spec lives there).
        assert_eq!(l.home_dir(), user_home().join(".cua-local"));
    }

    #[test]
    fn no_channel_and_environment_pair_shares_a_home_or_a_keychain_service() {
        let pairs: Vec<(CuaChannel, CuaEnv)> = CuaChannel::ALL
            .into_iter()
            .flat_map(|c| [CuaEnv::Prod, CuaEnv::Staging].map(|e| (c, e)))
            .collect();
        assert_eq!(pairs.len(), 4);
        for (i, (c1, e1)) in pairs.iter().enumerate() {
            for (c2, e2) in &pairs[i + 1..] {
                let what = format!("{c1:?}/{e1:?} and {c2:?}/{e2:?}");
                assert_ne!(c1.env_home_dir(*e1), c2.env_home_dir(*e2), "{what}");
                assert_ne!(c1.keychain_service(*e1), c2.keychain_service(*e2), "{what}");
                // Across channels no home is inside the other's.
                if c1 != c2 {
                    let (h1, h2) = (c1.env_home_dir(*e1), c2.env_home_dir(*e2));
                    assert!(!h1.starts_with(&h2) && !h2.starts_with(&h1), "{what}");
                }
            }
        }
    }

    #[test]
    fn coding_agents_get_names_of_the_channel() {
        assert_eq!(CuaChannel::Stable.mcp_server_name(), "cua");
        assert_eq!(CuaChannel::Local.mcp_server_name(), "cua-local");
        assert_eq!(
            CuaChannel::Stable.skill_install_name("cua-spaces"),
            "cua-spaces"
        );
        assert_eq!(
            CuaChannel::Local.skill_install_name("cua-spaces"),
            "cua-spaces-local"
        );
        // The server is the channel's CLI, so the entry launches its own.
        for c in CuaChannel::ALL {
            assert_eq!(c.mcp_server_name(), c.cli_name());
        }
        // Nothing the two channels register in a coding agent is shared.
        for skill in ["cua-spaces", "cua-driver", "gui-automation"] {
            assert_ne!(
                CuaChannel::Stable.skill_install_name(skill),
                CuaChannel::Local.skill_install_name(skill)
            );
        }
        assert_ne!(
            CuaChannel::Stable.mcp_server_name(),
            CuaChannel::Local.mcp_server_name()
        );
    }

    #[test]
    fn lume_api_names_its_port() {
        for c in CuaChannel::ALL {
            assert_eq!(c.lume_api(), format!("http://127.0.0.1:{}", c.lume_port()));
        }
    }

    #[test]
    fn local_executable_resolves_to_local_names() {
        // The CLI name and the Local app path agree with path detection.
        let l = CuaChannel::Local;
        assert!(CuaChannel::path_is_local(&l.cli_path()));
        assert!(CuaChannel::path_is_local(
            &l.app_path().join("Contents/MacOS/cua")
        ));
        let s = CuaChannel::Stable;
        assert!(!CuaChannel::path_is_local(&s.cli_path()));
        assert!(!CuaChannel::path_is_local(
            &s.app_path().join("Contents/MacOS/cua")
        ));
    }
}
