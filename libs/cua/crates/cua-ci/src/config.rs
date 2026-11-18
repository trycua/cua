//! A pool's configuration: `$CUA_HOME/ci/<pool>.toml`.
//!
//! It names the GitHub scope, the image, where Spaces run, how many
//! runners, the timeouts and the egress policy. It holds no secret: the
//! `[github]` table says where the credential is kept
//! ([`crate::secret`]), never the credential itself.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Apple's macOS license allows two macOS VMs per Mac, and Apple
/// Virtualization (which Lume runs on) refuses a third.
pub const MACOS_VMS_PER_HOST: u32 = 2;

/// The label every runner of every pool carries, besides GitHub's
/// `self-hosted`, OS and architecture labels.
pub const POOL_LABEL: &str = "cua";

/// The most memory (MiB) a macOS job VM gets by default.
pub const DEFAULT_MACOS_MEMORY_MB: u64 = 8192;

/// The canonical macOS image (`ghcr.io/trycua/macos:26`).
pub const DEFAULT_MACOS_IMAGE: &str = "ghcr.io/trycua/macos:26";

/// The Space label that marks a pool's Spaces (`cua-ci=<pool>`).
pub const SPACE_LABEL_POOL: &str = "cua-ci";

/// The Space label that names the slot (`cua-ci-slot=<slot id>`).
pub const SPACE_LABEL_SLOT: &str = "cua-ci-slot";

/// Where runners register: an organization or one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `org:NAME`.
    Org(String),
    /// `repo:OWNER/NAME`.
    Repo {
        /// Owner (user or organization).
        owner: String,
        /// Repository name.
        name: String,
    },
}

impl Scope {
    /// `org:NAME` or `repo:OWNER/NAME`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let bad = || {
            Error::Config(format!(
                "scope {s:?}: use org:NAME or repo:OWNER/NAME (for example org:trycua)"
            ))
        };
        if let Some(org) = s.strip_prefix("org:") {
            if !valid_github_name(org) {
                return Err(bad());
            }
            return Ok(Scope::Org(org.to_string()));
        }
        if let Some(repo) = s.strip_prefix("repo:") {
            let (owner, name) = repo.split_once('/').ok_or_else(bad)?;
            if !valid_github_name(owner) || !valid_repo_name(name) {
                return Err(bad());
            }
            return Ok(Scope::Repo {
                owner: owner.to_string(),
                name: name.to_string(),
            });
        }
        Err(bad())
    }

    /// The REST path prefix: `orgs/NAME` or `repos/OWNER/NAME`.
    pub fn api_prefix(&self) -> String {
        match self {
            Scope::Org(o) => format!("orgs/{o}"),
            Scope::Repo { owner, name } => format!("repos/{owner}/{name}"),
        }
    }

    /// A pool name for this scope (`trycua`, `owner-name`).
    pub fn default_pool_name(&self) -> String {
        let raw = match self {
            Scope::Org(o) => o.clone(),
            Scope::Repo { owner, name } => format!("{owner}-{name}"),
        };
        let mut s: String = raw
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        s.truncate(32);
        s.trim_matches('-').to_string()
    }

    /// Whether this is an organization.
    pub fn is_org(&self) -> bool {
        matches!(self, Scope::Org(_))
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::Org(o) => write!(f, "org:{o}"),
            Scope::Repo { owner, name } => write!(f, "repo:{owner}/{name}"),
        }
    }
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Scope::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn valid_github_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 39
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !s.starts_with('-')
}

fn valid_repo_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The guest OS family a pool's runners run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuestOs {
    /// A macOS VM (Lume, on a Mac).
    Macos,
    /// A Linux container or VM.
    Linux,
}

impl GuestOs {
    /// GitHub's OS label (`macOS`, `Linux`).
    pub fn label(self) -> &'static str {
        match self {
            GuestOs::Macos => "macOS",
            GuestOs::Linux => "Linux",
        }
    }

    /// `macos` or `linux`.
    pub fn as_str(self) -> &'static str {
        match self {
            GuestOs::Macos => "macos",
            GuestOs::Linux => "linux",
        }
    }
}

/// GitHub's architecture label for a guest architecture (`ARM64`, `X64`).
pub fn arch_label(arch: &str) -> Option<&'static str> {
    match arch.trim().to_ascii_lowercase().as_str() {
        "arm64" | "aarch64" => Some("ARM64"),
        "amd64" | "x86_64" | "x64" => Some("X64"),
        _ => None,
    }
}

/// The labels a runner registers with: `self-hosted`, its OS and
/// architecture, [`POOL_LABEL`], then the pool's own (deduplicated, in
/// that order).
pub fn runner_labels(os: GuestOs, arch: &str, extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec!["self-hosted".into(), os.label().into()];
    if let Some(a) = arch_label(arch) {
        out.push(a.into());
    }
    out.push(POOL_LABEL.into());
    for l in extra {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(l)) {
            out.push(l.clone());
        }
    }
    out
}

/// What a pool does about its Spaces' outbound connections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EgressMode {
    /// Deny private, carrier-grade NAT (tailnet), link-local and
    /// unique-local destinations ([`crate::egress::DEFAULT_DENY`]), plus
    /// `egress_deny`, except `egress_allow` and the guest's own resolver.
    #[default]
    DenyPrivate,
    /// No policy.
    Off,
}

/// How the pool authenticates to GitHub.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// A GitHub App: its private key is the secret.
    #[default]
    App,
    /// A token (fine-grained or classic) with the same rights.
    Token,
}

/// The `[github]` table: the API and where the credential is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    /// The REST API root (GitHub Enterprise Server: `https://HOST/api/v3`).
    #[serde(default = "default_api_url")]
    pub api_url: String,
    /// `app` or `token`.
    #[serde(default)]
    pub auth: AuthKind,
    /// The GitHub App's id (`auth = "app"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<u64>,
    /// The App's installation id; looked up from the scope when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<u64>,
    /// The credential store's service name
    /// ([`crate::secret::APP_KEY_SERVICE`] or
    /// [`crate::secret::TOKEN_SERVICE`]).
    pub secret_service: String,
    /// The credential store's account (the pool name).
    pub secret_account: String,
    /// Where the credential is kept: `keychain` (the macOS login keychain)
    /// or `file` (a 0600 file under `$CUA_HOME/ci/secrets/`, for a Mac
    /// set up over SSH whose keychain is locked, and on Linux).
    #[serde(default)]
    pub secret_store: crate::secret::StoreKind,
    /// A GitHub App private key kept in a file of its own (owned by this
    /// user, mode 0600) instead of the store: for a headless host, or one
    /// set up over SSH, whose login keychain cannot be used. Read first,
    /// before the store; `CUA_CI_GITHUB_APP_KEY_FILE` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_file: Option<std::path::PathBuf>,
}

fn default_api_url() -> String {
    "https://api.github.com".into()
}

fn default_on() -> String {
    "local".into()
}

fn default_image() -> String {
    DEFAULT_MACOS_IMAGE.into()
}

fn default_max() -> u32 {
    MACOS_VMS_PER_HOST
}

fn default_job_timeout() -> String {
    "6h".into()
}

fn default_idle_timeout() -> String {
    "60m".into()
}

fn default_register_timeout() -> String {
    "10m".into()
}

fn default_retention() -> u32 {
    7
}

/// One pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolConfig {
    /// The pool's name (lowercase letters, digits and `-`, at most 32).
    pub name: String,
    /// `org:NAME` or `repo:OWNER/NAME`.
    pub scope: Scope,
    /// The organization runner group runners join (organizations only;
    /// default: the organization's `Default` group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_group: Option<String>,
    /// Labels besides `self-hosted`, the OS, the architecture and
    /// `cua`.
    #[serde(default)]
    pub labels: Vec<String>,
    /// The image each Space starts from (default `ghcr.io/trycua/macos:26`).
    #[serde(default = "default_image")]
    pub image: String,
    /// The guest OS; inferred from `image` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<GuestOs>,
    /// Where Spaces run: `local` (this computer), `auto` (any of your
    /// computers) or `host:<computer>`.
    #[serde(default = "default_on")]
    pub on: String,
    /// At most this many runners (Spaces) at once. macOS on one computer:
    /// at most two.
    #[serde(default = "default_max")]
    pub max_runners: u32,
    /// Idle registered runners to keep ready (default: `max_runners`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<u32>,
    /// vCPUs per Space (default for macOS on this computer: half its cores
    /// per runner slot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Memory per Space in MiB (default for macOS on this computer: its
    /// memory less 8 GiB split between the slots, at most 8 GiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    /// Grow each Space's disk to this many GiB (default: the image's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u32>,
    /// A job that runs longer is stopped: its Space is deleted (default 6h).
    #[serde(default = "default_job_timeout")]
    pub job_timeout: String,
    /// An idle runner older than this is replaced by a fresh one (default
    /// 60m), so images and registrations stay fresh.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: String,
    /// A runner that has not come online at GitHub this long after it was
    /// started is replaced (default 10m).
    #[serde(default = "default_register_timeout")]
    pub register_timeout: String,
    /// The egress policy of the runners' Spaces.
    #[serde(default)]
    pub egress: EgressMode,
    /// More destinations to deny (IPv4/IPv6 addresses or CIDRs).
    #[serde(default)]
    pub egress_deny: Vec<String>,
    /// Destinations to allow although denied (a cache or registry on your
    /// network).
    #[serde(default)]
    pub egress_allow: Vec<String>,
    /// Refuse to start runners where the egress policy cannot be enforced
    /// (macOS VMs today) instead of starting them without it.
    #[serde(default)]
    pub require_egress: bool,
    /// Runner and Space names start with this (default `cua-<pool>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_prefix: Option<String>,
    /// Copied runner logs are kept this many days.
    #[serde(default = "default_retention")]
    pub log_retention_days: u32,
    /// GitHub and the credential's location.
    pub github: GitHubConfig,
}

impl PoolConfig {
    /// Reads and validates `path`.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!(
                "{}: {e} (create the pool with `cua ci connect --own-app`)",
                path.display()
            ))
        })?;
        Self::parse(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
    }

    /// Parses and validates TOML.
    pub fn parse(text: &str) -> Result<Self> {
        let cfg: PoolConfig = toml::from_str(text).map_err(|e| Error::Config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// TOML, as `cua ci connect --own-app` writes it.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))
    }

    /// Writes the file (0600: it names where the credential is).
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.validate()?;
        cua_home::guard_write(path)?;
        cua_home::write_private(path, self.to_toml()?.as_bytes())?;
        Ok(())
    }

    /// Checks every field.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Config(m));
        if !valid_pool_name(&self.name) {
            return bad(format!(
                "pool name {:?}: use 1 to 32 lowercase letters, digits and -",
                self.name
            ));
        }
        if self.runner_group.is_some() && !self.scope.is_org() {
            return bad(
                "runner_group: repository runners have no runner group (only organizations do)"
                    .into(),
            );
        }
        for l in &self.labels {
            if l.trim().is_empty() || l.len() > 100 || l.contains(',') {
                return bad(format!(
                    "label {l:?}: labels are 1 to 100 characters without commas"
                ));
            }
        }
        if self.labels.len() > 90 {
            return bad("labels: at most 90 of your own".into());
        }
        if self.image.trim().is_empty() {
            return bad("image: name an image (for example ghcr.io/trycua/macos:26)".into());
        }
        if self.is_checkpoint() {
            if self.os.is_none() {
                return bad(format!(
                    "image {:?}: a checkpoint does not say its OS; set os = \"macos\" or \"linux\" \
                     (--os)",
                    self.image
                ));
            }
            if self.disk_gb.is_some() {
                return bad(
                    "disk_gb: a Space forked from a checkpoint keeps the checkpoint's disk".into(),
                );
            }
        }
        if self.os.is_none() && self.image.to_ascii_lowercase().contains("windows") {
            return bad(
                "image: Windows runners are not supported yet; use a macOS or Linux image".into(),
            );
        }
        let on = self.on.trim();
        if !(on == "local"
            || on == "auto"
            || on.strip_prefix("host:").is_some_and(|h| !h.is_empty()))
        {
            return bad(format!(
                "on {on:?}: use local, auto or host:<computer> (a cloud account is not a runner \
                 location)"
            ));
        }
        if self.max_runners == 0 || self.max_runners > 64 {
            return bad("max_runners: 1 to 64".into());
        }
        if let Some(w) = self.warm
            && w > self.max_runners
        {
            return bad(format!(
                "warm {w}: at most max_runners ({})",
                self.max_runners
            ));
        }
        for (field, v, max) in [
            (
                "job_timeout",
                &self.job_timeout,
                Duration::from_secs(5 * 24 * 3600),
            ),
            (
                "idle_timeout",
                &self.idle_timeout,
                Duration::from_secs(7 * 24 * 3600),
            ),
            (
                "register_timeout",
                &self.register_timeout,
                Duration::from_secs(24 * 3600),
            ),
        ] {
            let d = parse_duration(field, v)?;
            if d < Duration::from_secs(60) || d > max {
                return bad(format!(
                    "{field} {v:?}: between 1m and {}",
                    humantime::format_duration(max)
                ));
            }
        }
        let policy = crate::egress::EgressPolicy {
            allow: self.egress_allow.clone(),
            deny: self.egress_deny.clone(),
        };
        policy.check().map_err(Error::Config)?;
        if let Some(p) = &self.name_prefix
            && !valid_prefix(p)
        {
            return bad(format!(
                "name_prefix {p:?}: 1 to 40 letters, digits and -, starting with a letter"
            ));
        }
        if self.log_retention_days == 0 {
            return bad("log_retention_days: at least 1".into());
        }
        if self.github.auth == AuthKind::App && self.github.app_id.is_none() {
            return bad("github.app_id: a GitHub App pool needs the App's id".into());
        }
        if !(self.github.api_url.starts_with("https://")
            || self.github.api_url.starts_with("http://127.0.0.1")
            || self.github.api_url.starts_with("http://localhost"))
        {
            return bad("github.api_url: an https:// URL".into());
        }
        if self.github.secret_service.is_empty() || self.github.secret_account.is_empty() {
            return bad("github.secret_service and github.secret_account: both are needed".into());
        }
        Ok(())
    }

    /// The guest OS: `os`, else inferred from the image name.
    pub fn guest_os(&self) -> GuestOs {
        self.os.unwrap_or_else(|| {
            if self.image.to_ascii_lowercase().contains("macos") {
                GuestOs::Macos
            } else {
                GuestOs::Linux
            }
        })
    }

    /// Whether the image is a checkpoint (`checkpoint:<name>`, a golden
    /// image made with `cua sb checkpoint`): each Space is a fork of it.
    pub fn is_checkpoint(&self) -> bool {
        self.image.trim().starts_with("checkpoint:")
    }

    /// Whether Spaces run on one computer (`local` or `host:<id>`), so the
    /// two-macOS-VM limit caps the pool.
    pub fn single_host(&self) -> bool {
        self.on.trim() != "auto"
    }

    /// `max_runners`, capped at [`MACOS_VMS_PER_HOST`] for macOS on one
    /// computer.
    pub fn effective_max(&self) -> u32 {
        if self.guest_os() == GuestOs::Macos && self.single_host() {
            self.max_runners.min(MACOS_VMS_PER_HOST)
        } else {
            self.max_runners
        }
    }

    /// Idle runners to keep (default: every slot).
    pub fn effective_warm(&self) -> u32 {
        self.warm
            .unwrap_or(self.max_runners)
            .min(self.effective_max())
    }

    /// Runner and Space names start with this.
    pub fn prefix(&self) -> String {
        self.name_prefix
            .clone()
            .unwrap_or_else(|| format!("cua-{}", self.name))
    }

    /// `job_timeout`.
    pub fn job_timeout(&self) -> Duration {
        parse_duration("job_timeout", &self.job_timeout).unwrap_or(Duration::from_secs(6 * 3600))
    }

    /// `idle_timeout`.
    pub fn idle_timeout(&self) -> Duration {
        parse_duration("idle_timeout", &self.idle_timeout).unwrap_or(Duration::from_secs(3600))
    }

    /// `register_timeout`.
    pub fn register_timeout(&self) -> Duration {
        parse_duration("register_timeout", &self.register_timeout)
            .unwrap_or(Duration::from_secs(600))
    }

    /// The Space size: what the pool sets, else for macOS on this computer
    /// a share of `host` (half the cores, and the memory less 8 GiB split
    /// between the slots, at most [`DEFAULT_MACOS_MEMORY_MB`]), else the
    /// Space defaults. The memory is capped because a golden fork restores
    /// the saved memory: measured on a Mac mini, a fork of a 60 GB VM took
    /// 42 s to the first job step and one of an 8 GB VM 19 s (a cold create
    /// 25 s); the number of vCPUs made no difference to the start.
    pub fn space_size(&self, host: Option<HostResources>) -> (Option<u32>, Option<u64>) {
        let slots = self.effective_max().max(1);
        // A fork of a checkpoint has the checkpoint's size unless asked.
        let auto = (self.guest_os() == GuestOs::Macos
            && self.on.trim() == "local"
            && !self.is_checkpoint())
        .then_some(host)
        .flatten();
        let cpus = self.cpus.or_else(|| auto.map(|h| (h.cpus / slots).max(2)));
        let memory = self.memory_mb.or_else(|| {
            auto.map(|h| {
                (h.memory_mb.saturating_sub(8192) / slots as u64)
                    .clamp(4096, DEFAULT_MACOS_MEMORY_MB)
            })
        });
        (cpus, memory)
    }
}

/// This computer's cores and memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostResources {
    /// Logical CPUs.
    pub cpus: u32,
    /// Memory in MiB.
    pub memory_mb: u64,
}

impl HostResources {
    /// This computer's, when they can be read.
    pub fn detect() -> Option<Self> {
        let cpus = std::thread::available_parallelism().ok()?.get() as u32;
        let memory_mb = host_memory_bytes()? / (1024 * 1024);
        Some(Self { cpus, memory_mb })
    }
}

#[cfg(target_os = "macos")]
fn host_memory_bytes() -> Option<u64> {
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    // SAFETY: sysctlbyname writes at most `len` bytes into `v`, a u64.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut v as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && v > 0).then_some(v)
}

#[cfg(target_os = "linux")]
fn host_memory_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = text
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn host_memory_bytes() -> Option<u64> {
    None
}

fn parse_duration(field: &str, v: &str) -> Result<Duration> {
    humantime::parse_duration(v.trim())
        .map_err(|e| Error::Config(format!("{field} {v:?}: {e} (for example 6h, 90m)")))
}

/// A pool name: 1 to 32 of `a-z`, `0-9` and `-`, not starting or ending
/// with `-`.
pub fn valid_pool_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn valid_prefix(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> PoolConfig {
        PoolConfig::parse(
            r#"
name = "trycua"
scope = "org:trycua"
labels = ["cua-ci"]

[github]
app_id = 12345
secret_service = "cua-ci-github-app"
secret_account = "trycua"
"#,
        )
        .unwrap()
    }

    #[test]
    fn defaults_are_a_two_runner_macos_pool_on_this_machine() {
        let c = sample();
        assert_eq!(c.image, DEFAULT_MACOS_IMAGE);
        assert_eq!(c.guest_os(), GuestOs::Macos);
        assert_eq!(c.on, "local");
        assert_eq!(c.effective_max(), 2);
        assert_eq!(c.effective_warm(), 2);
        assert_eq!(c.job_timeout(), Duration::from_secs(6 * 3600));
        assert_eq!(c.idle_timeout(), Duration::from_secs(3600));
        assert_eq!(c.egress, EgressMode::DenyPrivate);
        assert_eq!(c.prefix(), "cua-trycua");
        assert_eq!(c.github.api_url, "https://api.github.com");
        // Round trip.
        assert_eq!(PoolConfig::parse(&c.to_toml().unwrap()).unwrap(), c);
    }

    #[test]
    fn macos_on_one_machine_is_capped_at_two() {
        let mut c = sample();
        c.max_runners = 6;
        assert_eq!(c.effective_max(), 2);
        c.on = "host:mini-2".into();
        assert_eq!(c.effective_max(), 2);
        // Any of your computers: each has its own two.
        c.on = "auto".into();
        assert_eq!(c.effective_max(), 6);
        // Linux has no such limit.
        c.on = "local".into();
        c.image = "ghcr.io/trycua/linux:24.04".into();
        assert_eq!(c.guest_os(), GuestOs::Linux);
        assert_eq!(c.effective_max(), 6);
    }

    #[test]
    fn macos_on_this_machine_gets_a_share_of_it() {
        let c = sample();
        let host = HostResources {
            cpus: 20,
            memory_mb: 131072,
        };
        assert_eq!(c.space_size(Some(host)), (Some(10), Some(8192)));
        // A small Mac still gets the Space minimums.
        let small = HostResources {
            cpus: 2,
            memory_mb: 8192,
        };
        assert_eq!(c.space_size(Some(small)), (Some(2), Some(4096)));
        // What the pool sets wins; elsewhere the Space defaults apply.
        let mut set = c.clone();
        set.cpus = Some(4);
        assert_eq!(set.space_size(Some(host)).0, Some(4));
        let mut auto = c.clone();
        auto.on = "auto".into();
        assert_eq!(auto.space_size(Some(host)), (None, None));
    }

    #[test]
    fn a_checkpoint_image_names_its_os() {
        let mut c = sample();
        c.image = "checkpoint:xcode-26".into();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("os = \"macos\""), "{e}");
        c.os = Some(GuestOs::Macos);
        c.validate().unwrap();
        assert!(c.is_checkpoint());
        assert_eq!(c.effective_max(), 2);
        let host = HostResources {
            cpus: 20,
            memory_mb: 131072,
        };
        assert_eq!(
            c.space_size(Some(host)),
            (None, None),
            "the checkpoint's size"
        );
        c.disk_gb = Some(200);
        assert!(
            c.validate()
                .unwrap_err()
                .to_string()
                .contains("checkpoint's disk")
        );
    }

    #[test]
    fn labels_lead_with_githubs_own() {
        let l = runner_labels(GuestOs::Macos, "arm64", &["cua-ci".into(), "macOS".into()]);
        assert_eq!(l, ["self-hosted", "macOS", "ARM64", "cua", "cua-ci"]);
        let l = runner_labels(GuestOs::Linux, "x86_64", &[]);
        assert_eq!(l, ["self-hosted", "Linux", "X64", "cua"]);
    }

    #[test]
    fn scopes() {
        assert_eq!(
            Scope::parse("org:trycua").unwrap().api_prefix(),
            "orgs/trycua"
        );
        let r = Scope::parse("repo:trycua/cua").unwrap();
        assert_eq!(r.api_prefix(), "repos/trycua/cua");
        assert_eq!(r.default_pool_name(), "trycua-cua");
        assert_eq!(r.to_string(), "repo:trycua/cua");
        for bad in ["trycua", "org:", "repo:trycua", "org:a/b", "repo:a/b c"] {
            assert!(Scope::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn invalid_fields_say_which() {
        let base = sample();
        type Edit = Box<dyn Fn(&mut PoolConfig)>;
        let cases: Vec<(Edit, &str)> = vec![
            (Box::new(|c| c.name = "Bad Name".into()), "pool name"),
            (Box::new(|c| c.max_runners = 0), "max_runners"),
            (Box::new(|c| c.warm = Some(3)), "warm 3"),
            (Box::new(|c| c.job_timeout = "soon".into()), "job_timeout"),
            (Box::new(|c| c.idle_timeout = "5s".into()), "idle_timeout"),
            (Box::new(|c| c.on = "aws".into()), "on \"aws\""),
            (
                Box::new(|c| c.egress_deny = vec!["example.com".into()]),
                "example.com",
            ),
            (Box::new(|c| c.labels = vec!["a,b".into()]), "label"),
            (
                Box::new(|c| c.image = "ghcr.io/trycua/windows:2022".into()),
                "Windows",
            ),
            (Box::new(|c| c.github.app_id = None), "app_id"),
            (
                Box::new(|c| c.github.api_url = "http://example.com".into()),
                "api_url",
            ),
            (
                Box::new(|c| {
                    c.scope = Scope::parse("repo:a/b").unwrap();
                    c.runner_group = Some("g".into());
                }),
                "runner_group",
            ),
        ];
        for (edit, want) in cases {
            let mut c = base.clone();
            edit(&mut c);
            let e = c.validate().unwrap_err().to_string();
            assert!(e.contains(want), "{want}: {e}");
        }
        // Unknown keys are refused, not ignored.
        let e = PoolConfig::parse("name = \"x\"\nscope = \"org:x\"\nmax = 3\n").unwrap_err();
        assert!(e.to_string().contains("max"), "{e}");
    }

    #[test]
    fn the_file_holds_no_secret_and_is_private() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("trycua.toml");
        let c = sample();
        c.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("secret_service = \"cua-ci-github-app\""));
        assert!(!text.contains("PRIVATE KEY"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(PoolConfig::load(&p).unwrap(), c);
    }
}
