//! The performance core of `cua ci`: everything that decides how long a
//! job waits for a computer and how long it runs once it has one.
//!
//! - [`warm`]: the warm pool. Idle, fully prepared VMs per slot (macOS
//!   first, within Apple's two-VM limit), a refill policy with a cap, an
//!   idle TTL, scale to zero and a spend budget. A claim registers a warm
//!   VM as a runner with a just-in-time configuration in about a second.
//! - [`golden`]: the saved-memory warm start. One golden VM is booted
//!   once, checkpointed with its tools and caches warm, and every job VM
//!   is a fork of that checkpoint. The golden image is keyed by what is in
//!   it and rebuilt when any of that changes.
//! - [`boot`]: what runs in the guest around a job: the job hooks that
//!   time and prepare it, the services a CI VM does not need, the fixups
//!   after a fork.
//! - [`cachevol`]: read-only dependency caches (pnpm, cargo, SwiftPM,
//!   sccache) and the rule that only default-branch jobs ever write them.
//! - [`mirror`]: a read-only mirror of the job's repository so
//!   `actions/checkout` fetches only what is new.
//! - [`registry`]: Docker Hub pull-through mirrors for Linux guests, so a
//!   job's pulls neither hit the hub's per-address limit nor wait on it.
//! - [`timers`]: the per-phase timeline of a job and the structured
//!   events the bench reads (`ci-contract/EVENTS.md`).
//!
//! Every module is pure policy over small seams (the clock, Spaces, the
//! filesystem), so the pool logic is tested without a VM.

pub mod boot;
pub mod cachevol;
pub mod golden;
pub mod job_runner;
pub mod mirror;
pub mod registry;
pub mod runtime;
pub mod timers;
pub mod warm;

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// When a warm VM becomes a runner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterMode {
    /// At claim time: a warm VM is only booted and prepared. The runner is
    /// registered with a one-job JIT configuration when a job is taken
    /// (the hosted service mints it), so no runner idles at GitHub.
    #[default]
    Claim,
    /// Ahead of time: idle runners wait registered and GitHub assigns them
    /// jobs itself (no service round trip, today's behaviour).
    Eager,
}

/// How the pool's VMs start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    /// Taken from the warm pool.
    #[default]
    WarmPool,
    /// Forked from the golden checkpoint.
    Fork,
    /// Created from the image and booted.
    ColdCreate,
}

impl StartMode {
    /// The word `EVENTS.md` uses.
    pub fn as_str(self) -> &'static str {
        match self {
            StartMode::WarmPool => "warm_pool",
            StartMode::Fork => "fork",
            StartMode::ColdCreate => "cold_create",
        }
    }
}

fn yes() -> bool {
    true
}

fn d_warm_ttl() -> String {
    "30m".into()
}

fn d_zero_after() -> String {
    "15m".into()
}

fn d_golden_age() -> String {
    "7d".into()
}

fn d_cache_gb() -> u32 {
    20
}

fn d_cache_stores() -> Vec<String> {
    ["pnpm", "cargo", "swiftpm", "sccache"]
        .into_iter()
        .map(String::from)
        .collect()
}

/// The `[perf]` table of a pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerfConfig {
    /// Turn the performance core on: warm VMs wait unregistered and a
    /// claim registers them. Off: the pool behaves as it always did.
    #[serde(default)]
    pub enabled: bool,
    /// When a warm VM is registered.
    #[serde(default)]
    pub register: RegisterMode,
    /// A warm VM idle longer than this is replaced (default 30m), so
    /// idle minutes on a metered computer stay bounded.
    #[serde(default = "d_warm_ttl")]
    pub warm_ttl: String,
    /// With no job for this long the pool scales to zero warm VMs
    /// (default 15m); the next job starts cold or from the fork.
    #[serde(default = "d_zero_after")]
    pub scale_to_zero_after: String,
    /// Start job VMs as forks of the golden checkpoint.
    #[serde(default = "yes")]
    pub fork: bool,
    /// The golden checkpoint is rebuilt when older than this (default 7d).
    #[serde(default = "d_golden_age")]
    pub golden_max_age: String,
    /// Read-only dependency caches.
    #[serde(default)]
    pub cache: CacheConfig,
    /// The read-only repository mirror for `actions/checkout`.
    #[serde(default)]
    pub mirror: MirrorConfig,
    /// Docker registry mirrors for Linux guests.
    #[serde(default)]
    pub registry: registry::RegistryConfig,
    /// A spend budget for the pool's VMs (metered computers only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetConfig>,
    /// Turn off the services a CI VM does not need (Spotlight, photo
    /// analysis, update checks, sleep) at boot.
    #[serde(default = "yes")]
    pub trim_guest: bool,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            register: RegisterMode::Claim,
            warm_ttl: d_warm_ttl(),
            scale_to_zero_after: d_zero_after(),
            fork: true,
            golden_max_age: d_golden_age(),
            cache: CacheConfig::default(),
            mirror: MirrorConfig::default(),
            registry: registry::RegistryConfig::default(),
            budget: None,
            trim_guest: true,
        }
    }
}

/// The `[perf.cache]` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Mount caches into job VMs.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Which stores (`pnpm`, `cargo`, `swiftpm`, `sccache`).
    #[serde(default = "d_cache_stores")]
    pub stores: Vec<String>,
    /// The most disk all caches may take, GiB (oldest evicted first).
    #[serde(default = "d_cache_gb")]
    pub size_cap_gb: u32,
    /// Let a fork pull request read the repository's default-branch cache
    /// (public repositories only; never for a private one).
    #[serde(default = "yes")]
    pub fork_read_public: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stores: d_cache_stores(),
            size_cap_gb: d_cache_gb(),
            fork_read_public: true,
        }
    }
}

/// The `[perf.mirror]` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorConfig {
    /// Use a mirror for `actions/checkout`.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Mirror private repositories too (only with a per-repository
    /// credential the host holds; it never enters the VM).
    #[serde(default)]
    pub private: bool,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            private: false,
        }
    }
}

/// The `[perf.budget]` table: what idle and busy VMs may cost a month.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConfig {
    /// The monthly limit, USD.
    pub monthly_usd: f64,
    /// What one VM costs an hour, USD (`0` for a computer you own).
    pub usd_per_hour: f64,
}

impl Eq for BudgetConfig {}

impl PerfConfig {
    /// `<pool dir>/perf.toml`: the performance settings of a pool. They
    /// live beside the pool's configuration, not in it, so a pool file
    /// written before them keeps loading.
    pub fn path(paths: &crate::paths::CiPaths, pool: &str) -> std::path::PathBuf {
        paths.pool_dir(pool).join("perf.toml")
    }

    /// Reads and validates `path`; `None` when there is no file.
    pub fn load(path: &std::path::Path) -> Result<Option<Self>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::Config(format!("{}: {e}", path.display()))),
        };
        let cfg: PerfConfig =
            toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        cfg.validate()
            .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
        Ok(Some(cfg))
    }

    /// Checks every field.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Config(m));
        for (f, v, lo, hi) in [
            ("perf.warm_ttl", &self.warm_ttl, 60, 24 * 3600),
            (
                "perf.scale_to_zero_after",
                &self.scale_to_zero_after,
                60,
                7 * 86_400,
            ),
            (
                "perf.golden_max_age",
                &self.golden_max_age,
                3600,
                90 * 86_400,
            ),
        ] {
            let d = humantime::parse_duration(v.trim())
                .map_err(|e| Error::Config(format!("{f} {v:?}: {e} (for example 30m, 7d)")))?;
            if d.as_secs() < lo || d.as_secs() > hi {
                return bad(format!(
                    "{f} {v:?}: between {} and {}",
                    humantime::format_duration(Duration::from_secs(lo)),
                    humantime::format_duration(Duration::from_secs(hi))
                ));
            }
        }
        for s in &self.cache.stores {
            if !cachevol::STORES.iter().any(|k| k.name == s.as_str()) {
                return bad(format!(
                    "perf.cache.stores {s:?}: one of pnpm, cargo, swiftpm, sccache"
                ));
            }
        }
        self.registry.validate()?;
        if self.cache.size_cap_gb == 0 || self.cache.size_cap_gb > 2048 {
            return bad("perf.cache.size_cap_gb: 1 to 2048".into());
        }
        if let Some(b) = &self.budget
            && !(b.monthly_usd > 0.0
                && b.monthly_usd.is_finite()
                && b.usd_per_hour >= 0.0
                && b.usd_per_hour.is_finite())
        {
            return bad("perf.budget: monthly_usd above 0 and usd_per_hour 0 or more".into());
        }
        Ok(())
    }

    /// `warm_ttl`.
    pub fn warm_ttl(&self) -> Duration {
        humantime::parse_duration(self.warm_ttl.trim()).unwrap_or(Duration::from_secs(1800))
    }

    /// `scale_to_zero_after`.
    pub fn scale_to_zero_after(&self) -> Duration {
        humantime::parse_duration(self.scale_to_zero_after.trim())
            .unwrap_or(Duration::from_secs(900))
    }

    /// `golden_max_age`.
    pub fn golden_max_age(&self) -> Duration {
        humantime::parse_duration(self.golden_max_age.trim())
            .unwrap_or(Duration::from_secs(7 * 86_400))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_valid() {
        let c = PerfConfig::default();
        assert!(!c.enabled);
        c.validate().unwrap();
        assert_eq!(c.warm_ttl(), Duration::from_secs(1800));
        assert_eq!(c.scale_to_zero_after(), Duration::from_secs(900));
        assert_eq!(c.golden_max_age(), Duration::from_secs(7 * 86_400));
    }

    #[test]
    fn bad_values_say_which_field() {
        let mut c = PerfConfig::default();
        c.warm_ttl = "5s".into();
        assert!(
            c.validate()
                .unwrap_err()
                .to_string()
                .contains("perf.warm_ttl")
        );
        let mut c = PerfConfig::default();
        c.cache.stores.push("npm".into());
        assert!(c.validate().unwrap_err().to_string().contains("npm"));
        let mut c = PerfConfig::default();
        c.budget = Some(BudgetConfig {
            monthly_usd: 0.0,
            usd_per_hour: 1.0,
        });
        assert!(c.validate().is_err());
    }

    #[test]
    fn the_file_is_beside_the_pool_and_optional() {
        let d = tempfile::tempdir().unwrap();
        let paths = crate::paths::CiPaths::new(d.path());
        let p = PerfConfig::path(&paths, "acme");
        assert_eq!(p, d.path().join("acme/perf.toml"));
        assert!(PerfConfig::load(&p).unwrap().is_none());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "enabled = true\nwarm_ttl = \"10m\"\n").unwrap();
        let c = PerfConfig::load(&p).unwrap().unwrap();
        assert!(c.enabled);
        assert_eq!(c.warm_ttl(), Duration::from_secs(600));
        std::fs::write(&p, "warm_ttl = \"1s\"\n").unwrap();
        let e = PerfConfig::load(&p).unwrap_err().to_string();
        assert!(e.contains("perf.toml") && e.contains("warm_ttl"), "{e}");
        std::fs::write(&p, "nonsense = 1\n").unwrap();
        assert!(PerfConfig::load(&p).is_err());
        // The pool list does not mistake it for a pool.
        std::fs::write(paths.config("acme"), "").unwrap();
        assert_eq!(paths.pools(), vec!["acme".to_string()]);
    }

    #[test]
    fn toml_round_trip_with_defaults() {
        let c: PerfConfig = toml::from_str("enabled = true\n[cache]\nsize_cap_gb = 5\n").unwrap();
        assert!(c.enabled && c.fork && c.trim_guest);
        assert_eq!(c.cache.size_cap_gb, 5);
        assert_eq!(c.register, RegisterMode::Claim);
        assert!(toml::from_str::<PerfConfig>("bogus = 1").is_err());
    }
}
