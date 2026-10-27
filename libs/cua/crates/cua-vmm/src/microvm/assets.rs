//! What a microVM host needs, and where it comes from: the Firecracker
//! binary (a pinned release), the guest kernel (Firecracker's CI 6.1
//! build, pinned by SHA-256; its arm64 build also boots under
//! Virtualization.framework), the guest init (`cua-microvm-init`, shipped
//! next to `cua`), `pasta` (passt) for unprivileged networking, and
//! `mkfs.ext4`/`debugfs` (and `sqfstar` for squashfs bases) for disk images.
//! A Mac needs `cua-vz` and the network gateway instead ([`super::vz`],
//! [`crate::netgw`]).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Result, VmmError};
use crate::host;
use crate::types::Arch;

/// The pinned Firecracker release.
pub const FIRECRACKER_VERSION: &str = "v1.17.0";

/// The release tarball's SHA-256 per architecture.
fn firecracker_sha256(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558",
        Arch::Aarch64 => "e351ebe4f7a16b5873bbd51005d2e6767103cff4d5ebc829df2d3f95a93e2256",
    }
}

/// The pinned guest kernel (Firecracker CI's 6.1 configuration: virtio
/// block/net/vsock, squashfs, overlayfs, ext4, VMGenID).
pub const KERNEL_VERSION: &str = "6.1.155";

fn kernel_sha256(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2",
        Arch::Aarch64 => "e3544b10603acbf3db492cb52e000d22ba202cb4b63b9add027565683e11c591",
    }
}

fn kernel_url(arch: Arch) -> String {
    format!(
        "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/{}/vmlinux-{KERNEL_VERSION}",
        arch.qemu()
    )
}

fn firecracker_url(arch: Arch) -> String {
    format!(
        "https://github.com/firecracker-microvm/firecracker/releases/download/{FIRECRACKER_VERSION}/firecracker-{FIRECRACKER_VERSION}-{}.tgz",
        arch.qemu()
    )
}

/// The `firecracker` binary: `$CUA_FIRECRACKER`, then `~/.cua/bin`, then
/// `PATH`.
pub fn firecracker_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_FIRECRACKER").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let own = host::cua_home().join("bin").join("firecracker");
    if own.is_file() {
        return Some(own);
    }
    host::which("firecracker")
}

/// `pasta` (passt), which gives a microVM its network without root:
/// `$CUA_PASTA`, then `~/.cua/bin`, then `PATH`. passt publishes no
/// versioned static build to pin, so cua does not download it: a host
/// without it gets [`pasta_missing`].
pub fn pasta_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_PASTA").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let own = host::cua_home().join("bin").join("pasta");
    if own.is_file() {
        return Some(own);
    }
    host::which("pasta")
}

/// The one line a host without passt gets, in the app and the CLI alike.
pub const PASTA_HINT: &str = "microVM Spaces need passt for their network. Install it \
     (Ubuntu and Debian: sudo apt-get install passt; Fedora: sudo dnf install passt), then \
     create the Space again";

/// The error for a host without passt: what to install, never a raw
/// "not found".
pub fn pasta_missing() -> VmmError {
    VmmError::missing("passt", PASTA_HINT)
}

/// The guest init: `$CUA_MICROVM_INIT`, next to the running executable (or
/// in an app bundle's `Contents/Resources`, since a Linux binary cannot sit
/// among a Mac app's signed executables), `~/.cua/bin`, then `PATH`. It
/// must be a static Linux binary.
pub fn init_bin() -> Option<PathBuf> {
    const NAME: &str = "cua-microvm-init";
    if let Some(p) = std::env::var_os("CUA_MICROVM_INIT").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        for p in [dir.join(NAME), dir.join("../Resources").join(NAME)] {
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let own = host::cua_home().join("bin").join(NAME);
    if own.is_file() {
        return Some(own);
    }
    host::which(NAME)
}

/// Where the kernel is cached.
pub fn kernel_path(root: &Path) -> PathBuf {
    root.join("kernel")
        .join(format!("vmlinux-{KERNEL_VERSION}-{}", Arch::host().qemu()))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

async fn download(url: &str, dest: &Path, sha256: &str) -> Result<()> {
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    tracing::info!(url, dest = %dest.display(), "downloading");
    let resp = reqwest::get(url)
        .await
        .map_err(|e| VmmError::other(format!("download {url}: {e}")))?
        .error_for_status()
        .map_err(|e| VmmError::other(format!("download {url}: {e}")))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| VmmError::other(format!("download {url}: {e}")))?;
    let got = format!("{:x}", Sha256::digest(&bytes));
    if got != sha256 {
        return Err(VmmError::other(format!(
            "download {url}: SHA-256 {got} does not match the pinned {sha256}"
        )));
    }
    let partial = dest.with_extension("partial");
    tokio::fs::write(&partial, &bytes).await?;
    tokio::fs::rename(&partial, dest).await?;
    Ok(())
}

/// The guest kernel, downloaded (and verified) on first use.
pub async fn ensure_kernel(root: &Path) -> Result<PathBuf> {
    let p = kernel_path(root);
    if p.is_file() {
        return Ok(p);
    }
    crate::progress::report(
        crate::progress::Progress::phase(crate::progress::Phase::Preparing)
            .detail(format!("Downloading the microVM kernel {KERNEL_VERSION}")),
    );
    let arch = Arch::host();
    download(&kernel_url(arch), &p, kernel_sha256(arch)).await?;
    if sha256_file(&p)? != kernel_sha256(arch) {
        let _ = std::fs::remove_file(&p);
        return Err(VmmError::other(
            "the downloaded kernel does not match its pin",
        ));
    }
    Ok(p)
}

/// The `firecracker` binary, installed on first use: the pinned release
/// into `~/.cua/bin`, as the kernel is fetched, with the create's stage
/// saying so. Only on Linux (macOS microVMs use `cua-vz`).
pub async fn ensure_firecracker() -> Result<PathBuf> {
    ensure_firecracker_with(firecracker_bin, install_firecracker).await
}

/// [`ensure_firecracker`] with the lookup and the installer given (tests).
pub async fn ensure_firecracker_with(
    find: impl Fn() -> Option<PathBuf>,
    install: impl AsyncFnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    if let Some(p) = find() {
        return Ok(p);
    }
    crate::progress::report(
        crate::progress::Progress::phase(crate::progress::Phase::Preparing).detail(format!(
            "Setting up microVMs: installing Firecracker {FIRECRACKER_VERSION}"
        )),
    );
    let bin = install().await?;
    tracing::info!(bin = %bin.display(), "installed Firecracker on first use");
    Ok(bin)
}

/// Installs the pinned Firecracker release into `~/.cua/bin`
/// (`cua runtime setup firecracker`, and the first microVM create).
pub async fn install_firecracker() -> Result<PathBuf> {
    if std::env::consts::OS != "linux" {
        return Err(VmmError::Unsupported {
            backend: "firecracker",
            op: "a host other than Linux (Firecracker needs KVM)",
        });
    }
    let arch = Arch::host();
    let dir = host::cua_home().join("bin");
    std::fs::create_dir_all(&dir)?;
    let tgz = dir.join(format!("firecracker-{FIRECRACKER_VERSION}.tgz"));
    download(&firecracker_url(arch), &tgz, firecracker_sha256(arch)).await?;
    let member = format!(
        "release-{FIRECRACKER_VERSION}-{a}/firecracker-{FIRECRACKER_VERSION}-{a}",
        a = arch.qemu()
    );
    let out = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tgz)
        .arg("-C")
        .arg(&dir)
        .arg(&member)
        .output()
        .await?;
    let _ = std::fs::remove_file(&tgz);
    if !out.status.success() {
        return Err(VmmError::Command {
            cmd: "tar".into(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let bin = dir.join("firecracker");
    std::fs::rename(dir.join(&member), &bin)?;
    let _ =
        std::fs::remove_dir_all(dir.join(format!("release-{FIRECRACKER_VERSION}-{}", arch.qemu())));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(bin)
}

pub use crate::auto::MicroVmReport;

/// Inspects the host (read-only).
pub async fn report(root: &Path) -> MicroVmReport {
    let firecracker = firecracker_bin();
    let firecracker_version = match &firecracker {
        Some(b) => host::run(b, &["--version"])
            .await
            .ok()
            .and_then(|s| s.lines().next().map(|l| l.trim().to_string())),
        None => None,
    };
    MicroVmReport {
        kvm: std::env::consts::OS == "linux" && host::kvm_usable().is_ok(),
        firecracker,
        firecracker_version,
        init: init_bin(),
        pasta: pasta_bin(),
        sqfstar: host::which("sqfstar"),
        debugfs: host::which("debugfs"),
        init_static: init_bin()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| super::rootfs::elf_interpreter(&b))
            .is_some_and(|interp| interp.is_none()),
        mkfs_ext4: host::which("mkfs.ext4"),
        iptables: host::which("iptables"),
        ip: host::which("ip"),
        kernel_cached: kernel_path(root).is_file(),
        userns: userns_allowed(),
    }
}

/// Whether this user may create user namespaces (Ubuntu 24.04 restricts
/// them through AppArmor unless a profile allows the binary).
fn userns_allowed() -> bool {
    if std::env::consts::OS != "linux" {
        return false;
    }
    let read = |p: &str| {
        std::fs::read_to_string(p)
            .map(|s| s.trim().to_string())
            .ok()
    };
    if read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") {
        return false;
    }
    if read("/proc/sys/user/max_user_namespaces").as_deref() == Some("0") {
        return false;
    }
    // AppArmor's restriction applies to binaries without a profile; pasta
    // ships one on Ubuntu, so this only warns.
    true
}

#[cfg(test)]
mod first_use_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn capture() -> (Arc<Mutex<Vec<String>>>, crate::progress::Sink) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let sink: crate::progress::Sink = Arc::new(move |p: &crate::progress::Progress| {
            s.lock()
                .unwrap()
                .push(format!("{} {}", p.phase.as_str(), p.detail));
        });
        (seen, sink)
    }

    #[tokio::test]
    async fn firecracker_installs_on_first_use_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("firecracker");
        let (seen, sink) = capture();
        let installed = Arc::new(Mutex::new(0));
        let n = installed.clone();
        let b = bin.clone();
        let got = crate::progress::scope(sink, async {
            ensure_firecracker_with(
                || None,
                async move || {
                    *n.lock().unwrap() += 1;
                    std::fs::write(&b, b"#!/bin/sh\n").unwrap();
                    Ok(b.clone())
                },
            )
            .await
        })
        .await
        .unwrap();
        assert_eq!(got, bin);
        assert_eq!(*installed.lock().unwrap(), 1);
        let lines = seen.lock().unwrap().join("\n");
        assert!(lines.contains("installing Firecracker"), "{lines}");
    }

    #[tokio::test]
    async fn an_installed_firecracker_is_used_as_is() {
        let p = PathBuf::from("/opt/fc/firecracker");
        let q = p.clone();
        let got = ensure_firecracker_with(
            move || Some(q.clone()),
            async || -> Result<PathBuf> { panic!("no install when it is there") },
        )
        .await
        .unwrap();
        assert_eq!(got, p);
    }

    #[tokio::test]
    async fn a_failed_install_is_the_error() {
        let e = ensure_firecracker_with(|| None, async || Err(VmmError::other("offline")))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("offline"), "{e}");
    }

    #[test]
    fn without_passt_the_error_says_what_to_install() {
        let e = pasta_missing().to_string();
        assert!(e.contains("sudo apt-get install passt"), "{e}");
        assert!(e.contains("sudo dnf install passt"), "{e}");
        assert!(!e.contains("not found"), "{e}");
    }
}
