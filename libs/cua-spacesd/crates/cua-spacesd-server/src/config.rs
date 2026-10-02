// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Server configuration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

/// Default path of the token file.
pub const DEFAULT_TOKEN_FILE: &str = "/run/cua/env-token";

/// Largest `bytes` payload accepted in one chunk message.
pub const MAX_CHUNK_BYTES: u32 = 4 * 1024 * 1024;
/// Largest encoded gRPC message accepted or sent.
pub const MAX_MESSAGE_BYTES: u32 = 8 * 1024 * 1024;
/// Recommended bulk transfer chunk size.
pub const PREFERRED_CHUNK_BYTES: u32 = 1024 * 1024;
/// Default per-process scrollback ring.
pub const DEFAULT_SCROLLBACK_BYTES: u64 = 8 * 1024 * 1024;
/// Largest per-process scrollback ring a client may request.
pub const MAX_SCROLLBACK_BYTES: u64 = 512 * 1024 * 1024;

/// Static configuration of one server instance.
#[derive(Debug, Clone, Serialize)]
pub struct ServerConfig {
    /// TCP address for gRPC, gRPC-Web and HTTP.
    pub listen: SocketAddr,
    /// Where the token was read from, for `--print-config` (never the value).
    pub token_source: String,
    /// Allow a non-loopback bind without a token, serving only
    /// `GetCapabilities`, `Health` and `Init` until `Init` installs one.
    pub insecure_bootstrap: bool,
    /// Where the first bootstrap `Init` persists its token (0600), so a
    /// driver restart keeps it. `None`: memory only.
    pub bootstrap_token_file: Option<PathBuf>,
    /// Await-token-file mode: the token comes only from this file, polled
    /// for changes (install, rotate, revoke). Non-loopback binds are allowed
    /// without a token; only `GetCapabilities` and `Health` answer until the
    /// file holds one. `Init` never sets the token in this mode.
    pub await_token_file: Option<PathBuf>,
    /// Poll interval of the token file.
    #[serde(with = "secs")]
    pub token_poll_interval: Duration,
    /// Accept a world-accessible token file (default: only as root in a
    /// container, see `token_file::default_allow_world_readable`).
    pub token_file_allow_world_readable: bool,
    /// Directory for driver state (upload staging, CA bundle).
    pub data_dir: PathBuf,
    /// Default scrollback ring size per process.
    pub default_scrollback_bytes: u64,
    /// Serve `/mcp` (streamable-HTTP MCP) when a tool registry is available.
    pub enable_mcp: bool,
    /// Allow `Shutdown` with `GUEST_POWEROFF` / `GUEST_REBOOT`.
    pub allow_guest_power: bool,
    /// Report this runtime instead of detecting it (`kubevirt`, `gvisor`,
    /// `lume`, `qemu`, `container`, `bare`, `hyperv`).
    pub runtime_override: Option<String>,
    /// Report this OS family (`linux`, `macos`, `windows`) instead of the
    /// build target's. Only for loopback test fixtures that stand in for a
    /// Space of another OS; not a command-line option.
    pub os_override: Option<String>,
    /// UDP port of the direct QUIC media listener (0 = disabled). Reported in
    /// `SideChannels`; the listener itself belongs to the desktop provider.
    pub media_quic_port: u16,
    /// Teleport destination root (default `~/Downloads`).
    pub downloads_dir: Option<PathBuf>,
    /// Home directory app-session imports land in (default `$HOME`).
    pub teleport_home: Option<PathBuf>,
    /// Where import ledgers live (default `/run/cua/teleport-ledger` when
    /// `/run/cua` is writable, else `<data_dir>/teleport/ledger`).
    pub teleport_ledger_dir: Option<PathBuf>,
    /// Force the polling filesystem watcher (for filesystems without inotify
    /// / FSEvents support, e.g. some gVisor and network mounts).
    pub force_poll_watcher: bool,
    /// HTTP/2 PING interval (and TCP keepalive) for idle connections.
    #[serde(with = "secs")]
    pub keepalive_interval: Duration,
    /// Grace period for in-flight requests on shutdown.
    #[serde(with = "secs")]
    pub shutdown_grace: Duration,
    /// Initial `PresenceSettings.cursor_probe`: presence may read the real
    /// cursor shape at a participant's position by briefly moving the idle
    /// guest pointer there and back. `SystemService.Init` can change it.
    pub cursor_probe: bool,
    /// Record every authorized remote access in `<data_dir>/access.log`
    /// (see [`crate::access_log`]). Off by default so embedders and tests
    /// never write into a real data dir; the `cua-spacesd` binary turns it
    /// on unless `--no-access-log`.
    pub access_log: bool,
}

impl ServerConfig {
    /// `<data_dir>/access.log`.
    pub fn access_log_path(&self) -> PathBuf {
        self.data_dir.join(ACCESS_LOG_FILE)
    }
}

/// The access log's file name in the data dir.
pub const ACCESS_LOG_FILE: &str = "access.log";

mod secs {
    pub fn serialize<S: serde::Serializer>(
        d: &std::time::Duration,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], cua_proto::SPACESD_DEFAULT_PORT)),
            token_source: "none".into(),
            insecure_bootstrap: false,
            bootstrap_token_file: None,
            await_token_file: None,
            token_poll_interval: crate::token_file::DEFAULT_POLL_INTERVAL,
            token_file_allow_world_readable: false,
            data_dir: default_data_dir(),
            default_scrollback_bytes: DEFAULT_SCROLLBACK_BYTES,
            enable_mcp: true,
            allow_guest_power: false,
            runtime_override: None,
            os_override: None,
            media_quic_port: 0,
            downloads_dir: None,
            teleport_home: None,
            teleport_ledger_dir: None,
            force_poll_watcher: false,
            keepalive_interval: Duration::from_secs(20),
            shutdown_grace: Duration::from_secs(5),
            cursor_probe: true,
            access_log: false,
        }
    }
}

/// `~/.cua/spacesd` (or a temp dir when there is no home). A machine that
/// only has an older data dir (`~/.cua/guestd`, else `~/.cua/env-driver`)
/// keeps using it, so its machine id and state survive the upgrade.
pub fn default_data_dir() -> PathBuf {
    match home_dir() {
        Some(home) => data_dir_in(&home.join(".cua")),
        None => std::env::temp_dir().join("cua-spacesd"),
    }
}

fn data_dir_in(cua: &std::path::Path) -> PathBuf {
    let dir = cua.join("spacesd");
    if dir.exists() {
        return dir;
    }
    ["guestd", "env-driver"]
        .iter()
        .map(|legacy| cua.join(legacy))
        .find(|legacy| legacy.is_dir())
        .unwrap_or(dir)
}

/// The current user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Where a token was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToken {
    /// The token (non-empty).
    pub token: String,
    /// `"flag"`, `"env:CUA_ENV_TOKEN"` or `"file:<path>"`.
    pub source: String,
}

/// Resolves the token: explicit value, then `CUA_ENV_TOKEN`, then the token
/// file. Empty values are ignored; surrounding whitespace in the file is
/// trimmed.
pub fn resolve_token(
    explicit: Option<&str>,
    env: Option<&str>,
    file: &std::path::Path,
) -> std::io::Result<Option<ResolvedToken>> {
    if let Some(token) = explicit.map(str::trim).filter(|t| !t.is_empty()) {
        return Ok(Some(ResolvedToken {
            token: token.to_owned(),
            source: "flag".into(),
        }));
    }
    if let Some(token) = env.map(str::trim).filter(|t| !t.is_empty()) {
        return Ok(Some(ResolvedToken {
            token: token.to_owned(),
            source: "env:CUA_ENV_TOKEN".into(),
        }));
    }
    match std::fs::read_to_string(file) {
        Ok(contents) => {
            let token = contents.trim();
            Ok((!token.is_empty()).then(|| ResolvedToken {
                token: token.to_owned(),
                source: format!("file:{}", file.display()),
            }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(error),
        Err(error) => Err(error),
    }
}

/// Default listen address: every interface when a token is configured,
/// loopback otherwise.
pub fn default_listen(has_token: bool, port: u16) -> SocketAddr {
    if has_token {
        SocketAddr::from(([0, 0, 0, 0], port))
    } else {
        SocketAddr::from(([127, 0, 0, 1], port))
    }
}

/// Refuses a non-loopback bind without a token unless bootstrap or
/// await-token-file mode is on (`deferred_token`).
pub fn check_bind_policy(
    listen: SocketAddr,
    has_token: bool,
    deferred_token: bool,
) -> Result<(), String> {
    if listen.ip().is_loopback() || has_token || deferred_token {
        return Ok(());
    }
    Err(format!(
        "refusing to listen on non-loopback address {listen} without a token: set CUA_ENV_TOKEN, \
         write {DEFAULT_TOKEN_FILE}, pass --token, or bind 127.0.0.1"
    ))
}

#[cfg(test)]
mod tests {

    #[test]
    fn data_dir_prefers_spacesd_and_keeps_a_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cua = tmp.path();
        assert_eq!(super::data_dir_in(cua), cua.join("spacesd"));
        std::fs::create_dir_all(cua.join("env-driver")).unwrap();
        assert_eq!(super::data_dir_in(cua), cua.join("env-driver"));
        std::fs::create_dir_all(cua.join("guestd")).unwrap();
        assert_eq!(super::data_dir_in(cua), cua.join("guestd"));
        std::fs::create_dir_all(cua.join("spacesd")).unwrap();
        assert_eq!(super::data_dir_in(cua), cua.join("spacesd"));
    }

    use super::*;

    #[test]
    fn non_loopback_without_token_is_refused() {
        let any: SocketAddr = "0.0.0.0:3211".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:3211".parse().unwrap();
        let lo6: SocketAddr = "[::1]:3211".parse().unwrap();
        assert!(check_bind_policy(any, false, false).is_err());
        assert!(check_bind_policy(any, true, false).is_ok());
        assert!(check_bind_policy(any, false, true).is_ok());
        assert!(check_bind_policy(lo, false, false).is_ok());
        assert!(check_bind_policy(lo6, false, false).is_ok());
    }

    #[test]
    fn default_listen_depends_on_token() {
        assert_eq!(default_listen(true, 3211).to_string(), "0.0.0.0:3211");
        assert_eq!(default_listen(false, 3211).to_string(), "127.0.0.1:3211");
    }

    #[test]
    fn token_resolution_order() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("env-token");
        assert_eq!(resolve_token(None, None, &file).unwrap(), None);
        std::fs::write(&file, "  from-file\n").unwrap();
        assert_eq!(
            resolve_token(None, None, &file).unwrap().unwrap().token,
            "from-file"
        );
        assert_eq!(
            resolve_token(None, Some("from-env"), &file)
                .unwrap()
                .unwrap()
                .source,
            "env:CUA_ENV_TOKEN"
        );
        assert_eq!(
            resolve_token(Some("flag"), Some("from-env"), &file)
                .unwrap()
                .unwrap()
                .token,
            "flag"
        );
        assert_eq!(
            resolve_token(Some(""), Some(" "), &file)
                .unwrap()
                .unwrap()
                .token,
            "from-file"
        );
    }
}
