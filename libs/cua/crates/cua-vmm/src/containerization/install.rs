//! Apple's `container` tooling, cua's own copy: the signed release package
//! pinned by version and SHA-256, expanded (no installer, no root) into
//! `<root>/install/<version>`, its services run with cua's own install and
//! application roots so they never use a user's own `container` setup.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Result, VmmError};
use crate::host;
use crate::types::Arch;

/// The pinned `container` release.
pub const VERSION: &str = "1.5.0";

/// SHA-256 of `container-<VERSION>-installer-signed.pkg` (signed by
/// "Developer ID Installer: Apple Inc. - Containerization" and notarized).
pub const PKG_SHA256: &str = "a24808cb202318fa1c3bbee0c6c6887fe1225fe899d7b687a0ddd939bd6573f8";

/// Its size, for the doctor's plan.
pub const PKG_BYTES: u64 = 118_045_087;

/// The oldest macOS the backend supports (Containerization needs macOS 26
/// for its networking).
pub const MIN_MACOS: u32 = 26;

pub fn pkg_url() -> String {
    format!(
        "https://github.com/apple/container/releases/download/{VERSION}/container-{VERSION}-installer-signed.pkg"
    )
}

/// An Apple silicon Mac on macOS 26 or later.
pub fn supported_host() -> bool {
    host::HostOs::current() == host::HostOs::Macos
        && Arch::host() == Arch::Aarch64
        && host::macos_major().is_some_and(|m| m >= MIN_MACOS)
}

/// Why this host cannot run the backend, if it cannot.
pub fn unsupported_reason() -> Option<String> {
    if host::HostOs::current() != host::HostOs::Macos || Arch::host() != Arch::Aarch64 {
        return Some(
            "Apple's Containerization runs on Apple silicon Macs (macOS 26 or later)".into(),
        );
    }
    match host::macos_major() {
        Some(m) if m >= MIN_MACOS => None,
        _ => Some(format!(
            "Apple's Containerization needs macOS {MIN_MACOS} or later (this Mac runs {})",
            host::macos_version().unwrap_or_else(|| "an unknown version".into())
        )),
    }
}

/// `<root>/install/<VERSION>`: the expanded package (`bin/`, `libexec/`).
pub fn install_root(root: &Path) -> PathBuf {
    root.join("install").join(VERSION)
}

/// `<root>/data`: images, containers, kernels.
pub fn app_root(root: &Path) -> PathBuf {
    root.join("data")
}

/// The installed `container` binary, if installed.
pub fn installed_bin(root: &Path) -> Option<PathBuf> {
    let b = install_root(root).join("bin").join("container");
    b.is_file().then_some(b)
}

/// Downloads the pinned package (verified as it streams) and expands it into
/// [`install_root`]. Idempotent; returns the `container` binary.
pub async fn install(root: &Path) -> Result<PathBuf> {
    if let Some(b) = installed_bin(root) {
        return Ok(b);
    }
    if let Some(why) = unsupported_reason() {
        return Err(VmmError::other(why));
    }
    let staging = root
        .join("install")
        .join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = async {
        let pkg = staging.join("container.pkg");
        download(&pkg_url(), &pkg, PKG_SHA256).await?;
        let expanded = staging.join("x");
        let out = tokio::process::Command::new("/usr/sbin/pkgutil")
            .arg("--expand-full")
            .arg(&pkg)
            .arg(&expanded)
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmmError::Command {
                cmd: "pkgutil --expand-full".into(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        let payload = expanded.join("Payload");
        if !payload.join("bin").join("container").is_file() {
            return Err(VmmError::other(
                "the container package has no bin/container in its payload",
            ));
        }
        let dest = install_root(root);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&payload, &dest)?;
        Ok(dest.join("bin").join("container"))
    }
    .await;
    let _ = std::fs::remove_dir_all(&staging);
    let bin = result?;
    tracing::info!(bin = %bin.display(), version = VERSION, "installed Apple container");
    Ok(bin)
}

/// Streams `url` into `dest`, checking its SHA-256 against `sha256`.
async fn download(url: &str, dest: &Path, sha256: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    tracing::info!(url, dest = %dest.display(), "downloading");
    let fail = |e: reqwest::Error| VmmError::other(format!("download {url}: {e}"));
    let mut resp = reqwest::get(url)
        .await
        .map_err(fail)?
        .error_for_status()
        .map_err(fail)?;
    let mut f = tokio::fs::File::create(dest).await?;
    let mut h = Sha256::new();
    while let Some(chunk) = resp.chunk().await.map_err(fail)? {
        h.update(&chunk);
        f.write_all(&chunk).await?;
    }
    f.flush().await?;
    let got = format!("{:x}", h.finalize());
    if got != sha256 {
        let _ = std::fs::remove_file(dest);
        return Err(VmmError::other(format!(
            "download {url}: SHA-256 {got} does not match the pinned {sha256}"
        )));
    }
    Ok(())
}

pub use crate::auto::ContainerizationReport;
