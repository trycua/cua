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
/// The wall for an install the person accepted to run in software emulation.
pub const EMULATED_INSTALL_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);
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

/// The part of a first-time build's progress (0 to 1) that is downloading (the
/// ISO, then the drivers): the app tells "Downloading Windows" from
/// "Installing Windows" by this fraction (`cua-spaces-app-core`
/// `WINDOWS_INSTALL_FROM`, which a test there holds to this value).
pub const DOWNLOADS_END: f64 = 0.45;

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
            memory_mb: if arch == WindowsArch::Arm64 {
                8192
            } else {
                6144
            },
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

/// A first-time build reports under the download phase of the create (the
/// image is "pulled" once; the Space's own phases, the clone and the boot,
/// come after it and start the readiness clock).
fn progress(detail: impl Into<String>, fraction: f64) {
    use cua_vmm::progress::{Phase, Progress, report};
    let detail: String = detail.into();
    tracing::info!(%detail, fraction, "windows build progress");
    report(
        Progress::phase(Phase::Pulling)
            .detail(detail)
            .fraction(fraction),
    );
}

/// Reports how far the install is: the guest writes about 12 GiB of the disk
/// (sparse) before it is done, so the file's size is the best clock there is.
pub fn report_install_progress(disk: &Path, _disk_gb: u32) {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Windows spends minutes at a time not writing (the first logon, the
    // driver and service setup): the fraction still creeps up with every
    // report, so the app's stall watch sees an install that is alive.
    static REPORTS: AtomicU64 = AtomicU64::new(0);
    let n = REPORTS.fetch_add(1, Ordering::Relaxed) as f64;
    let used = std::fs::metadata(disk).map(|m| m.len()).unwrap_or(0);
    let frac = (used as f64 / (14u64 << 30) as f64).clamp(0.0, 1.0);
    progress(
        "Installing Windows (about 20 to 40 minutes, once)",
        (0.47 + 0.48 * frac + n * 1e-5).min(0.96),
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

/// What the answer file installs from `isop`: whether it is evaluation media
/// (it carries its own key) and the edition to pick by name.
///
/// Downloaded media is known from the table. An ISO of the person's own is not,
/// and Setup stops at the "Product key" page when a retail ISO gets no key, so a
/// client ISO whose file name does not say "eval" is taken to be retail
/// multi-edition media: Pro, with the generic key (it activates nothing).
/// Server ISOs name their edition from the table either way.
fn install_edition(
    v: &WindowsVersion,
    arch: WindowsArch,
    isop: &iso::ResolvedIso,
    asked: Option<&str>,
) -> (bool, Option<String>) {
    use super::table::Media;
    let pro = || {
        let family = if v.name.starts_with("Windows 10") {
            "Windows 10"
        } else {
            "Windows 11"
        };
        format!("{family} Pro")
    };
    let (evaluation, retail) = if isop.origin == iso::IsoOrigin::User {
        let eval = isop
            .path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("eval"));
        (eval, !eval && !v.server)
    } else {
        let media = v.media(arch);
        (
            media == Media::Evaluation,
            matches!(media, Media::Consumer(_)),
        )
    };
    let edition = asked.map(str::to_string).or_else(|| retail.then(pro));
    (evaluation, edition)
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
    let isop = iso::resolve_iso(
        &IsoRequest {
            version: v,
            arch,
            user_iso: req.user_iso.as_deref(),
            cache_dir: &layout.iso_dir(),
            sha256: req.iso_sha256.as_deref(),
            scan_dirs: iso::default_scan_dirs(&layout.iso_dir()),
        },
        fetcher,
        &mut |_| {},
    )
    .await?;
    let (evaluation_media, edition) = install_edition(v, arch, &isop, req.edition.as_deref());
    progress("Preparing the unattended install", 0.46);
    let work = dir.join("work");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(layout.iso_dir())?;
    let virtio = layout.iso_dir().join("virtio-win.iso");
    if !virtio.is_file() {
        iso::download_reporting(
            fetcher,
            VIRTIO_URL,
            &virtio,
            "Downloading the Windows drivers for the virtual computer",
            (0.40, 0.45),
            &mut |_| {},
        )
        .await?;
    }
    // The newest cua-spacesd with a Windows build (the pinned one when GitHub
    // cannot be read).
    let spacesd = fetcher
        .get_text(
            "https://api.github.com/repos/trycua/cua/releases?per_page=40",
            None,
        )
        .await
        .ok()
        .and_then(|j| unattend::latest_spacesd(&j))
        .unwrap_or_else(|| unattend::SPACESD_VERSION.to_string());
    let data_iso = work.join("unattend.iso");
    let disc = unattend::data_disc(&UnattendOptions {
        version: v,
        arch,
        account: &req.account,
        edition: edition.as_deref(),
        evaluation_media,
        virtio: true,
        spacesd_version: &spacesd,
    });
    std::fs::write(&data_iso, disc)?;

    progress(
        format!("Installing {} (about 20 to 40 minutes, once)", v.name),
        0.47,
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
        timeout: if arch == WindowsArch::host() {
            INSTALL_TIMEOUT
        } else {
            EMULATED_INSTALL_TIMEOUT
        },
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
    progress("Sealing the image", 0.97);
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
        spacesd: spacesd.clone(),
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
    let tag = r
        .strip_prefix(crate::canonical::WINDOWS_REPO)?
        .strip_prefix(':')?;
    super::table::lookup(tag).filter(|v| v.builds_locally())
}

/// Remembers a Windows ISO the person chose for the image `reference` names
/// (what `cua sb create windows:11 --iso <path>` does), in `layout`: the
/// build that follows installs from it instead of downloading one. Answers
/// the file's name, or why it cannot be used, in plain words.
pub fn use_user_iso(
    layout: &Layout,
    reference: &str,
    path: &Path,
) -> std::result::Result<String, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let version = local_build_target(reference)
        .ok_or_else(|| "This image does not install from a Windows ISO.".to_string())?;
    let arch = WindowsArch::host();
    version.check_arch(arch).map_err(|e| e.to_string())?;
    iso::register(&layout.iso_dir(), version, arch, path).map_err(|e| iso_problem(&name, &e))?;
    Ok(name)
}

/// Why an ISO the person chose cannot be used, in plain words.
fn iso_problem(name: &str, e: &ImageError) -> String {
    let why = match e {
        ImageError::Reference(_, why) => why.as_str(),
        _ => return format!("{name} can't be used: {e}"),
    };
    if why.starts_with("no such file") {
        format!("{name} is gone. Choose the Windows ISO again.")
    } else if why.contains("too small") || why.contains("not an ISO 9660") {
        format!(
            "{name} is not a Windows disc image (.iso). Choose the file you downloaded from Microsoft."
        )
    } else if why.contains("ARM64 ISO") {
        format!("{name} is for ARM64 PCs, but this computer needs the x64 Windows ISO.")
    } else if why.contains("x64 ISO") {
        format!("{name} is for x64 PCs, but this computer needs the ARM64 Windows ISO.")
    } else {
        format!("{name} can't be used: {why}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(path: &str, origin: iso::IsoOrigin) -> iso::ResolvedIso {
        iso::ResolvedIso {
            path: path.into(),
            origin,
        }
    }

    fn answer_file(v: &WindowsVersion, arch: WindowsArch, r: &iso::ResolvedIso) -> String {
        let (evaluation_media, edition) = install_edition(v, arch, r, None);
        unattend::autounattend_xml(&UnattendOptions {
            version: v,
            arch,
            account: "User",
            edition: edition.as_deref(),
            evaluation_media,
            virtio: true,
            spacesd_version: "0.0.0",
        })
    }

    /// Setup stalls at "Product key" when a retail ISO of the person's own gets
    /// no key: it installs Pro with the generic key; evaluation media and Server
    /// keep their own.
    #[test]
    fn a_user_iso_installs_pro_with_the_generic_key_unless_it_is_evaluation_media() {
        let w11 = super::super::table::lookup("11").unwrap();
        let retail = resolved(
            "/Downloads/Windows11_Client_arm64_en-us_26300_9457.iso",
            iso::IsoOrigin::User,
        );
        let x = answer_file(w11, WindowsArch::Arm64, &retail);
        assert!(x.contains("<Key>/IMAGE/NAME</Key><Value>Windows 11 Pro</Value>"));
        assert!(x.contains("<ProductKey><Key>VK7JG-NPHTM-C97JM-9MPGT-3V66T</Key>"));
        let eval = resolved(
            "/x/26100.1.240331-1435.ge_release_CLIENTENTERPRISEEVAL_x64.iso",
            iso::IsoOrigin::User,
        );
        let x = answer_file(w11, WindowsArch::Amd64, &eval);
        assert!(!x.contains("ProductKey") && x.contains("/IMAGE/INDEX"));
        let server = super::super::table::lookup("2022").unwrap();
        let x = answer_file(server, WindowsArch::Amd64, &retail);
        assert!(!x.contains("ProductKey"));
        // Downloaded consumer media is unchanged.
        let dl = resolved("/cache/w11.iso", iso::IsoOrigin::Downloaded);
        let x = answer_file(w11, WindowsArch::Arm64, &dl);
        assert!(x.contains("Windows 11 Pro") && x.contains("VK7JG"));
    }
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
        async fn download(
            &self,
            _: &str,
            to: &Path,
            _: &mut (dyn FnMut(u64) + Send),
        ) -> Result<()> {
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
        let disk = ensure_base_with(&layout, &r, &NoNet, &fake, &Roomy)
            .await
            .unwrap();
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
        ensure_base_with(&layout, &r, &NoNet, &fake, &Roomy)
            .await
            .unwrap();
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
        let e = ensure_base_with(&layout, &r, &NoNet, &fake, &Roomy)
            .await
            .unwrap_err();
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
            ("ghcr.io/trycua/linux:24.04", None),
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

    /// A chosen ISO is remembered for the image (the build then finds it), and
    /// one that is not a disc image is refused in plain words.
    #[test]
    fn a_chosen_iso_is_remembered_and_a_wrong_file_is_refused_plainly() {
        use std::io::{Seek as _, SeekFrom, Write as _};
        let d = tempfile::tempdir().unwrap();
        let layout = Layout::new(d.path().join("windows"));
        let good = d.path().join("Windows11.iso");
        let mut f = std::fs::File::create(&good).unwrap();
        f.set_len(iso::MIN_ISO_BYTES).unwrap();
        f.seek(SeekFrom::Start(0x8001)).unwrap();
        f.write_all(b"CD001").unwrap();
        drop(f);
        let name = use_user_iso(&layout, "ghcr.io/trycua/windows:11", &good).unwrap();
        assert_eq!(name, "Windows11.iso");
        let v = local_build_target("ghcr.io/trycua/windows:11").unwrap();
        assert_eq!(
            iso::registered(&layout.iso_dir(), v, WindowsArch::host()),
            Some(std::fs::canonicalize(&good).unwrap())
        );
        let text = d.path().join("notes.txt");
        std::fs::write(&text, b"hello").unwrap();
        let e = use_user_iso(&layout, "ghcr.io/trycua/windows:11", &text).unwrap_err();
        assert!(e.contains("notes.txt is not a Windows disc image"), "{e}");
        assert!(!e.contains(d.path().to_str().unwrap()), "no path in {e}");
        let e = use_user_iso(&layout, "ghcr.io/trycua/linux:24.04", &good).unwrap_err();
        assert_eq!(e, "This image does not install from a Windows ISO.");
    }
}
