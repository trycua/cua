//! Overlays: put freshly built binaries into a running sandbox, so tests
//! always exercise the code under test and never a copy bundled in the
//! image (`cua sb create --overlay cua-driver=PATH`, `Sandbox.overlay`).
//!
//! For each overlay, as root in the guest: through the container engine for
//! local container sandboxes (`docker exec -u 0`, no in-guest
//! agent needed), else through cua-spacesd (as root, or with passwordless
//! sudo):
//!
//! 1. copy the file to a temporary guest path and check its sha256;
//! 2. copy it next to the target and `rename(2)` it over the target (atomic:
//!    a reader sees the old or the new file, never a partial one; running
//!    processes keep their old executable);
//! 3. record it in `/var/lib/cua/overlays/<name>.json` (path, sha256, the
//!    previous sha256, source, time), which `cua-spacesd doctor` checks as
//!    `build.overlay.<name>`;
//! 4. restart what runs it. `cua-spacesd` (the daemon this goes through) is
//!    restarted through its supervisor (supervisord program, systemd unit, or
//!    a signal and the supervisor's restart policy) in a detached session,
//!    then this waits until a new daemon process runs the injected
//!    executable. For any other name, processes still running an older copy
//!    of that executable are stopped (a supervisor, or the next call,
//!    starts the new one).
//!
//! Known names resolve their guest path: `cua-spacesd` is the executable of
//! the running daemon (`cua-guestd` and `cua-env-driver` in older images),
//! `cua-driver` the one on PATH (else `/usr/local/bin/cua-driver`). Other
//! names need `target`.

use super::{Sandbox, SpacesdClient, run};
use crate::{CuaError, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where overlay records live in the guest.
pub const OVERLAY_RECORD_DIR: &str = "/var/lib/cua/overlays";

/// A binary to inject into a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Overlay {
    /// `cua-driver`, `cua-spacesd`, or any name (then `target` is required).
    /// Letters, digits, `.`, `_`, `-`.
    pub name: String,
    /// The local file to inject.
    pub path: String,
    /// Guest path to replace. Default: resolved for `cua-driver` and
    /// `cua-spacesd`.
    #[uniffi(default = None)]
    pub target: Option<String>,
    /// Provenance recorded with it (for example the git sha it was built
    /// from). Default: the local path.
    #[uniffi(default = None)]
    pub source: Option<String>,
}

impl Overlay {
    /// `NAME=PATH` or `NAME=PATH:GUEST_PATH` (the CLI's `--overlay`).
    pub fn parse(spec: &str) -> Result<Overlay> {
        let (name, rest) = spec.split_once('=').ok_or_else(|| {
            CuaError::InvalidArgument(format!(
                "--overlay {spec:?}: use NAME=PATH or NAME=PATH:GUEST_PATH, e.g. cua-driver=./target/release/cua-driver"
            ))
        })?;
        // A guest path is absolute: split at the last ":/" (not a Windows
        // drive letter, `C:/...`).
        let drive = |i: usize| i == 1 && rest.as_bytes()[0].is_ascii_alphabetic();
        let (path, target) = match rest.rfind(":/") {
            Some(i) if i > 0 && !drive(i) => (&rest[..i], Some(rest[i + 1..].to_string())),
            _ => (rest, None),
        };
        let o = Overlay {
            name: name.trim().to_string(),
            path: path.to_string(),
            target,
            source: None,
        };
        o.validate()?;
        Ok(o)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let bad = |m: String| {
            Err(CuaError::InvalidArgument(format!(
                "overlay {}: {m}",
                self.name
            )))
        };
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            || self.name.starts_with('.')
        {
            return bad("the name must be 1 to 64 letters, digits, '.', '_' or '-'".into());
        }
        if self.path.is_empty() {
            return bad("no local path".into());
        }
        if let Some(t) = &self.target {
            if !t.starts_with('/') || t.ends_with('/') || !safe_text(t) || t.contains("/../") {
                return bad(format!("guest path {t:?} must be an absolute file path"));
            }
        } else if !matches!(self.name.as_str(), "cua-driver" | "cua-spacesd") {
            return bad("give the guest path: NAME=PATH:GUEST_PATH".into());
        }
        if let Some(s) = &self.source
            && !safe_text(s)
        {
            return bad("the source must be printable text without quotes or backslashes".into());
        }
        Ok(())
    }
}

/// What an overlay did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OverlayResult {
    /// Overlay name.
    pub name: String,
    /// Guest path that was replaced.
    pub target: String,
    /// sha256 (hex) of the injected file (pass it to `cua doctor --expect
    /// NAME=sha256:<hex>`).
    pub sha256: String,
    /// sha256 of the file it replaced, empty when there was none.
    pub previous_sha256: String,
    /// Bytes.
    pub size: u64,
    /// How the running program was restarted (`supervisor:cua-spacesd`,
    /// `systemd:cua-spacesd.service`, `stopped:<pids>`), empty when nothing
    /// ran it.
    pub restarted: String,
}

/// Printable, no quotes, backslashes or control characters (it is written
/// into a JSON record by a shell script).
fn safe_text(s: &str) -> bool {
    s.len() <= 1024
        && s.chars()
            .all(|c| !c.is_control() && c != '"' && c != '\\' && c != '\'')
}

/// Shell single-quoting.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell helpers. `cua_exe PID` prints a process's executable (from
/// `/proc/PID/exe`, else its argv[0]: root in gVisor may not read another
/// user's `exe` link), without a ` (deleted)` suffix. `cua_exe_sha PID`
/// prints the sha256 of what it runs (as its owner when root cannot read
/// it). `cua_daemon` prints `<pid> <exe>` of the running cua-spacesd (the
/// exec's parent first).
const FIND_DAEMON: &str = r#"cua_exe() {
  e=$(readlink "/proc/$1/exe" 2>/dev/null) || e=""
  [ -n "$e" ] || e=$(tr '\0' '\n' <"/proc/$1/cmdline" 2>/dev/null | head -n1) || true
  [ -n "$e" ] || return 1
  echo "${e% (deleted)}"
}
cua_exe_sha() {
  s=$(sha256sum "/proc/$1/exe" 2>/dev/null | cut -d' ' -f1)
  if [ -z "$s" ] && [ "$(id -u)" = 0 ]; then
    u=$(stat -c %U "/proc/$1" 2>/dev/null)
    s=$(runuser -u "$u" -- sha256sum "/proc/$1/exe" 2>/dev/null | cut -d' ' -f1)
  fi
  echo "$s"
}
cua_daemon() {
  for p in "$PPID" $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do
    exe=$(cua_exe "$p") || continue
    case "${exe##*/}" in cua-spacesd|cua-guestd|cua-env-driver) echo "$p $exe"; return 0;; esac
  done
  return 1
}
"#;

const ROOT: &str = r#"S=""
if [ "$(id -u)" != 0 ]; then
  S="sudo -n"
  $S true 2>/dev/null || { echo "overlays need root in the guest: run as root or give this user passwordless sudo" >&2; exit 5; }
fi
"#;

/// `$1` name, `$2` explicit target (may be empty). Prints `target=<path>`.
const RESOLVE: &str = r#"set -eu
name=$1; target=$2
if [ -z "$target" ]; then
  case "$name" in
    cua-spacesd)
      # The image's daemon may still be starting: wait for it (bounded).
      i=0; while [ $i -lt 120 ] && ! cua_daemon >/dev/null; do sleep 1; i=$((i + 1)); done
      if d=$(cua_daemon); then target=${d#* }
      elif [ -e /usr/local/bin/cua-spacesd ]; then target=$(readlink -f /usr/local/bin/cua-spacesd)
      else echo "no cua-spacesd runs in this guest" >&2; exit 4; fi ;;
    cua-driver)
      if p=$(command -v cua-driver 2>/dev/null); then target=$(readlink -f "$p"); else target=/usr/local/bin/cua-driver; fi ;;
    *) echo "overlay $name needs a guest path" >&2; exit 4 ;;
  esac
fi
echo "target=$target"
"#;

/// `$1` name `$2` uploaded file `$3` target `$4` sha256 `$5` source `$6`
/// time. Prints `previous=<sha256>`.
const INSTALL: &str = r#"set -eu
name=$1; tmp=$2; dst=$3; want=$4; src=$5; now=$6
got=$(sha256sum "$tmp" | cut -d' ' -f1)
[ "$got" = "$want" ] || { echo "uploaded $name has sha256 $got, expected $want" >&2; exit 3; }
prev=""
[ -f "$dst" ] && prev=$(sha256sum "$dst" | cut -d' ' -f1)
$S mkdir -p "$(dirname "$dst")" /var/lib/cua/overlays
$S cp "$tmp" "$dst.cua-overlay-new"
$S chmod 0755 "$dst.cua-overlay-new"
$S mv -f "$dst.cua-overlay-new" "$dst"
rm -f "$tmp"
got=$(sha256sum "$dst" | cut -d' ' -f1)
[ "$got" = "$want" ] || { echo "$dst has sha256 $got after the rename, expected $want" >&2; exit 3; }
printf '{"name":"%s","path":"%s","sha256":"%s","previous_sha256":"%s","source":"%s","applied_at":"%s"}\n' \
  "$name" "$dst" "$want" "$prev" "$src" "$now" | $S tee "/var/lib/cua/overlays/$name.json" >/dev/null
echo "previous=$prev"
"#;

/// Schedules a restart of the running cua-spacesd in a detached session
/// (this exec runs under it). Prints `pid=<old pid>` and `how=<method>`.
const RESTART_DAEMON: &str = r#"set -eu
d=$(cua_daemon) || { echo "how="; exit 0; }
pid=${d%% *}
unit=$(sed -n 's#.*/\([^/]*\.service\)$#\1#p' "/proc/$pid/cgroup" 2>/dev/null | head -n1 || true)
prog=""
if [ -S /run/supervisor.sock ] || [ -S /var/run/supervisor.sock ]; then
  prog=$($S supervisorctl status 2>/dev/null | awk '{print $1}' | grep -E '^(cua-spacesd|cua-guestd|cua-env-driver)$' | head -n1 || true)
fi
if [ -n "$prog" ]; then cmd="supervisorctl restart $prog"; how="supervisor:$prog"
elif [ -n "$unit" ] && command -v systemctl >/dev/null 2>&1; then cmd="systemctl restart $unit"; how="systemd:$unit"
else cmd="kill -TERM $pid"; how="signal:$pid"; fi
$S setsid -f sh -c "sleep 1; $cmd" >/dev/null 2>&1 </dev/null
sleep 0.3
echo "pid=$pid"
echo "how=$how"
"#;

/// Prints `<pid> <sha256>` of the running cua-spacesd.
const DAEMON_IDENTITY: &str = r#"d=$(cua_daemon) || exit 1
pid=${d%% *}
echo "$pid $(cua_exe_sha "$pid")"
"#;

/// `$1` program basename `$2` sha256: stops processes running another
/// build of it (restarting a supervisor program of that name instead).
/// Prints `how=<method>`.
const RESTART_PROGRAM: &str = r#"set -eu
prog=$1; want=$2; how=""
if [ -S /run/supervisor.sock ] && $S supervisorctl status "$prog" >/dev/null 2>&1; then
  $S supervisorctl restart "$prog" >/dev/null && how="supervisor:$prog"
fi
stopped=""
for p in $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do
  exe=$(cua_exe "$p") || continue
  case "${exe##*/}" in "$prog") ;; *) continue ;; esac
  sha=$(cua_exe_sha "$p")
  [ -n "$sha" ] || continue
  if [ "$sha" != "$want" ]; then $S kill -TERM "$p" 2>/dev/null && stopped="$stopped,$p"; fi
done
[ -n "$stopped" ] && how="${how:+$how;}stopped:${stopped#,}"
echo "how=$how"
"#;

fn field<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .map(str::trim)
}

/// How overlays reach the guest as root.
enum Guest {
    /// `docker exec -u 0` into a local container.
    Engine { engine: String, container: String },
    /// cua-spacesd's process and file services (root or `sudo -n`).
    Spacesd(Arc<SpacesdClient>),
}

impl Guest {
    /// The engine for a local container sandbox, when its CLI answers.
    async fn engine_for(sandbox: &Sandbox) -> Option<Guest> {
        let info = sandbox.info();
        let container = info.provider_details.get("container_id")?.clone();
        if container.is_empty() {
            return None;
        }
        let engine = std::env::var("CUA_CONTAINER_ENGINE")
            .ok()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| "docker".into());
        let ok = tokio::process::Command::new(&engine)
            .args(["inspect", "-f", "{{.State.Running}}", &container])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
        ok.then_some(Guest::Engine { engine, container })
    }

    async fn sh(&self, script: String, what: &str) -> Result<String> {
        let (success, stdout, stderr) = match self {
            Guest::Engine { engine, container } => {
                let o = tokio::time::timeout(
                    Duration::from_secs(300),
                    tokio::process::Command::new(engine)
                        .args(["exec", "-u", "0", container, "/bin/sh", "-c", &script])
                        .stdin(std::process::Stdio::null())
                        .output(),
                )
                .await
                .map_err(|_| CuaError::Timeout(format!("overlay {what}: {engine} exec timed out")))?
                .map_err(|e| CuaError::HostCapabilityMissing(format!("{engine} exec: {e}")))?;
                (
                    o.status.success(),
                    String::from_utf8_lossy(&o.stdout).into_owned(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                )
            }
            Guest::Spacesd(env) => {
                let o = env.sh(script, Some(120_000)).await?;
                (
                    o.exit.success,
                    String::from_utf8_lossy(&o.stdout).into_owned(),
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                )
            }
        };
        if !success {
            return Err(CuaError::Runtime(format!(
                "overlay {what}: {}",
                stderr.trim().lines().last().unwrap_or("failed")
            )));
        }
        Ok(stdout)
    }

    /// Copies `local` to `guest`; returns the sha256 the transfer reports
    /// (empty when it reports none; the install step checks it anyway).
    async fn upload(&self, local: &str, guest: &str) -> Result<String> {
        match self {
            Guest::Engine { engine, container } => {
                // Streamed through `exec -i` rather than `cp`: gVisor keeps the
                // rootfs overlay (and /tmp) inside the sandbox, where `cp`
                // into the container's filesystem would not be seen.
                let file = std::fs::File::open(local)
                    .map_err(|e| CuaError::NotFound(format!("overlay file {local}: {e}")))?;
                let o = tokio::time::timeout(
                    Duration::from_secs(600),
                    tokio::process::Command::new(engine)
                        .args([
                            "exec",
                            "-i",
                            "-u",
                            "0",
                            container,
                            "/bin/sh",
                            "-c",
                            &format!("umask 077; cat >{}", sq(guest)),
                        ])
                        .stdin(std::process::Stdio::from(file))
                        .output(),
                )
                .await
                .map_err(|_| CuaError::Timeout(format!("overlay upload: {engine} exec timed out")))?
                .map_err(|e| CuaError::HostCapabilityMissing(format!("{engine} exec: {e}")))?;
                if !o.status.success() {
                    return Err(CuaError::Runtime(format!(
                        "overlay upload: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    )));
                }
                Ok(String::new())
            }
            Guest::Spacesd(env) => Ok(env
                .upload_file(local.to_string(), guest.to_string(), None)
                .await?
                .sha256),
        }
    }
}

/// sha256 (hex) and size of a local file.
fn local_digest(path: &str) -> Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let meta = std::fs::metadata(path)
        .map_err(|e| CuaError::NotFound(format!("overlay file {path}: {e}")))?;
    if !meta.is_file() {
        return Err(CuaError::InvalidArgument(format!(
            "overlay file {path} is not a file"
        )));
    }
    let mut file = std::fs::File::open(path)
        .map_err(|e| CuaError::NotFound(format!("overlay file {path}: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| CuaError::Internal(format!("reading {path}: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((hex::encode(hasher.finalize()), meta.len()))
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    // Civil date from days since the epoch (proleptic Gregorian).
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

async fn apply_one(
    sandbox: &Sandbox,
    guest: &Guest,
    o: Overlay,
    deadline: Instant,
) -> Result<OverlayResult> {
    o.validate()?;
    let (sha256, size) = local_digest(&o.path)?;
    let stdout = guest
        .sh(
            format!(
                "set -- {} {}\n{FIND_DAEMON}{RESOLVE}",
                sq(&o.name),
                sq(o.target.as_deref().unwrap_or(""))
            ),
            "resolving the guest path",
        )
        .await?;
    let target = field(&stdout, "target")
        .filter(|t| t.starts_with('/') && safe_text(t))
        .ok_or_else(|| CuaError::Runtime(format!("overlay {}: no guest path resolved", o.name)))?
        .to_string();

    let tmp = format!(
        "/tmp/.cua-overlay-{}-{}-{}",
        o.name,
        std::process::id(),
        &sha256[..12]
    );
    let sent = guest.upload(&o.path, &tmp).await?;
    if !sent.is_empty() && !sent.eq_ignore_ascii_case(&sha256) {
        return Err(CuaError::Runtime(format!(
            "overlay {}: uploaded sha256 {sent} differs from the local file's {sha256}",
            o.name
        )));
    }
    let source = o.source.clone().unwrap_or_else(|| o.path.clone());
    let source = if safe_text(&source) {
        source
    } else {
        String::new()
    };
    let stdout = guest
        .sh(
            format!(
                "set -- {} {} {} {} {} {}\n{ROOT}{INSTALL}",
                sq(&o.name),
                sq(&tmp),
                sq(&target),
                sq(&sha256),
                sq(&source),
                sq(&now_rfc3339()),
            ),
            "installing",
        )
        .await?;
    let previous_sha256 = field(&stdout, "previous").unwrap_or_default().to_string();

    let restarted = if o.name == "cua-spacesd" {
        restart_daemon(sandbox, guest, &sha256, deadline).await?
    } else {
        let program = target.rsplit('/').next().unwrap_or_default().to_string();
        let stdout = guest
            .sh(
                format!(
                    "set -- {} {}\n{FIND_DAEMON}{ROOT}{RESTART_PROGRAM}",
                    sq(&program),
                    sq(&sha256)
                ),
                "restarting",
            )
            .await?;
        field(&stdout, "how").unwrap_or_default().to_string()
    };
    Ok(OverlayResult {
        name: o.name,
        target,
        sha256,
        previous_sha256,
        size,
        restarted,
    })
}

/// Restarts the guest's cua-spacesd and waits (bounded by `deadline`)
/// until a new process runs `sha256`.
async fn restart_daemon(
    sandbox: &Sandbox,
    guest: &Guest,
    sha256: &str,
    deadline: Instant,
) -> Result<String> {
    let identity = format!("{FIND_DAEMON}{DAEMON_IDENTITY}");
    // Already running this build (the same overlay applied twice): no restart.
    if let Ok(out) = guest.sh(identity.clone(), "identity").await
        && out.split_whitespace().nth(1) == Some(sha256)
    {
        return Ok(String::new());
    }
    let stdout = guest
        .sh(
            format!("{FIND_DAEMON}{ROOT}{RESTART_DAEMON}"),
            "restarting cua-spacesd",
        )
        .await?;
    let how = field(&stdout, "how").unwrap_or_default().to_string();
    if how.is_empty() {
        return Err(CuaError::Runtime(
            "overlay cua-spacesd: the daemon process was not found in the guest".into(),
        ));
    }
    let old_pid = field(&stdout, "pid").unwrap_or_default().to_string();
    let mut last = String::from("no answer");
    // Bounded: one probe per second until the deadline.
    for _ in 0..600 {
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        // Through spacesd, a fresh attachment each time (the old process is
        // gone); through the engine, the same exec.
        let probe = match guest {
            Guest::Engine { .. } => guest.sh(identity.clone(), "identity").await,
            Guest::Spacesd(_) => match sandbox.spacesd(Some(5_000)).await {
                Ok(env) => Guest::Spacesd(env).sh(identity.clone(), "identity").await,
                Err(e) => Err(e),
            },
        };
        match probe {
            Ok(out) => {
                let mut parts = out.split_whitespace();
                let (pid, sha) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                if pid != old_pid && sha == sha256 {
                    return Ok(how);
                }
                last = format!("pid {pid} runs sha256 {}", &sha[..sha.len().min(12)]);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(CuaError::Timeout(format!(
        "overlay cua-spacesd: the restarted daemon does not run the injected build ({last})"
    )))
}

#[uniffi::export]
impl Sandbox {
    /// Injects binaries (see [`Overlay`]): each replaces its guest file
    /// atomically, is recorded under `/var/lib/cua/overlays` with its
    /// sha256, and what runs it is restarted. Local containers go through
    /// the container engine; other sandboxes need cua-spacesd and root (or
    /// passwordless sudo) in the guest. `timeout_ms`
    /// bounds a cua-spacesd restart (default 180 s).
    #[uniffi::method(default(timeout_ms = None))]
    pub async fn overlay(
        self: Arc<Self>,
        overlays: Vec<Overlay>,
        timeout_ms: Option<u32>,
    ) -> Result<Vec<OverlayResult>> {
        for o in &overlays {
            o.validate()?;
        }
        let timeout = Duration::from_millis(u64::from(timeout_ms.unwrap_or(180_000)));
        let ordered_has_daemon = overlays.iter().any(|o| o.name == "cua-spacesd");
        run(async move {
            let deadline = Instant::now() + timeout;
            let guest = match Guest::engine_for(&self).await {
                Some(g) => {
                    // Replacing the daemon: let the image's own start first,
                    // so there is a running process to resolve and restart.
                    if ordered_has_daemon {
                        let _ = self.spacesd(Some(120_000)).await;
                    }
                    g
                }
                None => Guest::Spacesd(self.spacesd(Some(120_000)).await?),
            };
            // cua-spacesd last: its restart drops the connection the others use.
            let mut ordered = overlays;
            ordered.sort_by_key(|o| o.name == "cua-spacesd");
            let mut out = Vec::with_capacity(ordered.len());
            for o in ordered {
                out.push(apply_one(&self, &guest, o, deadline).await?);
            }
            Ok(out)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_specs() {
        let o = Overlay::parse("cua-driver=./target/release/cua-driver").unwrap();
        assert_eq!(o.name, "cua-driver");
        assert_eq!(o.path, "./target/release/cua-driver");
        assert_eq!(o.target, None);
        let o = Overlay::parse("tool=/tmp/a:b/tool:/opt/bin/tool").unwrap();
        assert_eq!(o.path, "/tmp/a:b/tool");
        assert_eq!(o.target.as_deref(), Some("/opt/bin/tool"));
        let o = Overlay::parse("cua-spacesd=C:/build/cua-spacesd").unwrap();
        assert_eq!(o.path, "C:/build/cua-spacesd");
        assert_eq!(o.target, None);
        assert!(Overlay::parse("cua-driver").is_err());
        assert!(
            Overlay::parse("tool=./tool").is_err(),
            "unknown names need a guest path"
        );
        assert!(Overlay::parse("../x=./x:/opt/x").is_err());
        assert!(Overlay::parse("x y=./x:/opt/x").is_err());
        assert!(Overlay::parse("x=./x:/opt/\"x").is_err());
        assert!(Overlay::parse("cua-driver=").is_err());
    }

    #[test]
    fn quoting_is_shell_safe() {
        assert_eq!(sq("a b"), "'a b'");
        assert_eq!(sq("it's"), "'it'\\''s'");
    }

    #[test]
    fn fields_are_read_from_script_output() {
        let out = "noise\ntarget=/usr/local/bin/cua-guestd\nprevious=\n";
        assert_eq!(field(out, "target"), Some("/usr/local/bin/cua-guestd"));
        assert_eq!(field(out, "previous"), Some(""));
        assert_eq!(field(out, "how"), None);
    }

    #[test]
    fn digests_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("abc");
        std::fs::write(&p, b"abc").unwrap();
        let (sha, size) = local_digest(p.to_str().unwrap()).unwrap();
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(size, 3);
        assert!(local_digest(dir.path().to_str().unwrap()).is_err());
        assert!(local_digest("/nonexistent/cua-overlay").is_err());
    }

    #[test]
    fn timestamps_are_rfc3339() {
        let t = now_rfc3339();
        assert_eq!(t.len(), 20, "{t}");
        assert!(t.starts_with("20") && t.ends_with('Z') && t.as_bytes()[10] == b'T');
    }
}
