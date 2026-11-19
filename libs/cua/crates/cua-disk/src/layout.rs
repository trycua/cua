//! Every location the SDK writes.
//!
//! Everything lives under one root, the cua home (`$CUA_HOME`, else
//! `~/.cua`), so one setting moves it (for example to a larger volume) and
//! one command accounts for it. The exceptions are stores another program
//! owns or a platform convention places:
//!
//! | location | why it is elsewhere |
//! |---|---|
//! | Lume's VM store (`~/.lume`, `lume config`) | Lume owns its VMs; the SDK records the ones it creates under `vmm/lume/owned` |
//! | the container engine (Colima, Docker Desktop, a Linux data root) | the engine owns images and containers; the SDK labels its own |
//! | cua-bench runs (`$XDG_DATA_HOME/cua-bench/runs`) | results are user data, placed where the XDG spec says |
//! | cua-spacesd logs (`~/Library/Logs/cua-spacesd`, `$XDG_STATE_HOME/cua-spacesd/logs`) | OS log conventions |

use std::path::{Path, PathBuf};

/// The cua home and the paths under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    home: PathBuf,
    /// Include the locations outside the home (bench runs, spacesd logs).
    external: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self::system(cua_vmm::host::cua_home())
    }
}

impl Layout {
    /// A layout confined to `home`: the locations outside it (bench runs,
    /// spacesd logs) are left out. Tests use it.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            external: false,
        }
    }

    /// The host's layout with the cua home at `home`.
    pub fn system(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            external: true,
        }
    }

    /// The cua home.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Image cache (`images/{blobs,disks,rootfs,refs}`).
    pub fn images(&self) -> PathBuf {
        self.home.join("images")
    }

    /// QEMU instances, bases, checkpoints and frozen layers.
    pub fn qemu(&self) -> PathBuf {
        self.home.join("vmm").join("qemu")
    }

    /// Lume Linux disk conversions, seeds and `serve.log`.
    pub fn lume(&self) -> PathBuf {
        self.home.join("vmm").join("lume")
    }

    /// Records of the Lume VMs the SDK created.
    pub fn lume_owned(&self) -> PathBuf {
        self.lume().join("owned")
    }

    /// Ledger of the container images the SDK pulled or built.
    pub fn docker_pulls(&self) -> PathBuf {
        self.home.join("docker").join("pulls")
    }

    /// Local build outputs (`build/<tag>`, `build/out-<id>`) and the build
    /// SSH key.
    pub fn build(&self) -> PathBuf {
        self.home.join("build")
    }

    /// The host install cache teleported apps use
    /// (`cache/installables/<sha256>.<ext>`, written by cua-spaces).
    pub fn installables(&self) -> PathBuf {
        self.home.join("cache").join("installables")
    }

    /// The app icon cache every UI shares (`cache/icons/<key>.png`, written
    /// by cua-icon-cache).
    pub fn icons(&self) -> PathBuf {
        self.home.join("cache").join("icons")
    }

    /// Sandbox state files (and `.ephemeral/` leases).
    pub fn sandboxes(&self) -> PathBuf {
        self.home.join("sandboxes")
    }

    /// cua logs (`daemon.log`).
    pub fn logs(&self) -> PathBuf {
        self.home.join("logs")
    }

    /// The daemon log.
    pub fn daemon_log(&self) -> PathBuf {
        self.logs().join("daemon.log")
    }

    /// Host service state (`cua host`), including `driver.log`.
    pub fn host(&self) -> PathBuf {
        self.home.join("host")
    }

    /// `cua do` trajectories.
    pub fn trajectories(&self) -> PathBuf {
        self.home.join("trajectories")
    }

    /// Screenshots `cua do` saves without `--save` (newest few kept).
    pub fn screenshots(&self) -> PathBuf {
        self.home.join("screenshots")
    }

    /// Recorded skills.
    pub fn skills(&self) -> PathBuf {
        self.home.join("skills")
    }

    /// The cua-bench dataset registry cache.
    pub fn bench_registry(&self) -> PathBuf {
        std::env::var_os("CUA_BENCH_REGISTRY_CACHE")
            .filter(|v| self.external && !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join("cbregistry"))
    }

    /// The cache configuration (`cache.json`).
    pub fn config(&self) -> PathBuf {
        self.home.join(cua_vmm::disk::CONFIG_FILE)
    }

    /// The garbage-collection lock.
    pub fn gc_lock(&self) -> PathBuf {
        self.home.join("gc.lock")
    }

    /// Stamp of the last automatic garbage collection.
    pub fn gc_stamp(&self) -> PathBuf {
        self.home.join("gc.stamp")
    }

    /// Caches of the Python adapters (Hyper-V, Android, Tart, URL images).
    pub fn legacy(&self) -> Vec<PathBuf> {
        vec![
            self.home.join("cua-sandbox"),
            self.home.join("android-sdk"),
            self.home.join("android-avd"),
        ]
    }

    /// cua-bench run results (outside the cua home, XDG data); `None` for a
    /// confined layout.
    pub fn bench_runs(&self) -> Option<PathBuf> {
        if !self.external {
            return None;
        }
        let p = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| cua_vmm::host::home_dir().join(".local").join("share"))
            .join("cua-bench")
            .join("runs");
        Some(p)
    }

    /// cua-spacesd and viewer logs on this host (outside the cua home; none
    /// for a confined layout).
    pub fn spacesd_logs(&self) -> Vec<PathBuf> {
        if !self.external {
            return vec![];
        }
        if let Some(d) = std::env::var_os("CUA_ENV_LOG_DIR").filter(|v| !v.is_empty()) {
            return vec![PathBuf::from(d)];
        }
        let home = cua_vmm::host::home_dir();
        if cfg!(target_os = "macos") {
            vec![
                home.join("Library/Logs/cua-spacesd"),
                // The LaunchAgent's stdout/stderr (packaging/macos plist).
                home.join("Library/Logs/cua-spacesd.log"),
            ]
        } else if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA")
                .map(|d| vec![PathBuf::from(d).join("cua-spacesd").join("Logs")])
                .unwrap_or_default()
        } else {
            std::env::var_os("XDG_STATE_HOME")
                .filter(|v| !v.is_empty())
                .map(|d| vec![PathBuf::from(d).join("cua-spacesd").join("logs")])
                .unwrap_or_else(|| vec![std::env::temp_dir().join("cua-spacesd").join("logs")])
        }
    }

    /// Top-level entries of the cua home the accounting names explicitly;
    /// anything else is reported as `other`.
    pub fn known_top_level() -> &'static [&'static str] {
        &[
            "images",
            "vmm",
            "docker",
            "build",
            "cache",
            "sandboxes",
            "logs",
            "host",
            "trajectories",
            "skills",
            "screenshots",
            "cbregistry",
            "cua-sandbox",
            "android-sdk",
            "android-avd",
        ]
    }
}
