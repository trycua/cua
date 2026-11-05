//! Building a Windows base disk on this computer, once.
//!
//! `ensure_base` is what the first Space of `windows:11` (or any local-build
//! version) runs: find the ISO, check the disk, install Windows unattended in
//! a VM until cua-spacesd answers, power it off, and keep the disk as the base
//! under `$CUA_HOME/images/windows/<version>-<arch>/`. Every later Space is a
//! clone of that disk and finds it already there.
//!
//! The VM itself is behind [`Installer`]: [`super::native`] runs QEMU with the
//! host's accelerator (KVM, HVF), [`super::wsl`] runs QEMU with KVM inside WSL2
//! on a Windows PC. Tests use a fake.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::iso::{self, IsoFetcher, IsoRequest};
use super::table::{WindowsArch, WindowsVersion};
use super::unattend::{self, UnattendOptions};
use crate::error::{ImageError, Result};

/// A finished base is at least this big (an interrupted install is not).
pub const MIN_BASE_BYTES: u64 = 4 << 30;
/// What a build writes beyond the finished disk: the ISO copy, the data
/// discs, and the install's scratch. The disk itself is sparse.
pub const BUILD_SCRATCH_BYTES: u64 = 24 << 30;
/// The most an install may take. With hardware acceleration it is 15 to 40
/// minutes; this is the wall where a stuck one is given up on.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// A build lock older than this is a crashed build's.
const STALE_LOCK: Duration = Duration::from_secs(4 * 60 * 60);

/// virtio-win drivers (storage, network) for the install, from Fedora.
pub const VIRTIO_URL: &str =
    "https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso";

/// Where the images live: `$CUA_HOME/images/windows`.
#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$CUA_HOME/images/windows`.
    pub fn default_root() -> Self {
        Self::new(cua_vmm::host::cua_home().join("images").join("windows"))
    }

    /// The ISO cache.
    pub fn iso_dir(&self) -> PathBuf {
        self.root.join("iso")
    }

    /// `<root>/<version>-<arch>`.
    pub fn dir(&self, v: &WindowsVersion, arch: WindowsArch) -> PathBuf {
        self.root.join(format!("{}-{}", v.id, arch.as_str()))
    }

    /// The sealed base disk.
    pub fn disk(&self, v: &WindowsVersion, arch: WindowsArch) -> PathBuf {
        self.dir(v, arch).join("base.qcow2")
    }

    fn manifest(&self, v: &WindowsVersion, arch: WindowsArch) -> PathBuf {
        self.dir(v, arch).join("base.json")
    }
}

/// What a finished base records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseManifest {
    pub version: String,
    pub arch: String,
    pub iso_sha256: String,
    pub account: String,
    pub spacesd: String,
    pub built_at: u64,
    pub disk_bytes: u64,
}

/// The base disk, when a complete build is there.
pub fn cached_base(layout: &Layout, v: &WindowsVersion, arch: WindowsArch) -> Option<PathBuf> {
    let disk = layout.disk(v, arch);
    let size = std::fs::metadata(&disk).ok()?.len();
    let m: BaseManifest =
        serde_json::from_slice(&std::fs::read(layout.manifest(v, arch)).ok()?).ok()?;
    (size >= MIN_BASE_BYTES && m.disk_bytes == size).then_some(disk)
}

/// What to build.
#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub version: &'static WindowsVersion,
    pub arch: WindowsArch,
    /// Your ISO (`--iso`, or the one registered for this version).
    pub user_iso: Option<PathBuf>,
    pub iso_sha256: Option<String>,
    /// The local administrator (`CUA_WINDOWS_USER`).
    pub account: String,
    /// Install this edition by name (consumer media).
    pub edition: Option<String>,
    pub cpus: u32,
    pub memory_mb: u64,
}

impl BuildRequest {
    /// The request for `version` on this host, reading `CUA_WINDOWS_ISO`,
    /// `CUA_WINDOWS_ISO_SHA256`, `CUA_WINDOWS_EDITION` and `CUA_WINDOWS_USER`
    /// through `env`, and the ISO registered for the version in `layout`.
    pub fn from_env(
        version: &'static WindowsVersion,
        arch: WindowsArch,
        layout: &Layout,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let account = unattend::account_name(None, env("CUA_WINDOWS_USER").as_deref())
            .map_err(|e| ImageError::Reference(format!("windows:{}", version.id), e))?;
        let user_iso = env("CUA_WINDOWS_ISO")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| iso::registered(&layout.iso_dir(), version, arch));
        Ok(Self {
            version,
            arch,
            user_iso,
            iso_sha256: env("CUA_WINDOWS_ISO_SHA256").filter(|s| !s.trim().is_empty()),
            account,
            edition: env("CUA_WINDOWS_EDITION").filter(|s| !s.trim().is_empty()),
            cpus: 4,
            memory_mb: if arch == WindowsArch::Arm64 { 8192 } else { 6144 },
        })
    }
}

/// Everything an installer needs, all of it on disk already.
#[derive(Clone, Debug)]
pub struct InstallPlan {
    pub name: String,
    pub arch: WindowsArch,
    pub windows_iso: PathBuf,
    pub data_iso: PathBuf,
    pub virtio_iso: PathBuf,
    /// The blank disk size in GiB.
    pub disk_gb: u32,
    pub cpus: u32,
    pub memory_mb: u64,
    pub work_dir: PathBuf,
    pub timeout: Duration,
}

/// Runs the install: boots the VM on the plan's discs and returns the
/// installed disk (a standalone qcow2) once cua-spacesd answered in the guest
/// and the guest was powered off.
#[async_trait]
pub trait Installer: Send + Sync {
    async fn install(&self, plan: &InstallPlan) -> Result<PathBuf>;
}

fn progress(detail: impl Into<String>, fraction: f64) {
    use cua_vmm::progress::{Phase, Progress, report};
    report(
        Progress::phase(Phase::Creating)
            .detail(detail)
            .fraction(fraction),
    );
}

/// Holds `dir/build.lock` while a build runs, so two creates of the same image
/// do not install Windows twice. A lock a crashed build left is taken over.
struct Lock(PathBuf);

impl Lock {
    fn take(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("build.lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .unwrap_or_default();
                    if age > STALE_LOCK {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    return Err(ImageError::Build {
                        step: "windows".into(),
                        detail: format!(
                            "another build of this image is running ({}); wait for it, or delete that file if no build is running",
                            path.display()
                        ),
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(ImageError::Build {
            step: "windows".into(),
            detail: "could not take the build lock".into(),
        })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The base disk for `req`, building it first when it is not there.
///
/// `fetcher` finds the Windows ISO and the virtio drivers; `installer` runs
/// the VM. Refuses before anything is downloaded when the disk is too full.
pub async fn ensure_base(
    layout: &Layout,
    req: &BuildRequest,
    fetcher: &dyn IsoFetcher,
    installer: &dyn Installer,
) -> Result<PathBuf> {
    ensure_base_with(layout, req, fetcher, installer, &cua_vmm::disk::SystemProbe).await
}

/// [`ensure_base`] against an explicit free-space source.
pub async fn ensure_base_with(
    layout: &Layout,
    req: &BuildRequest,
    fetcher: &dyn IsoFetcher,
    installer: &dyn Installer,
    probe: &dyn cua_vmm::disk::SpaceProbe,
) -> Result<PathBuf> {
    let (v, arch) = (req.version, req.arch);
    v.check_arch(arch)?;
    if let Some(d) = cached_base(layout, v, arch) {
        return Ok(d);
    }
    // Disk guard: refuse now, not when Windows runs out of room in the middle.
    check_disk(layout, v, probe)?;
    let dir = layout.dir(v, arch);
    let _lock = Lock::take(&dir)?;
    // Another process may have finished while this one waited for nothing.
    if let Some(d) = cached_base(layout, v, arch) {
        return Ok(d);
    }

    progress(format!("Finding the {} ISO", v.name), 0.0);
    let mut got = 0u64;
    let isop = iso::resolve_iso(
        &IsoRequest {
            version: v,
            arch,
            user_iso: req.user_iso.as_deref(),
            cache_dir: &layout.iso_dir(),
            sha256: req.iso_sha256.as_deref(),
        },
        fetcher,
        &mut |n| {
            got += n;
            use cua_vmm::progress::{Phase, Progress, report};
            report(
                Progress::phase(Phase::Pulling)
                    .detail(format!("Downloading {} evaluation ISO", v.name)),
            );
        },
    )
    .await?;
    let evaluation_media = isop.origin != iso::IsoOrigin::User;
    progress("Preparing the unattended install", 0.05);
    let work = dir.join("work");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(layout.iso_dir())?;
    let virtio = layout.iso_dir().join("virtio-win.iso");
    if !virtio.is_file() {
        fetcher.download(VIRTIO_URL, &virtio, &mut |_| {}).await?;
    }
    let data_iso = work.join("unattend.iso");
    let disc = unattend::data_disc(&UnattendOptions {
        version: v,
        arch,
        account: &req.account,
        edition: req.edition.as_deref(),
        evaluation_media,
        virtio: true,
    });
    std::fs::write(&data_iso, disc)?;

    progress(
        format!("Installing {} (about 20 to 40 minutes, once)", v.name),
        0.1,
    );
    let plan = InstallPlan {
        name: format!("cua-build-windows-{}-{}", v.id, arch.as_str()),
        arch,
        windows_iso: isop.path.clone(),
        data_iso,
        virtio_iso: virtio,
        disk_gb: v.disk_gb,
        cpus: req.cpus,
        memory_mb: req.memory_mb,
        work_dir: work.clone(),
        timeout: INSTALL_TIMEOUT,
    };
    let installed = installer.install(&plan).await?;
    let size = std::fs::metadata(&installed)?.len();
    if size < MIN_BASE_BYTES {
        return Err(ImageError::Build {
            step: "windows install".into(),
            detail: format!(
                "the installed disk is {size} bytes: the install did not finish (look at {})",
                work.display()
            ),
        });
    }
    progress("Sealing the image", 0.95);
    let disk = layout.disk(v, arch);
    if disk.exists() {
        std::fs::remove_file(&disk)?;
    }
    if std::fs::rename(&installed, &disk).is_err() {
        std::fs::copy(&installed, &disk)?;
        let _ = std::fs::remove_file(&installed);
    }
    let manifest = BaseManifest {
        version: v.id.into(),
        arch: arch.as_str().into(),
        iso_sha256: iso::sha256_file(&isop.path).unwrap_or_default(),
        account: req.account.clone(),
        spacesd: unattend::SPACESD_VERSION.into(),
        built_at: now(),
        disk_bytes: std::fs::metadata(&disk)?.len(),
    };
    std::fs::write(
        layout.manifest(v, arch),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    // The scratch (data disc, install copies) is not needed again.
    let _ = std::fs::remove_dir_all(&work);
    progress("Windows is ready", 1.0);
    Ok(disk)
}

/// Refuses when the volume holding the images cannot take a build.
pub fn check_disk(
    layout: &Layout,
    v: &WindowsVersion,
    probe: &dyn cua_vmm::disk::SpaceProbe,
) -> Result<()> {
    let cfg = cua_vmm::disk::CacheConfig::load();
    cua_vmm::disk::ensure_space_in(
        probe,
        &layout.root_or_parent(),
        BUILD_SCRATCH_BYTES + u64::from(v.disk_gb) * (1 << 30) / 4,
        &format!("build {}", v.name),
        cfg.min_free,
        cfg.warn_free,
    )
    .map_err(cua_vmm::VmmError::from)?;
    Ok(())
}

impl Layout {
    /// The nearest directory that exists, for the free-space check.
    fn root_or_parent(&self) -> PathBuf {
        let mut p = self.root.clone();
        while !p.exists() {
            match p.parent() {
                Some(q) => p = q.to_path_buf(),
                None => break,
            }
        }
        p
    }
}

/// The version a reference names when it is a Windows local build
/// (`ghcr.io/trycua/windows:11`, with or without a digest), else `None`.
pub fn local_build_target(reference: &str) -> Option<&'static WindowsVersion> {
    let r = reference.split('@').next().unwrap_or(reference);
    let tag = r.strip_prefix(crate::canonical::WINDOWS_REPO)?.strip_prefix(':')?;
    super::table::lookup(tag).filter(|v| v.builds_locally())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct NoNet;
    #[async_trait]
    impl IsoFetcher for NoNet {
        async fn eval_page(&self, _: &str) -> Result<String> {
            Err(ImageError::Registry("no network in tests".into()))
        }
        async fn final_url(&self, l: &str) -> Result<String> {
            Ok(l.into())
        }
        async fn download(&self, _: &str, to: &Path, _: &mut (dyn FnMut(u64) + Send)) -> Result<()> {
            std::fs::write(to, b"virtio")?;
            Ok(())
        }
    }

    #[derive(Default)]
    struct Fake {
        plans: Mutex<Vec<InstallPlan>>,
        size: u64,
    }
    #[async_trait]
    impl Installer for Fake {
        async fn install(&self, plan: &InstallPlan) -> Result<PathBuf> {
            self.plans.lock().unwrap().push(plan.clone());
            let disk = plan.work_dir.join("installed.qcow2");
            std::fs::File::create(&disk)?.set_len(self.size)?;
            Ok(disk)
        }
    }

    /// Plenty of room, whatever the machine running the tests has.
    struct Roomy;
    impl cua_vmm::disk::SpaceProbe for Roomy {
        fn space(&self, _: &Path) -> std::io::Result<cua_vmm::disk::Space> {
            Ok(cua_vmm::disk::Space {
                available: 1 << 40,
                total: 1 << 41,
            })
        }
    }

    fn iso_file(dir: &Path, name: &str) -> PathBuf {
        use std::io::{Seek as _, SeekFrom, Write as _};
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.seek(SeekFrom::Start(0x8001)).unwrap();
        f.write_all(b"CD001").unwrap();
        f.set_len(iso::MIN_ISO_BYTES + 1).unwrap();
        p
    }

    fn request(v: &str, iso: Option<PathBuf>) -> BuildRequest {
        BuildRequest {
            version: super::super::table::lookup(v).unwrap(),
            arch: WindowsArch::Amd64,
            user_iso: iso,
            iso_sha256: None,
            account: "User".into(),
            edition: None,
            cpus: 2,
            memory_mb: 4096,
        }
    }

    #[tokio::test]
    async fn the_first_create_builds_and_the_second_finds_it() {
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path().join("windows"));
        let iso = iso_file(d.path(), "Win11.iso");
        let fake = Fake {
            size: MIN_BASE_BYTES + 1,
            ..Default::default()
        };
        let r = request("11", Some(iso.clone()));
        let disk = ensure_base_with(&layout, &r, &NoNet, &fake, &Roomy).await.unwrap();
        assert_eq!(disk, layout.disk(r.version, r.arch));
        assert!(cached_base(&layout, r.version, r.arch).is_some());
        {
            let plans = fake.plans.lock().unwrap();
            assert_eq!(plans.len(), 1);
            assert_eq!(plans[0].windows_iso, iso);
            assert_eq!(plans[0].disk_gb, 64);
            assert!(plans[0].virtio_iso.ends_with("virtio-win.iso"));
        }
        // The second create does not install again.
        ensure_base_with(&layout, &r, &NoNet, &fake, &Roomy).await.unwrap();
        assert_eq!(fake.plans.lock().unwrap().len(), 1);
        // The scratch is gone, the lock released.
        assert!(!layout.dir(r.version, r.arch).join("work").exists());
        assert!(!layout.dir(r.version, r.arch).join("build.lock").exists());
    }

    #[tokio::test]
    async fn an_install_that_did_not_finish_leaves_no_base() {
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path().join("windows"));
        let iso = iso_file(d.path(), "w.iso");
        let fake = Fake {
            size: 1 << 20,
            ..Default::default()
        };
        let r = request("2025", Some(iso));
        let e = ensure_base_with(&layout, &r, &NoNet, &fake, &Roomy).await.unwrap_err();
        assert!(e.to_string().contains("did not finish"), "{e}");
        assert!(cached_base(&layout, r.version, r.arch).is_none());
        assert!(!layout.dir(r.version, r.arch).join("build.lock").exists());
    }

    #[tokio::test]
    async fn a_running_build_blocks_a_second_one_but_a_stale_lock_does_not() {
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path().join("windows"));
        let r = request("11", None);
        let dir = layout.dir(r.version, r.arch);
        let held = Lock::take(&dir).unwrap();
        let e = Lock::take(&dir).err().expect("second lock").to_string();
        assert!(e.contains("another build"), "{e}");
        drop(held);
        assert!(Lock::take(&dir).is_ok());
    }

    #[test]
    fn a_full_disk_refuses_before_any_download() {
        struct Full;
        impl cua_vmm::disk::SpaceProbe for Full {
            fn space(&self, _: &Path) -> std::io::Result<cua_vmm::disk::Space> {
                Ok(cua_vmm::disk::Space {
                    available: 1 << 30,
                    total: 100 << 30,
                })
            }
        }
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path().join("windows"));
        let v = super::super::table::lookup("11").unwrap();
        let e = check_disk(&layout, v, &Full).unwrap_err().to_string();
        assert!(e.contains("build Windows 11"), "{e}");
    }

    #[test]
    fn references_name_their_version() {
        for (r, id) in [
            ("ghcr.io/trycua/windows:11", Some("11")),
            ("ghcr.io/trycua/windows:2025@sha256:abc", Some("2025")),
            ("ghcr.io/trycua/windows:2022", None),
            ("ghcr.io/trycua/windows:2022-disk", None),
            ("ghcr.io/trycua/linux:11", None),
        ] {
            assert_eq!(local_build_target(r).map(|v| v.id), id, "{r}");
        }
    }

    #[test]
    fn the_request_reads_the_environment_and_checks_the_account() {
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path());
        let v = super::super::table::lookup("11").unwrap();
        let env = |k: &str| match k {
            "CUA_WINDOWS_USER" => Some("Docker".to_string()),
            "CUA_WINDOWS_ISO" => Some("/isos/w.iso".to_string()),
            _ => None,
        };
        let r = BuildRequest::from_env(v, WindowsArch::Amd64, &layout, &env).unwrap();
        assert_eq!(r.account, "Docker");
        assert_eq!(r.user_iso.as_deref(), Some(Path::new("/isos/w.iso")));
        let bad = |k: &str| (k == "CUA_WINDOWS_USER").then(|| "a b".to_string());
        assert!(BuildRequest::from_env(v, WindowsArch::Amd64, &layout, &bad).is_err());
    }
}
