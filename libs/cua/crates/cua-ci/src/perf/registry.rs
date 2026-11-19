//! Registry mirrors for Docker pulls inside a Linux job.
//!
//! Anonymous pulls from Docker Hub are rate limited by address, and the
//! addresses of cloud computers (and of one packed host running many jobs)
//! share that allowance: in the measured AWS series 7 of 20 runs of a job
//! that pulls five images failed with `429 Too Many Requests`. A
//! pull-through mirror answers repeated pulls itself and falls back to the
//! hub for what it does not hold, so a pull neither fails nor waits on the
//! hub's limit.
//!
//! What this module does, and what it does not:
//!
//! - It tells a Linux guest's Docker daemon to try the configured mirrors
//!   first (`registry-mirrors` in `/etc/docker/daemon.json`). The daemon
//!   itself falls back to Docker Hub when a mirror fails, so a bad mirror
//!   costs a pull's time, not the job.
//! - Docker verifies image digests whoever served the bytes, so a mirror
//!   cannot change what a digest-pinned pull gets.
//! - No credential ever goes into the VM: a mirror URL with a user name
//!   or password in it is refused. Authenticated pulls to lift Docker
//!   Hub's limit belong on the host side (a pull-through cache you run,
//!   listed here and allowed in `egress_allow`) or in the workflow
//!   (`docker/login-action` with a GitHub secret), never in this file.
//! - It does not touch macOS guests (no Docker there) and it does not
//!   rewrite image names, so `docker pull ghcr.io/...` is unaffected.

use serde::{Deserialize, Serialize};

use crate::config::GuestOs;
use crate::{Error, Result};

/// Google's public pull-through cache of Docker Hub. It needs no account
/// and serves the popular and official images.
pub const DEFAULT_MIRROR: &str = "https://mirror.gcr.io";

fn d_mirrors() -> Vec<String> {
    vec![DEFAULT_MIRROR.to_string()]
}

/// The `[perf.registry]` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryConfig {
    /// Docker Hub mirrors, tried in order before the hub itself (default:
    /// Google's public cache). An empty list turns the feature off.
    #[serde(default = "d_mirrors")]
    pub mirrors: Vec<String>,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            mirrors: d_mirrors(),
        }
    }
}

/// Most mirrors a pool may list (the daemon tries them in order).
pub const MAX_MIRRORS: usize = 4;

impl RegistryConfig {
    /// Checks every mirror.
    pub fn validate(&self) -> Result<()> {
        if self.mirrors.len() > MAX_MIRRORS {
            return Err(Error::Config(format!(
                "perf.registry.mirrors: at most {MAX_MIRRORS}"
            )));
        }
        for m in &self.mirrors {
            check_mirror(m)
                .map_err(|e| Error::Config(format!("perf.registry.mirrors {m:?}: {e}")))?;
        }
        Ok(())
    }

    /// The mirrors a guest of `os` gets (none on macOS, where there is no
    /// Docker).
    pub fn for_guest(&self, os: GuestOs) -> &[String] {
        match os {
            GuestOs::Linux => &self.mirrors,
            GuestOs::Macos => &[],
        }
    }
}

/// A mirror is an `http` or `https` origin: a host, an optional port, no
/// user name or password (that would be a credential in the VM), no path,
/// query or fragment, and only characters that are safe in a shell word.
pub fn check_mirror(m: &str) -> std::result::Result<(), String> {
    let (scheme, rest) = m
        .split_once("://")
        .ok_or("write it as https://host[:port]")?;
    if scheme != "https" && scheme != "http" {
        return Err("only http or https".into());
    }
    if rest.contains('@') {
        return Err("no user name or password in it (credentials never enter a VM)".into());
    }
    let host_port = rest.trim_end_matches('/');
    if host_port.is_empty()
        || host_port.contains(['/', '?', '#'])
        || !host_port
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return Err("a host and an optional port only, no path".into());
    }
    if let Some((_, port)) = host_port.rsplit_once(':')
        && !port.starts_with(']')
        && !(port.parse::<u16>().is_ok_and(|p| p > 0))
    {
        return Err("the port is not a number".into());
    }
    Ok(())
}

/// The line `job.env` carries (shell syntax), empty when there is nothing
/// to configure. The mirrors are validated words, quoted anyway.
pub fn env_line(mirrors: &[String]) -> String {
    if mirrors.is_empty() {
        return String::new();
    }
    format!(
        "CUA_CI_REGISTRY_MIRRORS={}\n",
        crate::guest::sh_quote(&mirrors.join(" "))
    )
}

/// The shell the job-start hook runs: when mirrors are set and Docker is
/// installed, put them in the daemon's configuration (keeping everything
/// else in it) and ask a running daemon to reload. Best effort and silent:
/// it never fails the job. A daemon that is not running yet reads the file
/// when it starts.
pub fn hook_snippet() -> &'static str {
    r#"if [ -n "${CUA_CI_REGISTRY_MIRRORS:-}" ] && command -v docker >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
  t0=$(ms)
  sudo -n mkdir -p /etc/docker
  sudo -n env MIRRORS="$CUA_CI_REGISTRY_MIRRORS" python3 - <<'PY' 2>/dev/null
import json, os
p = "/etc/docker/daemon.json"
try:
    cfg = json.load(open(p))
except Exception:
    cfg = {}
if not isinstance(cfg, dict):
    cfg = {}
want = os.environ.get("MIRRORS", "").split()
if want and cfg.get("registry-mirrors") != want:
    cfg["registry-mirrors"] = want
    tmp = p + ".cua-tmp"
    json.dump(cfg, open(tmp, "w"), indent=2)
    os.replace(tmp, p)
    open("/run/cua-ci-docker-reload", "w").write("1")
PY
  if [ -e /run/cua-ci-docker-reload ]; then
    sudo -n rm -f /run/cua-ci-docker-reload
    sudo -n systemctl reload docker >/dev/null 2>&1 || sudo -n pkill -HUP dockerd >/dev/null 2>&1
  fi
  t1=$(ms)
  printf '{"k":"registry_mirror","at":%s,"ms":%s}\n' "$t1" "$((t1 - t0))" >> "$D/timers.jsonl"
fi
"#
}

#[cfg(test)]
mod tests;
