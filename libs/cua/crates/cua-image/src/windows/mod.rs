//! Windows images built on this computer from an evaluation ISO.
//!
//! `windows` / `windows:2022` is the published containerDisk. The other
//! Windows versions (`windows:11`, `windows:2025`, ...) have no published
//! image: the first Space made from one installs Windows unattended from an
//! ISO (yours, or Microsoft's evaluation media), seals the result as a base
//! disk under `$CUA_HOME/images/windows/<version>-<arch>/`, and every later
//! Space is a clone of it. See `docs/windows-images.md`.
//!
//! * [`table`] - the version table and alias normalisation.
//! * [`iso`] - ISO resolution: your path, then the cache, then a download.
//! * [`unattend`] - the answer file, the in-guest setup script, the data ISO.
//! * [`build`] - the build plan, cache layout and orchestration.

pub mod build;
pub mod iso;
pub mod joliet;
pub mod msdl;
pub mod native;
pub mod table;
pub mod unattend;
pub mod wsl;

pub use table::{WindowsArch, WindowsVersion, lookup, normalize_version, versions};

/// How a build runs on a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallerKind {
    /// The runtime's own QEMU: KVM on Linux, HVF on a Mac.
    Qemu,
    /// QEMU with KVM inside WSL2, on a Windows PC.
    Wsl2,
}

/// The installer for `host`: a Windows PC builds inside WSL2.
pub fn installer_kind(host: cua_vmm::host::HostOs) -> InstallerKind {
    match host {
        cua_vmm::host::HostOs::Windows => InstallerKind::Wsl2,
        _ => InstallerKind::Qemu,
    }
}

/// The disk of a `windows:<version>` local build, building it first when it is
/// not there (the first Space of that version). `None` when `reference` is
/// not a Windows local build (a published image is pulled, not built).
pub async fn ensure_local_base(
    reference: &str,
    arch: WindowsArch,
) -> crate::error::Result<Option<std::path::PathBuf>> {
    let Some(version) = build::local_build_target(reference) else {
        return Ok(None);
    };
    let layout = build::Layout::default_root();
    let env = |k: &str| std::env::var(k).ok();
    let req = build::BuildRequest::from_env(version, arch, &layout, &env)?;
    // A build that could only run in software emulation is refused now, before
    // a multi-gigabyte ISO is downloaded for nothing (a cached base is fine).
    if build::cached_base(&layout, version, arch).is_none() {
        native::refuse_software_emulation(arch, WindowsArch::host())?;
    }
    let fetcher = iso::HttpFetcher;
    let disk = match installer_kind(cua_vmm::host::HostOs::current()) {
        InstallerKind::Wsl2 => {
            build::ensure_base(&layout, &req, &fetcher, &wsl::WslInstaller::new()).await?
        }
        InstallerKind::Qemu => {
            build::ensure_base(&layout, &req, &fetcher, &native::QemuInstaller::new()).await?
        }
    };
    Ok(Some(disk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_pc_builds_in_wsl_and_everything_else_in_qemu() {
        use cua_vmm::host::HostOs;
        assert_eq!(installer_kind(HostOs::Windows), InstallerKind::Wsl2);
        assert_eq!(installer_kind(HostOs::Macos), InstallerKind::Qemu);
        assert_eq!(installer_kind(HostOs::Linux), InstallerKind::Qemu);
    }

    #[tokio::test]
    async fn a_build_that_would_be_emulated_is_refused_before_any_download() {
        // The other architecture than this host's: Server 2025 is x64 only, so
        // on an ARM host it is the foreign one; elsewhere ARM64 Windows 11 is.
        let (reference, arch) = if WindowsArch::host() == WindowsArch::Arm64 {
            ("ghcr.io/trycua/windows:2025", WindowsArch::Amd64)
        } else {
            ("ghcr.io/trycua/windows:11", WindowsArch::Arm64)
        };
        if cua_vmm::qemu::software_emulation_allowed() {
            return;
        }
        let e = ensure_local_base(reference, arch).await.unwrap_err().to_string();
        assert!(e.contains("software emulation") && e.contains("hours"), "{e}");
    }

    #[tokio::test]
    async fn a_published_image_is_not_built() {
        for r in ["ghcr.io/trycua/windows:2022", "ghcr.io/trycua/linux:24.04"] {
            assert_eq!(
                ensure_local_base(r, WindowsArch::Amd64).await.unwrap(),
                None
            );
        }
    }
}
