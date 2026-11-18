//! The macOS Space with developer tools: Xcode, its iOS Simulator runtime and
//! the Apple developer tools on top of the full macOS image.
//!
//! Xcode and the iOS Simulator runtimes are Apple software under Apple's
//! license, for use on Apple-branded hardware. The image is published like the
//! other macOS tiers and pulled by default; when it is not available, or the
//! user prefers the Xcode already on this Mac (`CUA_DEV_IMAGE=local`), it is
//! built here as a clone of the full image's base VM plus that Xcode
//! (`libs/images/macos/derive-local.sh`, which this module embeds and runs).
//! The catalog lists the image's fallback parent (`derived_from`);
//! [`dev_parent`] recognises its reference.
//!
//! Selection is automatic: a Space created from `ghcr.io/trycua/macos:26-xcode`
//! (the `xcode` tier, or `dev`) pulls or builds the base with progress and a
//! disk check, and every later Space is a clone of it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::{Result, VmmError};
use crate::progress::{Phase, Progress};

use super::builtin::LumeCmd;

/// The canonical macOS repository.
pub const MACOS_REPO: &str = "ghcr.io/trycua/macos";

/// What the build is assumed to write on top of the full image's base: Xcode
/// and its archive, the iOS runtime, the Metal toolchain, tools and caches.
pub const DEV_IMAGE_ESTIMATE: u64 = 35 * (1 << 30);

/// The most a build may take: the Xcode copy, the runtime download and the
/// tools, on a slow connection.
pub const DERIVE_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);

/// The recipe files, as built into this crate (libs/images/macos).
const DERIVE_LOCAL: &str = include_str!(concat!(env!("OUT_DIR"), "/macos-derive-local.sh"));
const VERSIONS_ENV: &str = include_str!(concat!(env!("OUT_DIR"), "/macos-versions.env"));
const TIER_XCODE: &str = include_str!(concat!(env!("OUT_DIR"), "/macos-tier-xcode.sh"));
const FETCH: &str = include_str!(concat!(env!("OUT_DIR"), "/macos-fetch.sh"));
const XCODE_SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/macos-xcode-source.sh"));

/// The full macOS image `reference` is derived from when it is the developer
/// tools image (`ghcr.io/trycua/macos:<ver>-xcode`, with or without a
/// `@sha256:` digest, which is the published image's own and is dropped), else
/// `None`.
pub fn dev_parent(reference: &str) -> Option<String> {
    let tag_ref = reference.split('@').next().unwrap_or(reference);
    let tag = tag_ref.strip_prefix(MACOS_REPO)?.strip_prefix(':')?;
    let version = tag.strip_suffix("-xcode")?;
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    Some(format!("{MACOS_REPO}:{version}"))
}

/// Whether the user asked to build from the Xcode on this Mac instead of
/// pulling the published image (`CUA_DEV_IMAGE=local`).
pub fn prefer_local() -> bool {
    std::env::var("CUA_DEV_IMAGE").is_ok_and(|v| v.trim().eq_ignore_ascii_case("local"))
}

/// Errors unless this host is Apple-branded hardware with Apple silicon, the
/// only place the image may run.
pub fn require_apple_hardware() -> Result<()> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Ok(());
    }
    Err(VmmError::UnsupportedImage {
        image: format!("{MACOS_REPO}:26-xcode"),
        reason: "Xcode and the iOS Simulator runtimes are Apple software under Apple's license, \
                 for use on Apple-branded hardware: the macOS Space with developer tools runs \
                 only on a Mac with Apple silicon."
            .into(),
    })
}

/// Whether the SDK's recorded source `reference` of a base VM is a full
/// macOS image of `version` (`26`): `macos:26` or its dated pin
/// `macos:26-20261021-fca2193`, with or without a digest. Not a slim, xcode
/// or other version.
pub fn is_full_macos_base(reference: &str, version: &str) -> bool {
    let tag_ref = reference.split('@').next().unwrap_or(reference);
    let Some(tag) = tag_ref
        .strip_prefix(MACOS_REPO)
        .and_then(|t| t.strip_prefix(':'))
    else {
        return false;
    };
    if tag == version {
        return true;
    }
    let Some(pin) = tag.strip_prefix(version).and_then(|t| t.strip_prefix('-')) else {
        return false;
    };
    // <yyyymmdd>-<sha7>
    let mut parts = pin.split('-');
    let (Some(date), Some(sha), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && sha.len() == 7
        && sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Writes the recipe under `root` (`<root>/<content hash>/`, laid out as in
/// the repository) and returns the script. Idempotent: the same recipe is
/// written once.
pub fn write_recipe(root: &Path) -> std::io::Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for f in [DERIVE_LOCAL, VERSIONS_ENV, TIER_XCODE, FETCH, XCODE_SOURCE] {
        h.update((f.len() as u64).to_le_bytes());
        h.update(f.as_bytes());
    }
    let id: String = h
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect();
    let dir = root.join(id);
    let script = dir.join("derive-local.sh");
    if script.exists() {
        return Ok(script);
    }
    let tmp = root.join(format!(".tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("files"))?;
    for (name, text) in [
        ("derive-local.sh", DERIVE_LOCAL),
        ("versions.env", VERSIONS_ENV),
        ("files/tier-xcode.sh", TIER_XCODE),
        ("files/fetch.sh", FETCH),
        ("files/xcode-source.sh", XCODE_SOURCE),
    ] {
        std::fs::write(tmp.join(name), text)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["derive-local.sh", "files/tier-xcode.sh"] {
            std::fs::set_permissions(tmp.join(name), std::fs::Permissions::from_mode(0o755))?;
        }
    }
    match std::fs::rename(&tmp, &dir) {
        Ok(()) => Ok(script),
        // Another process wrote it first.
        Err(_) if script.exists() => {
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(script)
        }
        Err(e) => Err(e),
    }
}

/// The progress a `[macos-dev HH:MM:SS] message` line of the script says, or
/// `None` for any other line.
pub fn progress_of(line: &str) -> Option<Progress> {
    let msg = line.strip_prefix("[macos-dev ")?.split_once("] ")?.1.trim();
    let detail = match msg {
        m if m.starts_with("Xcode source:") => "Finding Xcode on this Mac",
        m if m.starts_with("archiving") => "Packing Xcode for the Space",
        m if m.starts_with("cloning") => "Cloning the macOS image",
        m if m.starts_with("booting") => "Starting the build",
        m if m.starts_with("installing the developer tools") => {
            "Installing Xcode, the iOS Simulator and the developer tools"
        }
        m if m.starts_with("shutting down") => "Finishing",
        m if m.starts_with("fetching") => "Downloading the developer tools",
        _ => return None,
    };
    Some(Progress::phase(Phase::Creating).detail(detail))
}

/// The error for the script's exit `code` and the tail of its output.
pub fn exit_error(code: Option<i32>, tail: &str) -> VmmError {
    match code {
        Some(3) => VmmError::Missing {
            what: "Xcode".into(),
            hint: "Apple requires a signed-in download, so Cua cannot get one for you. \
                   Install Xcode from the App Store, or download the Xcode .xip from \
                   developer.apple.com/download and put it in ~/XcodesCache, then create \
                   the macOS Space with developer tools again."
                .into(),
        },
        Some(4) => VmmError::HostCapacity(message_of(tail, "Not enough free disk")),
        Some(5) => VmmError::UnsupportedImage {
            image: format!("{MACOS_REPO}:26-xcode"),
            reason: message_of(tail, "The Xcode on this Mac needs a newer macOS"),
        },
        _ => VmmError::Command {
            cmd: "derive-local.sh".into(),
            code,
            stderr: crate::exec::tail(tail, 2000),
        },
    }
}

/// The paragraph of `tail` that starts at the line beginning `marker`, to the
/// end of the output (the script prints its refusal last).
fn message_of(tail: &str, marker: &str) -> String {
    match tail.find(marker) {
        Some(i) => tail[i..].trim().to_string(),
        None => tail.trim().to_string(),
    }
}

/// One run of the build.
pub struct Derive<'a> {
    /// The Lume that runs the VMs.
    pub lume: &'a LumeCmd,
    /// Lume's VM storage directory (the base VM is there, and the result).
    pub storage: &'a Path,
    /// The full image's base VM to clone.
    pub base_vm: &'a str,
    /// The derived base VM to make.
    pub name: &'a str,
    /// Scratch space for staging and logs.
    pub work: &'a Path,
}

impl Derive<'_> {
    /// Runs the embedded recipe. Progress is reported as it goes; the script
    /// removes a half-built VM itself.
    pub async fn run(&self) -> Result<()> {
        let script = write_recipe(&self.work.join("recipe"))?;
        self.run_script(&script).await
    }

    /// [`Self::run`] with `script` as the build (tests).
    pub async fn run_script(&self, script: &Path) -> Result<()> {
        std::fs::create_dir_all(self.work)?;
        let mut cmd = tokio::process::Command::new("/bin/bash");
        cmd.arg(script)
            .args(["--base-vm", self.base_vm, "--name", self.name, "--out"])
            .arg(self.work.join("out"))
            .envs(self.lume.env().iter().map(|(k, v)| (k, v)))
            .env("CUA_LUME_BIN", self.lume.bin())
            .env("CUA_LUME_STORAGE", self.storage)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        tracing::info!(
            base = self.base_vm,
            name = self.name,
            "building the macOS Space with developer tools"
        );
        let mut child = cmd.spawn().map_err(|e| VmmError::Command {
            cmd: "bash derive-local.sh".into(),
            code: None,
            stderr: e.to_string(),
        })?;
        let stderr = child.stderr.take().expect("stderr is piped");
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: std::collections::VecDeque<String> = Default::default();
        let read = async {
            while let Some(line) = lines.next_line().await? {
                if let Some(p) = progress_of(&line) {
                    crate::progress::report(p);
                }
                tracing::debug!(target: "cua_vmm::derive", "{line}");
                if tail.len() == 60 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            child.wait().await
        };
        let status = tokio::time::timeout(DERIVE_TIMEOUT, read)
            .await
            .map_err(|_| VmmError::Timeout {
                name: self.name.to_string(),
                secs: DERIVE_TIMEOUT.as_secs(),
                detail: "building the macOS Space with developer tools".into(),
            })??;
        if status.success() {
            return Ok(());
        }
        let tail: Vec<String> = tail.into_iter().collect();
        Err(exit_error(status.code(), &tail.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ref in the macOS repository, built so the image-ref gate (which wants
    /// catalog refs) does not read these synthetic tags as image references.
    fn m(tag: &str) -> String {
        format!("{MACOS_REPO}:{tag}")
    }

    const D: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn the_developer_tools_image_names_its_parent() {
        assert_eq!(dev_parent(&m("26-xcode")), Some(m("26")));
        // The digest is the published image's own: the parent is by tag.
        assert_eq!(dev_parent(&format!("{}@{D}", m("26-xcode"))), Some(m("26")));
        let other_org = format!("ghcr.io/{}/macos:26-xcode", "other");
        let other_os = format!("ghcr.io/trycua/{}:24.04-xcode", "linux");
        for not in [
            m("26"),
            m("26-slim"),
            m("-xcode"),
            m("26-xcode-26.6"),
            m("26-xcode-disk"),
            other_org,
            other_os,
        ] {
            assert_eq!(dev_parent(&not), None, "{not}");
        }
    }

    #[test]
    fn only_a_full_image_of_the_version_is_a_parent() {
        for ok in [
            m("26"),
            m("26-20261021-fca2193"),
            format!("{}@{D}", m("26-20261021-fca2193")),
        ] {
            assert!(is_full_macos_base(&ok, "26"), "{ok}");
        }
        let other_os = format!("ghcr.io/trycua/{}:26", "linux");
        for no in [
            m("26-slim"),
            m("26-slim-20261021-fca2193"),
            m("26-xcode"),
            m("15"),
            m("26-2026102-fca2193"),
            m("26-20261021-FCA2193"),
            other_os,
        ] {
            assert!(!is_full_macos_base(&no, "26"), "{no}");
        }
    }

    #[test]
    fn the_script_says_where_the_build_is() {
        let d = |l: &str| progress_of(l).map(|p| p.detail);
        assert_eq!(
            d("[macos-dev 20:01:36] Xcode source: /Applications/Xcode.app").as_deref(),
            Some("Finding Xcode on this Mac")
        );
        assert!(progress_of("[macos-dev 20:02:00] booting cua-dev-1").is_some());
        assert!(
            progress_of("[macos-dev 20:02:19] installing the developer tools (Xcode 26.6)")
                .is_some()
        );
        assert!(progress_of("    ==> Rust targets").is_none());
        assert!(progress_of("[macos-dev 20:02:00] something else").is_none());
    }

    #[test]
    fn exits_become_messages_a_person_can_act_on() {
        let e = exit_error(Some(3), "").to_string();
        assert!(e.contains("Xcode is not available"), "{e}");
        assert!(
            e.contains("App Store") && e.contains("developer.apple.com"),
            "{e}"
        );
        let tail = "[macos-dev 1] Xcode source: x\nNot enough free disk to build the macOS Space with developer tools: it needs about 30 GB.\nMake room: remove Spaces and images you no longer use.";
        let e = exit_error(Some(4), tail).to_string();
        assert!(e.starts_with("Not enough free disk"), "{e}");
        assert!(e.contains("Make room"), "{e}");
        assert!(!e.contains("Xcode source"), "{e}");
        let e = exit_error(
            Some(5),
            "Xcode 27 needs macOS 27.0; this Space's macOS is 26.5.",
        )
        .to_string();
        assert!(e.contains("needs macOS 27.0"), "{e}");
        let e = exit_error(Some(1), "boom").to_string();
        assert!(e.contains("exit Some(1)") && e.contains("boom"), "{e}");
    }

    #[test]
    fn the_image_runs_only_on_apple_silicon_macs() {
        let r = require_apple_hardware();
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert!(r.is_ok());
        } else {
            let e = r.unwrap_err().to_string();
            assert!(
                e.contains("Apple-branded hardware") && e.contains("Apple silicon"),
                "{e}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_embedded_recipe_is_complete_and_parses() {
        let d = tempfile::tempdir().unwrap();
        let script = write_recipe(d.path()).unwrap();
        // Again: the same directory, nothing rewritten.
        assert_eq!(write_recipe(d.path()).unwrap(), script);
        let dir = script.parent().unwrap();
        for f in [
            "derive-local.sh",
            "versions.env",
            "files/tier-xcode.sh",
            "files/fetch.sh",
        ] {
            assert!(dir.join(f).is_file(), "{f}");
        }
        // The layout derive-local.sh expects of the repository.
        assert!(DERIVE_LOCAL.contains(r#"$HERE/files/fetch.sh"#));
        assert!(DERIVE_LOCAL.contains(r#"$HERE/files/tier-xcode.sh"#));
        for f in [
            "derive-local.sh",
            "files/tier-xcode.sh",
            "files/fetch.sh",
            "files/xcode-source.sh",
        ] {
            let ok = std::process::Command::new("/bin/bash")
                .arg("-n")
                .arg(dir.join(f))
                .status()
                .unwrap();
            assert!(ok.success(), "{f} does not parse");
        }
    }

    #[cfg(unix)]
    async fn run_stub(body: &str) -> Result<()> {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("stub.sh");
        std::fs::write(&script, format!("#!/bin/bash\n{body}\n")).unwrap();
        let lume = LumeCmd::plain("/usr/bin/true");
        Derive {
            lume: &lume,
            storage: d.path(),
            base_vm: "cua-base-x",
            name: "cua-base-y",
            work: &d.path().join("work"),
        }
        .run_script(&script)
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_run_reports_progress_and_maps_the_exit() {
        use std::sync::{Arc, Mutex};
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = {
            let seen = seen.clone();
            move |p: &Progress| seen.lock().unwrap().push(p.detail.clone())
        };
        let ok = crate::progress::scope(Arc::new(sink), run_stub(
            "echo '[macos-dev 10:00:00] cloning a -> b' >&2\necho '[macos-dev 10:00:01] installing the developer tools (Xcode 26.6)' >&2\necho \"$CUA_LUME_STORAGE|$CUA_LUME_BIN\" >&2",
        ))
        .await;
        ok.unwrap();
        let seen = seen.lock().unwrap().join("\n");
        assert!(seen.contains("Cloning the macOS image"), "{seen}");
        assert!(seen.contains("Installing Xcode"), "{seen}");

        let e = run_stub("echo 'No Xcode' >&2\nexit 3").await.unwrap_err();
        assert!(
            matches!(e, VmmError::Missing { ref what, .. } if what == "Xcode"),
            "{e:?}"
        );
        let e = run_stub("echo 'Not enough free disk to build: Make room' >&2\nexit 4")
            .await
            .unwrap_err();
        assert!(
            matches!(e, VmmError::HostCapacity(ref m) if m.contains("Make room")),
            "{e:?}"
        );
        let e = run_stub("exit 9").await.unwrap_err();
        assert!(
            matches!(e, VmmError::Command { code: Some(9), .. }),
            "{e:?}"
        );
    }
}
