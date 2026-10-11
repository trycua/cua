// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `auth`: the access token reaches cua-spacesd the way the image's
//! delivery mode says, with that mode's file contract, and never leaks.
//!
//! Three delivery modes are correct today, each with its own contract:
//!
//! | mode | detected by | contract |
//! |---|---|---|
//! | `local` | `/run/cua` is not a mount | `/run/cua/env-token` is `0640 root:<desktop user>` (written by `ensure-env-token.sh`) |
//! | `pod-secret` | `/run/cua` is a mount (Fleet pod Secret, or a local claim-secrets share) | `/run/cua/env-token` is root-owned with no world bits; the unprivileged copy `/run/cua-env/env-token` is `0600 <desktop user>` in a root `0755` directory |
//! | `kubevirt-bridge` | `/run/cua-claim/share` is a virtiofs mount (cloud #7888) | the share shows `0644` (expected), `/run/cua` is the bridge's tmpfs, `/run/cua/env-token` is `0600 root`, `cua-claim-secrets-sync` is active, and the copy is as above |

use std::path::Path;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{ConnectOptions, SpacesdClient};

use crate::sys;
use crate::{Ctx, Recorder};

/// Token delivery mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Local token file from the entrypoint / cloud-init.
    Local,
    /// A mounted claim Secret at `/run/cua`.
    PodSecret,
    /// KubeVirt virtiofs share mirrored by the guest bridge.
    KubevirtBridge,
}

impl Mode {
    /// Manifest name.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Local => "local",
            Mode::PodSecret => "pod-secret",
            Mode::KubevirtBridge => "kubevirt-bridge",
        }
    }
}

/// Where the probes look; tests point it at a temp tree.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `/run/cua`.
    pub claim_dir: String,
    /// `/run/cua-claim/share`.
    pub share_dir: String,
    /// `/run/cua-env`.
    pub synced_dir: String,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            claim_dir: "/run/cua".into(),
            share_dir: "/run/cua-claim/share".into(),
            synced_dir: "/run/cua-env".into(),
        }
    }
}

/// Detects the mode from the mount table.
pub fn detect(paths: &Paths, mounts: &[sys::Mount]) -> (Mode, Option<sys::Mount>) {
    let at = |point: &str| mounts.iter().rev().find(|m| m.point == point).cloned();
    if at(&paths.share_dir).is_some_and(|m| m.fstype == "virtiofs") {
        return (Mode::KubevirtBridge, at(&paths.claim_dir));
    }
    match at(&paths.claim_dir) {
        Some(mount) => (Mode::PodSecret, Some(mount)),
        None => (Mode::Local, None),
    }
}

/// Checks a file against an expected owner/group and mode rule; returns
/// problems.
pub fn expect_file(
    path: &Path,
    owner: Option<u32>,
    group: Option<u32>,
    rule: impl Fn(u32) -> bool,
    rule_text: &str,
) -> Vec<String> {
    let Some(mode) = sys::mode(path) else {
        // A root-only claim directory (the SDK's local-VM tmpfs, 0700) hides
        // the file from an unprivileged doctor (Diagnose runs as the
        // driver's user): unverifiable here, not missing. Root sees it.
        if hidden_from_us(path) {
            return Vec::new();
        }
        return vec![format!("{} is missing", path.display())];
    };
    let mut problems = Vec::new();
    if !rule(mode.mode) {
        problems.push(format!(
            "{} is {} (want {rule_text})",
            path.display(),
            mode.describe()
        ));
    }
    if owner.is_some_and(|o| o != mode.uid) || group.is_some_and(|g| g != mode.gid) {
        problems.push(format!(
            "{} is owned {} (want {}:{})",
            path.display(),
            mode.describe(),
            owner
                .map(|o| sys::user_name(o).unwrap_or(o.to_string()))
                .unwrap_or("*".into()),
            group
                .map(|g| sys::group_name(g).unwrap_or(g.to_string()))
                .unwrap_or("*".into()),
        ));
    }
    problems
}

/// Whether `path` cannot be inspected because a parent directory denies
/// this (non-root) user, as opposed to not existing.
pub fn hidden_from_us(path: &Path) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return false;
        }
    }
    matches!(
        std::fs::metadata(path),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
    )
}

/// The contract of `mode`, checked under `paths`. Returns (problems, facts).
pub fn check_contract(
    mode: Mode,
    paths: &Paths,
    claim_mount: Option<&sys::Mount>,
    desktop_user: &str,
    synced_in_use: bool,
) -> (Vec<String>, Vec<(String, String)>) {
    let uid = sys::user_id(desktop_user);
    let gid = sys::group_id(desktop_user);
    let token = Path::new(&paths.claim_dir).join("env-token");
    let synced = Path::new(&paths.synced_dir).join("env-token");
    let mut problems = Vec::new();
    let mut facts = vec![("mode".to_owned(), mode.name().to_owned())];
    if let Some(m) = sys::mode(&token) {
        facts.push(("token_file".into(), m.describe()));
    } else if hidden_from_us(&token) {
        facts.push((
            "token_file".into(),
            "not visible to this user (root-only claim directory); run the doctor as root to check it".into(),
        ));
    }
    match mode {
        Mode::Local => {
            problems.extend(expect_file(&token, Some(0), gid, |m| m == 0o640, "0640"));
        }
        Mode::PodSecret | Mode::KubevirtBridge => {
            if let Some(mount) = claim_mount {
                facts.push(("claim_fstype".into(), mount.fstype.clone()));
            }
            if mode == Mode::KubevirtBridge {
                if claim_mount.is_some_and(|m| m.fstype != "tmpfs") {
                    problems.push(format!(
                        "{} is {:?}, not the bridge's tmpfs",
                        paths.claim_dir,
                        claim_mount.map(|m| m.fstype.clone()).unwrap_or_default()
                    ));
                }
                problems.extend(expect_file(&token, Some(0), None, |m| m == 0o600, "0600"));
            } else {
                // Pod Secret: root 0600 (or 0440 with fsGroup = the desktop
                // user's group). Never world-accessible.
                // A share mounted directly (virtio-fs / 9p, the local
                // claim-secrets emulation) passes the host's ownership
                // through; only a Secret volume is root-owned.
                let shared =
                    claim_mount.is_some_and(|m| m.fstype == "virtiofs" || m.fstype == "9p");
                let owner = if shared { None } else { Some(0) };
                let group_ok = |m: u32| m & 0o007 == 0 && (m & 0o070 == 0 || m & 0o020 == 0);
                problems.extend(expect_file(
                    &token,
                    owner,
                    None,
                    group_ok,
                    "0600, or 0440 with fsGroup",
                ));
                if claim_mount.is_some_and(|m| m.fstype == "virtiofs" || m.fstype == "9p") {
                    facts.push((
                        "note".into(),
                        "claim share mounted directly (no guest bridge); KubeVirt shows it 0644"
                            .into(),
                    ));
                }
            }
            if synced_in_use {
                problems.extend(expect_file(&synced, uid, None, |m| m == 0o600, "0600"));
                problems.extend(expect_file(
                    Path::new(&paths.synced_dir),
                    Some(0),
                    None,
                    |m| m == 0o755,
                    "0755",
                ));
                if let Some(m) = sys::mode(&synced) {
                    facts.push(("synced_file".into(), m.describe()));
                }
            }
        }
    }
    (problems, facts)
}

/// Pids whose command line or (readable) environment holds `token`,
/// excluding cua-spacesd itself.
pub fn leaks(token: &str) -> (Vec<u32>, Vec<u32>) {
    let mut argv = Vec::new();
    let mut environ = Vec::new();
    let me = std::process::id();
    for pid in sys::pids() {
        let cmdline = sys::proc_cmdline(pid).unwrap_or_default();
        if cmdline.contains(token) {
            argv.push(pid);
        }
        if pid == me || cmdline.contains("cua-spacesd") {
            continue;
        }
        if let Some(env) = sys::proc_environ(pid) {
            let needle = token.as_bytes();
            if env.windows(needle.len()).any(|w| w == needle) {
                environ.push(pid);
            }
        }
    }
    (argv, environ)
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("auth") {
        return;
    }
    let token = ctx.token.clone().filter(|t| !t.is_empty());

    rec.run(
        "auth.unauthenticated",
        &["core"],
        Duration::from_secs(15),
        async {
            if token.is_none() {
                return Check::new(
                    "auth.unauthenticated",
                    Status::Skip,
                    "no token configured (open loopback bind); nothing to refuse",
                )
                .skip_reason("not_applicable");
            }
            let mut options = match ConnectOptions::parse(&ctx.client.endpoint().to_string()) {
                Ok(o) => o,
                Err(error) => {
                    return Check::new("auth.unauthenticated", Status::Fail, error.to_string())
                }
            };
            options = options.probe(false);
            let anonymous = match SpacesdClient::connect(options).await {
                Ok(c) => c,
                Err(error) => {
                    return Check::new("auth.unauthenticated", Status::Fail, error.to_string())
                }
            };
            match anonymous.stat("/").await {
                Err(error) if matches!(error.code(), Some(tonic::Code::Unauthenticated)) => {
                    Check::new(
                        "auth.unauthenticated",
                        Status::Pass,
                        "a call without the token is refused with UNAUTHENTICATED",
                    )
                }
                Err(error) => Check::new(
                    "auth.unauthenticated",
                    Status::Fail,
                    format!("unauthenticated Stat failed with the wrong error: {error}"),
                ),
                Ok(_) => Check::new(
                    "auth.unauthenticated",
                    Status::Fail,
                    "an unauthenticated Stat succeeded",
                )
                .fix("the service accepts calls without its token"),
            }
        },
    )
    .await;

    // Bind policy: a non-loopback listener implies a token.
    let config = ctx.print_config.lock().unwrap().clone();
    if let Some(config) = config {
        let listen = config["server"]["listen"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let source = config["server"]["token_source"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let loopback = listen
            .parse::<std::net::SocketAddr>()
            .map(|a| a.ip().is_loopback())
            .unwrap_or(false);
        rec.push(
            Check::new(
                "auth.bind_policy",
                super::verdict(loopback || source != "none"),
                format!("listens on {listen}, token from {source}"),
            )
            .fix("a non-loopback bind must have a token (CUA_ENV_TOKEN, the token file or await mode)"),
            &["core"],
        )
        .await;
    }

    if ctx.os() == "macos" {
        macos_token_file(ctx, rec).await;
        return;
    }
    if ctx.os() != "linux" {
        rec.skip(
            "auth.token_file",
            &["manifest:auth_modes"],
            "not_applicable",
            "token file contracts are Linux-only for now".into(),
        )
        .await;
        return;
    }
    let paths = Paths::default();
    let mounts = sys::read_capped(Path::new("/proc/self/mountinfo"))
        .map(|t| sys::parse_mountinfo(&t))
        .unwrap_or_default();
    let (mode, claim_mount) = detect(&paths, &mounts);
    let claimed = ctx
        .manifest
        .manifest
        .auth_modes
        .iter()
        .any(|m| m == mode.name());
    rec.push(
        Check::new(
            "auth.mode",
            super::verdict(claimed || !ctx.manifest.present()),
            format!(
                "token delivery mode {} ({})",
                mode.name(),
                if claimed {
                    "claimed by the image"
                } else {
                    "NOT claimed by the image"
                }
            ),
        )
        .fact("mode", mode.name())
        .fact("virtio_fs", sys::virtio_fs_present()),
        &["manifest:auth_modes"],
    )
    .await;

    let user = if ctx.manifest.manifest.display.user.is_empty() {
        "cua".to_owned()
    } else {
        ctx.manifest.manifest.display.user.clone()
    };
    // The running service's token file (await-token-file mode reports it in
    // Health); `--print-config` only shows how this process would start.
    let token_source = service_token_source(ctx)
        .await
        .unwrap_or_else(|| config_token_source(ctx));
    let synced_in_use = token_source.contains(&paths.synced_dir);
    rec.run("auth.token_file", &["manifest:auth_modes"], Duration::from_secs(15), async {
        let (mut problems, facts) = check_contract(mode, &paths, claim_mount.as_ref(), &user, synced_in_use);
        if mode == Mode::KubevirtBridge {
            match sys::run_local("systemctl", &["is-active", "cua-claim-secrets-sync.service"], &[], Duration::from_secs(10)).await {
                Ok(out) if out.stdout.trim() == "active" => {}
                Ok(out) => problems.push(format!("cua-claim-secrets-sync is {}", out.stdout.trim())),
                Err(error) => problems.push(format!("systemctl: {error}")),
            }
        }
        let mut check = Check::new(
            "auth.token_file",
            super::verdict(problems.is_empty()),
            if problems.is_empty() {
                format!("{} contract holds (token from {token_source})", mode.name())
            } else {
                problems.join("; ")
            },
        )
        .fix(match mode {
            Mode::Local => "ensure-env-token.sh must leave /run/cua/env-token root:<desktop user> 0640",
            Mode::PodSecret => "the claim Secret must be root 0600 and token-sync must keep /run/cua-env/env-token 0600",
            Mode::KubevirtBridge => "the claim-secrets guest bridge (cloud #7888) must mirror the share to /run/cua as root 0600",
        });
        for (k, v) in facts {
            check = check.fact(k, v);
        }
        check
    })
    .await;

    match token {
        None => {
            rec.skip(
                "auth.token_leak",
                &["core"],
                "not_applicable",
                "no token to look for".into(),
            )
            .await;
        }
        Some(token) => {
            rec.run("auth.token_leak", &["core"], Duration::from_secs(20), async {
                let (argv, environ) = tokio::task::spawn_blocking(move || leaks(&token))
                    .await
                    .unwrap_or_default();
                let root = sys::euid() == 0;
                let mut check = Check::new(
                    "auth.token_leak",
                    super::verdict(argv.is_empty() && environ.is_empty()),
                    if argv.is_empty() && environ.is_empty() {
                        format!(
                            "the token is in no process argv or environment besides cua-spacesd's{}",
                            if root { "" } else { " (environments of other users unreadable)" }
                        )
                    } else {
                        format!(
                            "token visible in argv of {}, environment of {}",
                            describe_pids(&argv),
                            describe_pids(&environ)
                        )
                    },
                )
                .fix("pass the token through the token file, not argv or an inherited CUA_ENV_TOKEN");
                check = check.fact("scanned_as_root", root);
                check
            })
            .await;
        }
    }
}

/// "pid 1 (supervisord), pid 21 (bash)".
fn describe_pids(pids: &[u32]) -> String {
    if pids.is_empty() {
        return "no process".into();
    }
    pids.iter()
        .take(12)
        .map(|pid| {
            let comm = sys::read_capped(Path::new(&format!("/proc/{pid}/comm")))
                .map(|c| c.trim().to_owned())
                .unwrap_or_default();
            format!("pid {pid} ({comm})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `await-file:<path>` when the running service follows a token file
/// (its Health `auth` component says so).
/// The macOS image contract (libs/images/macos): the driver runs as the
/// desktop user and reads its token from `~/.cua/spacesd/token`, which must
/// be owned by that user with no group or world bits, in a 0700 directory.
/// With no token file the driver must be in bootstrap mode (the cua SDK
/// installs a token with Init).
async fn macos_token_file(ctx: &Ctx, rec: &mut Recorder<'_>) {
    let user = ctx.manifest.manifest.display.user.clone();
    let home = if user.is_empty() {
        std::env::var("HOME").unwrap_or_default()
    } else {
        format!("/Users/{user}")
    };
    let dir = Path::new(&home).join(".cua").join("spacesd");
    let file = dir.join("token");
    let detail = ctx
        .client
        .health()
        .await
        .ok()
        .and_then(|h| h.components.into_iter().find(|c| c.name == "auth"))
        .map(|c| c.detail)
        .unwrap_or_default();
    rec.run("auth.token_file", &["manifest:auth_modes"], Duration::from_secs(15), async {
        let (problems, message) = macos_contract(&dir, &file, &user, &detail);
        Check::new("auth.token_file", super::verdict(problems.is_empty()), if problems.is_empty() {
            message
        } else {
            problems.join("; ")
        })
        .fact("mode", "local")
        .fact("token_file", file.display().to_string())
        .fix("start-spacesd.sh keeps ~/.cua/spacesd/token 0600 (directory 0700), owned by the desktop user")
    })
    .await;
}

/// Problems with the macOS token file contract, and the passing message.
pub fn macos_contract(
    dir: &Path,
    file: &Path,
    user: &str,
    health_detail: &str,
) -> (Vec<String>, String) {
    let uid = (!user.is_empty()).then(|| sys::user_id(user)).flatten();
    let private = |m: u32| m & 0o077 == 0;
    if sys::mode(file).is_none() && !hidden_from_us(file) {
        let bootstrap = !health_detail.contains("token from file");
        return if bootstrap {
            (
                Vec::new(),
                format!("no token file; bootstrap mode ({health_detail})"),
            )
        } else {
            (
                vec![format!(
                    "{} is missing but the driver reports: {health_detail}",
                    file.display()
                )],
                String::new(),
            )
        };
    }
    let mut problems = expect_file(file, uid, None, private, "0600");
    problems.extend(expect_file(dir, uid, None, private, "0700"));
    if !health_detail.is_empty() && !health_detail.contains(&file.display().to_string()) {
        problems.push(format!(
            "the driver does not read {}: {health_detail}",
            file.display()
        ));
    }
    let message = if health_detail.is_empty() {
        format!(
            "local contract holds: {} is private to its owner",
            file.display()
        )
    } else {
        format!("local contract holds ({health_detail})")
    };
    (problems, message)
}

async fn service_token_source(ctx: &Ctx) -> Option<String> {
    let health = ctx.client.health().await.ok()?;
    let detail = health
        .components
        .into_iter()
        .find(|c| c.name == "auth")?
        .detail;
    detail
        .strip_prefix("token from file ")
        .or_else(|| detail.strip_prefix("awaiting token file "))
        .map(|path| format!("await-file:{path}"))
}

fn config_token_source(ctx: &Ctx) -> String {
    ctx.print_config
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c["server"]["token_source"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn macos_token_file_contract() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".cua/spacesd");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = dir.join("token");
        let detail = format!("token from file {}", file.display());

        // No file: fine in bootstrap mode, wrong when the driver reads one.
        assert!(macos_contract(&dir, &file, "", "bootstrap: awaiting Init")
            .0
            .is_empty());
        assert!(!macos_contract(&dir, &file, "", &detail).0.is_empty());

        std::fs::write(&file, "t").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (problems, message) = macos_contract(&dir, &file, "", &detail);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(message.contains("local contract holds"));

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(macos_contract(&dir, &file, "", &detail)
            .0
            .iter()
            .any(|p| p.contains("want 0600")));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(macos_contract(&dir, &file, "", &detail)
            .0
            .iter()
            .any(|p| p.contains("want 0700")));
        // The driver reading some other file breaks the contract too.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            !macos_contract(&dir, &file, "", "token from file /etc/other")
                .0
                .is_empty()
        );
    }

    fn mount(point: &str, fstype: &str) -> sys::Mount {
        sys::Mount {
            point: point.into(),
            fstype: fstype.into(),
            source: "x".into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_root_only_directory_hides_rather_than_misses_the_token() {
        use std::os::unix::fs::PermissionsExt as _;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return; // root sees through 0700; nothing to test
        }
        let dir = tempfile::tempdir().unwrap();
        let claim = dir.path().join("cua");
        std::fs::create_dir(&claim).unwrap();
        std::fs::write(claim.join("env-token"), "t").unwrap();
        std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o000)).unwrap();
        let token = claim.join("env-token");
        assert!(hidden_from_us(&token));
        assert!(expect_file(&token, Some(0), None, |m| m == 0o600, "0600").is_empty());
        // A truly absent file is still reported.
        let absent = dir.path().join("nope");
        assert!(!hidden_from_us(&absent));
        assert_eq!(
            expect_file(&absent, None, None, |_| true, "any"),
            vec![format!("{} is missing", absent.display())]
        );
        std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn detects_each_mode() {
        let paths = Paths::default();
        assert_eq!(detect(&paths, &[]).0, Mode::Local);
        assert_eq!(
            detect(&paths, &[mount("/run/cua", "tmpfs")]).0,
            Mode::PodSecret
        );
        assert_eq!(
            detect(&paths, &[mount("/run/cua", "virtiofs")]).0,
            Mode::PodSecret
        );
        let bridge = [
            mount("/run/cua-claim/share", "virtiofs"),
            mount("/run/cua", "tmpfs"),
        ];
        let (mode, claim) = detect(&paths, &bridge);
        assert_eq!(mode, Mode::KubevirtBridge);
        assert_eq!(claim.unwrap().fstype, "tmpfs");
    }

    #[cfg(unix)]
    #[test]
    fn contracts_check_modes() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let claim = dir.path().join("cua");
        let synced = dir.path().join("cua-env");
        std::fs::create_dir_all(&claim).unwrap();
        std::fs::create_dir_all(&synced).unwrap();
        let token = claim.join("env-token");
        std::fs::write(&token, "t").unwrap();
        let paths = Paths {
            claim_dir: claim.to_string_lossy().into(),
            share_dir: dir.path().join("share").to_string_lossy().into(),
            synced_dir: synced.to_string_lossy().into(),
        };
        let me = sys::user_name(sys::euid()).unwrap_or_else(|| "root".into());
        let set = |p: &Path, m: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap()
        };
        // Local wants 0640 root-owned: a file owned by a non-root user fails
        // on ownership; the mode rule is checked independently.
        set(&token, 0o644);
        let (problems, _) = check_contract(Mode::Local, &paths, None, &me, false);
        assert!(
            problems.iter().any(|p| p.contains("want 0640")),
            "{problems:?}"
        );
        set(&token, 0o640);
        let (problems, _) = check_contract(Mode::Local, &paths, None, &me, false);
        assert!(
            !problems.iter().any(|p| p.contains("want 0640")),
            "{problems:?}"
        );
        // Pod secret: world-readable is refused; the synced copy must be 0600.
        set(&token, 0o644);
        let (problems, _) = check_contract(Mode::PodSecret, &paths, None, &me, false);
        assert!(
            problems.iter().any(|p| p.contains("0600, or 0440")),
            "{problems:?}"
        );
        set(&token, 0o600);
        let copy = synced.join("env-token");
        std::fs::write(&copy, "t").unwrap();
        set(&copy, 0o644);
        set(&synced, 0o755);
        let (problems, facts) = check_contract(Mode::PodSecret, &paths, None, &me, true);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("env-token") && p.contains("want 0600")),
            "{problems:?}"
        );
        assert!(facts.iter().any(|(k, _)| k == "synced_file"));
        // The bridge requires /run/cua to be its tmpfs.
        let (problems, _) = check_contract(
            Mode::KubevirtBridge,
            &paths,
            Some(&mount("/run/cua", "virtiofs")),
            &me,
            false,
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("not the bridge's tmpfs")),
            "{problems:?}"
        );
    }

    /// A claim directory this uid cannot search hides the token: recorded as
    /// a fact, not reported missing (the doctor runs as the desktop user).
    #[cfg(unix)]
    #[test]
    fn a_root_only_claim_dir_is_not_a_missing_token() {
        use std::os::unix::fs::PermissionsExt as _;
        if sys::euid() == 0 {
            return; // root sees through 0700
        }
        let dir = tempfile::tempdir().unwrap();
        let claim = dir.path().join("cua");
        let synced = dir.path().join("cua-env");
        std::fs::create_dir_all(&claim).unwrap();
        std::fs::create_dir_all(&synced).unwrap();
        std::fs::write(claim.join("env-token"), "t").unwrap();
        let copy = synced.join("env-token");
        std::fs::write(&copy, "t").unwrap();
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&synced, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o000)).unwrap();
        let paths = Paths {
            claim_dir: claim.to_string_lossy().into(),
            share_dir: dir.path().join("share").to_string_lossy().into(),
            synced_dir: synced.to_string_lossy().into(),
        };
        let me = sys::user_name(sys::euid()).unwrap_or_else(|| "root".into());
        let (problems, facts) = check_contract(Mode::PodSecret, &paths, None, &me, true);
        std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            !problems.iter().any(|p| p.contains("missing")),
            "{problems:?}"
        );
        assert!(
            facts
                .iter()
                .any(|(k, v)| k == "token_file" && v.contains("not visible")),
            "{facts:?}"
        );
        // A genuinely missing token still fails.
        std::fs::remove_file(claim.join("env-token")).unwrap();
        let (problems, _) = check_contract(Mode::PodSecret, &paths, None, &me, true);
        assert!(
            problems.iter().any(|p| p.contains("missing")),
            "{problems:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_a_token_in_a_child_environment() {
        let token = format!("leak-test-{}", std::process::id());
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .env("SOMETHING", &token)
            .spawn()
            .unwrap();
        // spawn() returns once the vfork'd child enters exec, which can be
        // before the kernel has set up the new image's environment
        // (/proc/<pid>/environ still reads empty), so poll briefly.
        let mut environ = Vec::new();
        for _ in 0..100 {
            environ = leaks(&token).1;
            if environ.contains(&child.id()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(environ.contains(&child.id()), "{environ:?}");
    }
}
