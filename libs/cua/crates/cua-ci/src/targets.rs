//! Capacity targets of `cua ci add`: a cloud account (AWS, Google Cloud,
//! Azure, Hetzner, Modal, MacStadium) or a computer reached over SSH that
//! serves Cua CI jobs besides this computer.
//!
//! This module holds what is the same for every target: what is kept on
//! this computer ([`TargetState`], never a secret), the safe defaults (a
//! monthly budget and a hard per-hour cap, both always set), the nearest
//! region, SSH address checks, the registration the hosted service gets for
//! a cloud pool, and the small files the capacity manager (in `cua ci
//! serve`) and the CLI share: its report for `status` and a teardown
//! request for `remove`. Provider tokens never go to the hosted service:
//! they are kept in the local credential store only.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::hosted::JobRunner;
use crate::paths::CiPaths;
use crate::service_api::{
    ByocInfo, Class, MachineClass, MachineCost, MachineKind, MachineRegistration, format_dollars,
};
use crate::{Error, Result};

/// The monthly budget `cua ci add` suggests, in dollars.
pub const DEFAULT_BUDGET_USD: f64 = 50.0;

/// The most a budget or cap may be (a typo guard).
pub const MAX_BUDGET_USD: f64 = 1_000_000.0;

/// What a target is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Amazon Web Services (no stored key: a role your account trusts).
    Aws,
    /// Google Cloud (workload identity federation).
    Gcp,
    /// Azure (a managed identity's federated credential).
    Azure,
    /// Hetzner Cloud (a project token kept on this computer).
    Hetzner,
    /// Modal sandboxes (a token kept on this computer).
    Modal,
    /// MacStadium Macs (a token kept on this computer).
    Macstadium,
    /// A computer you reach over SSH (rented or owned).
    Host,
}

impl Kind {
    /// Parses `aws`, `gcp`, `azure`, `hetzner`, `modal`, `macstadium`,
    /// `host`.
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "aws" => Ok(Self::Aws),
            "gcp" => Ok(Self::Gcp),
            "azure" => Ok(Self::Azure),
            "hetzner" => Ok(Self::Hetzner),
            "modal" => Ok(Self::Modal),
            "macstadium" => Ok(Self::Macstadium),
            "host" => Ok(Self::Host),
            other => Err(format!(
                "{other:?}: choose aws, gcp, azure, hetzner, modal, macstadium, or `host \
                 USER@ADDRESS`"
            )),
        }
    }

    /// The word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Gcp => "gcp",
            Self::Azure => "azure",
            Self::Hetzner => "hetzner",
            Self::Modal => "modal",
            Self::Macstadium => "macstadium",
            Self::Host => "host",
        }
    }

    /// A name for people.
    pub fn title(self) -> &'static str {
        match self {
            Self::Aws => "AWS",
            Self::Gcp => "Google Cloud",
            Self::Azure => "Azure",
            Self::Hetzner => "Hetzner",
            Self::Modal => "Modal",
            Self::Macstadium => "MacStadium",
            Self::Host => "Computer",
        }
    }

    /// Linked with no stored key (a role or identity the account trusts).
    pub fn is_federated(self) -> bool {
        matches!(self, Self::Aws | Self::Gcp | Self::Azure)
    }

    /// Needs a token, kept on this computer only.
    pub fn is_token(self) -> bool {
        matches!(self, Self::Hetzner | Self::Modal | Self::Macstadium)
    }

    /// Spends money in a cloud (so it has a budget and a cap).
    pub fn is_cloud(self) -> bool {
        self != Self::Host
    }

    /// Where its token lives in the credential store.
    pub fn token_service(self) -> Option<&'static str> {
        match self {
            Self::Hetzner => Some("cua-ci-hetzner-token"),
            Self::Modal => Some("cua-ci-modal-token"),
            Self::Macstadium => Some("cua-ci-macstadium-token"),
            _ => None,
        }
    }

    /// What to paste, in one phrase.
    pub fn token_what(self) -> &'static str {
        match self {
            Self::Hetzner => "a Hetzner project token (Read and Write)",
            Self::Modal => "a Modal token as ID:SECRET",
            Self::Macstadium => "a MacStadium API token",
            _ => "a token",
        }
    }

    /// The classes a target of this kind serves by default.
    pub fn default_classes(self) -> Vec<(Class, &'static str)> {
        match self {
            Self::Macstadium => vec![(Class::Macos, "arm64")],
            _ => vec![(Class::Linux, "x64")],
        }
    }

    /// Most jobs at once of a new target of this kind.
    pub fn default_slots(self) -> u32 {
        match self {
            Self::Macstadium => 2,
            _ => 8,
        }
    }
}

/// What is kept about one target (no secret).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetState {
    /// Its name (`aws`, `hetzner`, `host-build1`).
    pub name: String,
    /// What it is.
    pub kind: Kind,
    /// Region or location, chosen near this computer unless given.
    #[serde(default)]
    pub region: Option<String>,
    /// Monthly budget in dollars (cloud targets).
    #[serde(default)]
    pub budget_usd_month: f64,
    /// Hard cap in dollars an hour (cloud targets).
    #[serde(default)]
    pub max_per_hour_usd: f64,
    /// Computers end when no job needs them.
    #[serde(default = "yes")]
    pub scale_to_zero: bool,
    /// An AWS Mac host pool (macOS jobs) rather than Linux.
    #[serde(default)]
    pub macos: bool,
    /// The SSH address of a `host` target.
    #[serde(default)]
    pub address: Option<String>,
    /// A `host` target the capacity manager runs (its private key is in
    /// this computer's keychain; Cua installs and removes its own units
    /// over SSH) rather than a computer that signs in to Cua itself.
    #[serde(default)]
    pub managed: bool,
    /// `arm64` or `x64` for a managed host (what the probe found).
    #[serde(default)]
    pub arch: Option<String>,
    /// The AWS account id.
    #[serde(default)]
    pub account_id: Option<String>,
    /// The id this pool has at the hosted service (a cloud pool).
    #[serde(default)]
    pub machine_id: Option<String>,
    /// Most jobs at once.
    #[serde(default)]
    pub max_slots: u32,
    /// Free slots to keep ready (a warm reserve) while jobs are arriving,
    /// so a job does not wait for a cloud boot. 0: off (the default): every
    /// idle host ends after its idle time. It only runs inside a share of
    /// the budget caps and never buys a minimum-allocation host.
    #[serde(default)]
    pub warm_slots: u32,
    /// `--warm N`: the owner fixed the number of free slots kept ready (0:
    /// never). Unset: the autopilot decides from the job history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<u32>,
    /// What the autopilot optimizes: `cheapest`, `balanced` (default when
    /// empty) or `fastest`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
    /// When added (RFC 3339).
    #[serde(default)]
    pub added_at: String,
}

fn yes() -> bool {
    true
}

/// A target's name for a kind and an address.
pub fn target_name(kind: Kind, address: Option<&str>) -> std::result::Result<String, String> {
    match (kind, address) {
        (Kind::Host, Some(a)) => {
            let (user_host, _) = parse_ssh_address(a)?;
            let host = user_host.rsplit('@').next().unwrap_or(&user_host);
            let slug: String = host
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            Ok(format!("host-{}", slug.trim_matches('-')))
        }
        (Kind::Host, None) => Err("host needs an SSH address: cua ci add host USER@ADDRESS".into()),
        (k, None) => Ok(k.as_str().to_string()),
        (k, Some(_)) => Err(format!("{} takes no address", k.as_str())),
    }
}

/// The valid characters of a target name.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 48
        && n.starts_with(|c: char| c.is_ascii_alphanumeric())
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl TargetState {
    /// A new state with the defaults of `kind`.
    pub fn new(kind: Kind, name: &str) -> Self {
        Self {
            name: name.to_string(),
            kind,
            region: None,
            budget_usd_month: if kind.is_cloud() {
                DEFAULT_BUDGET_USD
            } else {
                0.0
            },
            max_per_hour_usd: if kind.is_cloud() {
                default_max_per_hour(DEFAULT_BUDGET_USD)
            } else {
                0.0
            },
            scale_to_zero: true,
            macos: false,
            address: None,
            managed: false,
            arch: None,
            account_id: None,
            machine_id: None,
            max_slots: kind.default_slots(),
            warm_slots: 0,
            warm: None,
            goal: String::new(),
            added_at: String::new(),
        }
    }

    /// Whether a capacity manager runs this target inside `cua ci serve`:
    /// every cloud, and an SSH host kept under management.
    pub fn has_manager(&self) -> bool {
        self.kind.is_cloud() || self.managed
    }

    /// The reserve the owner fixed, if they did (`--warm`; a state file
    /// from before the autopilot with warm slots set counts as fixed).
    pub fn fixed_warm(&self) -> Option<u32> {
        self.warm
            .or((self.warm_slots > 0).then_some(self.warm_slots))
    }

    /// The goal the autopilot plans for.
    pub fn goal_or_default(&self) -> &str {
        if self.goal.is_empty() {
            "balanced"
        } else {
            &self.goal
        }
    }

    /// Checks the state.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !valid_name(&self.name) {
            return Err(format!("{:?} is not a valid target name", self.name));
        }
        if self.has_manager() {
            validate_caps(self.budget_usd_month, self.max_per_hour_usd)?;
            if self.max_slots == 0 || self.max_slots > 256 {
                return Err("jobs at once: 1 to 256".into());
            }
            let warm = self.fixed_warm().unwrap_or(0);
            if warm > self.max_slots {
                return Err(format!(
                    "warm slots ({warm}) cannot exceed jobs at once ({})",
                    self.max_slots
                ));
            }
            if !matches!(self.goal.as_str(), "" | "cheapest" | "balanced" | "fastest") {
                return Err(format!(
                    "goal {:?}: use cheapest, balanced or fastest",
                    self.goal
                ));
            }
        }
        if self.kind == Kind::Host && self.address.is_none() {
            return Err("a host target needs its SSH address".into());
        }
        Ok(())
    }

    /// The registration the hosted service gets for a cloud pool.
    /// `cost.per_slot_hour_micros` is the hard hourly cap divided by the
    /// slots: an upper bound until the capacity manager reports real cost.
    pub fn registration(&self, client_version: &str) -> MachineRegistration {
        let mut classes: Vec<(Class, &str)> = self.kind.default_classes();
        if self.macos {
            classes = vec![(Class::Macos, "arm64")];
        } else if let Some(a) = self.arch.as_deref().filter(|_| self.managed) {
            classes = vec![(Class::Linux, if a == "arm64" { "arm64" } else { "x64" })];
        }
        let slots = self.max_slots.max(1);
        let os = if classes.iter().any(|(c, _)| *c == Class::Macos) {
            "macos"
        } else {
            "linux"
        };
        let arch = classes.first().map(|(_, a)| *a).unwrap_or("x64");
        MachineRegistration {
            name: format!("{} ({})", self.kind.title(), self.name),
            kind: MachineKind::Byoc,
            os: os.into(),
            arch: arch.into(),
            client_version: client_version.into(),
            classes: classes
                .into_iter()
                .map(|(class, arch)| MachineClass {
                    class,
                    arch: arch.into(),
                    max_slots: slots,
                    cpus: None,
                    memory_mb: None,
                    disk_gb: None,
                    warm_target: 0,
                    image: None,
                })
                .collect(),
            byoc: Some(ByocInfo {
                provider: if self.macos && self.kind == Kind::Aws {
                    "aws-mac".into()
                } else if self.managed {
                    "ssh".into()
                } else {
                    self.kind.as_str().into()
                },
                region: self.region.clone().unwrap_or_default(),
                max_slots: slots,
                scale_to_zero: self.scale_to_zero,
            }),
            cost: MachineCost {
                per_slot_hour_micros: (self.max_per_hour_usd * 1_000_000.0 / f64::from(slots))
                    .round() as u64,
            },
        }
    }
}

/// The hard per-hour cap suggested for a monthly budget: a twentieth of it,
/// at least a dollar, in whole cents.
pub fn default_max_per_hour(budget_usd_month: f64) -> f64 {
    let v = (budget_usd_month / 20.0).max(1.0);
    (v * 100.0).round() / 100.0
}

/// Both caps are required and sensible: positive, the hourly cap no more
/// than the monthly budget.
pub fn validate_caps(
    budget_usd_month: f64,
    max_per_hour_usd: f64,
) -> std::result::Result<(), String> {
    for (name, v) in [
        ("budget", budget_usd_month),
        ("hourly cap", max_per_hour_usd),
    ] {
        if !v.is_finite() || v <= 0.0 {
            return Err(format!("the {name} must be more than 0 dollars"));
        }
        if v > MAX_BUDGET_USD {
            return Err(format!("the {name} is at most {MAX_BUDGET_USD:.0} dollars"));
        }
    }
    if max_per_hour_usd > budget_usd_month {
        return Err("the hourly cap is more than the monthly budget".into());
    }
    Ok(())
}

/// Parses a dollar amount typed at a prompt (`50`, `$12.5`).
pub fn parse_usd(s: &str) -> std::result::Result<f64, String> {
    let t = s.trim().trim_start_matches('$');
    let v: f64 = t
        .parse()
        .map_err(|_| format!("{s:?}: a dollar amount such as 50"))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(format!("{s:?}: more than 0"));
    }
    Ok((v * 100.0).round() / 100.0)
}

/// The budget of the whole account at the service: the sum of the cloud
/// targets' budgets.
pub fn account_budget_micros(targets: &[TargetState]) -> u64 {
    targets
        .iter()
        .filter(|t| t.kind.is_cloud())
        .map(|t| (t.budget_usd_month * 1_000_000.0).round() as u64)
        .sum()
}

/// `+0200` as minutes.
pub fn parse_utc_offset(z: &str) -> Option<i32> {
    let z = z.trim();
    let (sign, rest) = match z.chars().next()? {
        '+' => (1, &z[1..]),
        '-' => (-1, &z[1..]),
        _ => return None,
    };
    if rest.len() != 4 || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i32 = rest[..2].parse().ok()?;
    let m: i32 = rest[2..].parse().ok()?;
    (h <= 14 && m < 60).then_some(sign * (h * 60 + m))
}

/// A region near the clock's offset from UTC, cheap and common for the
/// kind (`None` for kinds with no region).
pub fn nearest_region(kind: Kind, utc_offset_min: i32) -> Option<&'static str> {
    // Americas west of Chicago, Americas east, Europe and Africa, Asia and
    // Oceania, by offset.
    let zone = match utc_offset_min {
        i32::MIN..=-420 => 0, // Pacific, Mountain
        -419..=-180 => 1,     // Central, Eastern, South America
        -179..=239 => 2,      // Atlantic Europe to Moscow
        _ => 3,
    };
    match kind {
        Kind::Aws => Some(["us-west-2", "us-east-2", "eu-central-1", "ap-southeast-1"][zone]),
        Kind::Gcp => Some(["us-west1", "us-central1", "europe-west1", "asia-southeast1"][zone]),
        Kind::Azure => Some(["westus2", "eastus2", "westeurope", "southeastasia"][zone]),
        Kind::Hetzner => Some(["hil", "ash", "fsn1", "sin"][zone]),
        Kind::Modal | Kind::Macstadium | Kind::Host => None,
    }
}

/// Checks an SSH address `[user@]host[:port]` and returns `[user@]host` and
/// the port. Anything ssh could read as an option or a shell as a command
/// is refused.
pub fn parse_ssh_address(a: &str) -> std::result::Result<(String, Option<u16>), String> {
    let a = a.trim();
    let bad = |why: &str| Err(format!("{a:?}: {why}"));
    if a.is_empty() {
        return bad("an SSH address is needed (USER@ADDRESS)");
    }
    if a.starts_with('-') {
        return bad("an address cannot start with a dash");
    }
    if a.len() > 255 {
        return bad("too long");
    }
    let (rest, port) = match a.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => {
            let n: u16 = p
                .parse()
                .map_err(|_| format!("{a:?}: the port is a number"))?;
            if n == 0 {
                return bad("the port is 1 to 65535");
            }
            (h, Some(n))
        }
        Some(_) => return bad("IPv6 addresses are not supported here; use a host name"),
        None => (a, None),
    };
    let (user, host) = match rest.split_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            && !s.starts_with('-')
    };
    if !ok(host) {
        return bad("the host is letters, digits, dots and dashes");
    }
    if let Some(u) = user
        && !ok(u)
    {
        return bad("the user is letters, digits, dots, dashes and underscores");
    }
    Ok((rest.to_string(), port))
}

/// The `ssh` arguments that run `remote` on `address` with a terminal.
pub fn ssh_args(address: &str, remote: &str) -> std::result::Result<Vec<String>, String> {
    let (user_host, port) = parse_ssh_address(address)?;
    let mut v = vec![
        "-t".to_string(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
    ];
    if let Some(p) = port {
        v.push("-p".into());
        v.push(p.to_string());
    }
    v.push("--".into());
    v.push(user_host);
    v.push(remote.to_string());
    Ok(v)
}

/// What runs on a rented or owned computer: install cua when it is missing,
/// then connect that computer as one more computer of the account.
pub fn host_install_script(machine_name: Option<&str>) -> String {
    let name = machine_name
        .map(|n| format!(" --name {}", crate::guest::sh_quote(n)))
        .unwrap_or_default();
    format!(
        "set -e; command -v cua >/dev/null 2>&1 || {{ curl -fsSL https://cua.ai/install.sh | sh -s -- --cli-only; \
         export PATH=\"$HOME/.local/bin:$HOME/.cua/bin:$PATH\"; }}; cua ci connect --no-browser{name}"
    )
}

/// What runs on a host to take it out again.
pub const HOST_REMOVE_SCRIPT: &str =
    "export PATH=\"$HOME/.local/bin:$HOME/.cua/bin:$PATH\"; cua ci disconnect --yes";

// ---------------------------------------------------------------------
// Local files
// ---------------------------------------------------------------------

/// The targets kept on this computer.
pub struct TargetStore {
    dir: PathBuf,
}

impl TargetStore {
    /// Under `paths`.
    pub fn new(paths: &CiPaths) -> Self {
        Self {
            dir: paths.root().join("targets"),
        }
    }

    /// A target's own directory (state, report, teardown files).
    pub fn dir(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn state_path(&self, name: &str) -> PathBuf {
        self.dir(name).join("target.json")
    }

    /// Every target, by name.
    pub fn list(&self) -> Vec<TargetState> {
        let mut out: Vec<TargetState> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let t = std::fs::read_to_string(e.path().join("target.json")).ok()?;
                serde_json::from_str(&t).ok()
            })
            .collect();
        out.sort_by(|a: &TargetState, b| a.name.cmp(&b.name));
        out
    }

    /// One target.
    pub fn get(&self, name: &str) -> Result<Option<TargetState>> {
        if !valid_name(name) {
            return Err(Error::Config(format!(
                "{name:?} is not a valid target name"
            )));
        }
        match std::fs::read_to_string(self.state_path(name)) {
            Ok(t) => serde_json::from_str(&t)
                .map(Some)
                .map_err(|e| Error::Config(format!("{}: {e}", self.state_path(name).display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Saves a target (mode 0600, through a temporary file).
    pub fn save(&self, t: &TargetState) -> Result<()> {
        t.validate().map_err(Error::Config)?;
        let path = self.state_path(&t.name);
        cua_home::guard_write(&path)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        write_json(&path, t)
    }

    /// Forgets a target and its files.
    pub fn remove(&self, name: &str) -> Result<bool> {
        if !valid_name(name) {
            return Err(Error::Config(format!(
                "{name:?} is not a valid target name"
            )));
        }
        match std::fs::remove_dir_all(self.dir(name)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn write_json<T: Serialize>(path: &std::path::Path, v: &T) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(v).unwrap_or_default())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A host the capacity manager holds, as `PoolStatus` prints it (the
/// manager's own type is serialized to this shape).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostView {
    /// Name.
    #[serde(default)]
    pub name: String,
    /// Type.
    #[serde(default)]
    pub type_name: String,
    /// `allocating`, `booting`, `ready`.
    #[serde(default)]
    pub state: String,
    /// Slots.
    #[serde(default)]
    pub slots: u32,
    /// Free slots.
    #[serde(default)]
    pub free_slots: u32,
    /// Micro-dollars an hour.
    #[serde(default)]
    pub rate_micros_per_hour: u64,
}

/// The capacity manager's `PoolStatus`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PoolStatusView {
    /// The target.
    #[serde(default)]
    pub target: String,
    /// Hosts held now.
    #[serde(default)]
    pub hosts: Vec<HostView>,
    /// Free slots.
    #[serde(default)]
    pub free_slots: u32,
    /// Busy slots.
    #[serde(default)]
    pub busy_slots: u32,
    /// Micro-dollars an hour of the hosts held.
    #[serde(default)]
    pub live_rate_micros: u64,
    /// Micro-dollars committed this month.
    #[serde(default)]
    pub month_committed_micros: u64,
    /// The monthly budget.
    #[serde(default)]
    pub monthly_micros: u64,
    /// `ok`, `warn`, `exhausted`.
    #[serde(default)]
    pub budget_state: String,
    /// What the autopilot decided (absent when it is off or has no report).
    #[serde(default)]
    pub autopilot: Option<AutopilotView>,
}

/// The autopilot's decisions for a target, as the manager reports them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutopilotView {
    /// `cheapest`, `balanced` or `fastest`.
    #[serde(default)]
    pub goal: String,
    /// `--warm N` fixed the reserve.
    #[serde(default)]
    pub warm_override: Option<u32>,
    /// One entry per class of job seen.
    #[serde(default)]
    pub classes: Vec<ClassPilotView>,
}

/// The autopilot's plan for one class of job.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClassPilotView {
    /// The class.
    #[serde(default)]
    pub class: String,
    /// `learning` or `steady`.
    #[serde(default)]
    pub phase: String,
    /// Jobs learned from.
    #[serde(default)]
    pub jobs_seen: usize,
    /// Free slots kept ready now.
    #[serde(default)]
    pub warm_now: u32,
    /// Seconds an idle computer is kept.
    #[serde(default)]
    pub idle_ttl_secs: u64,
    /// Spot is used.
    #[serde(default)]
    pub spot: bool,
    /// What the planned reserve costs a month, micro-dollars.
    #[serde(default)]
    pub reserve_month_micros: u64,
    /// Free slots kept ready in each hour of the week (UTC, Monday 0).
    #[serde(default)]
    pub weekly: Vec<u8>,
    /// The decisions in plain words.
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// The lines of `cua ci why` for a target: the goal, then each class's
/// decisions in words.
pub fn why_lines(t: &TargetState, r: Option<&TargetReport>) -> Vec<String> {
    let goal = t.goal_or_default();
    let mut v = vec![format!("{}: the {goal} goal.", t.name)];
    if let Some(n) = t.fixed_warm() {
        v.push(format!(
            "You fixed the ready reserve at {n} slot(s) with --warm; everything else is chosen from your jobs."
        ));
    }
    let Some(a) = r.and_then(|r| r.status.autopilot.as_ref()) else {
        v.push("No plan yet: it appears once `cua ci serve` has run jobs for this target.".into());
        return v;
    };
    if a.classes.is_empty() {
        v.push("No jobs yet. Until 20 jobs are seen, computers are not kept ready and an idle one ends after 10 minutes.".into());
    }
    for c in &a.classes {
        v.push(format!(
            "{} ({}, {} jobs seen): {} slot(s) ready now, idle computers kept {} minute(s), {}.",
            c.class,
            c.phase,
            c.jobs_seen,
            c.warm_now,
            c.idle_ttl_secs.div_ceil(60),
            if c.spot {
                "spot capacity"
            } else {
                "on-demand capacity"
            }
        ));
        for l in &c.reasons {
            v.push(format!("  {l}"));
        }
    }
    v.push(format!(
        "Spending never passes the budget ({} a month, {} an hour); if the cloud cannot start a computer a job still starts cold.",
        format_dollars((t.budget_usd_month * 1_000_000.0) as u64),
        format_dollars((t.max_per_hour_usd * 1_000_000.0) as u64)
    ));
    v
}

/// The capacity manager's `CostReport` (per-minute figures are
/// micro-dollars a minute).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CostReportView {
    /// `2026-10`.
    #[serde(default)]
    pub month: String,
    /// Jobs finished.
    #[serde(default)]
    pub jobs: u64,
    /// Minutes the slots were held.
    #[serde(default)]
    pub billed_minutes: f64,
    /// Everything the hosts cost.
    #[serde(default)]
    pub total_micros: u64,
    /// Idle, boot and minimum allocations.
    #[serde(default)]
    pub overhead_micros: u64,
    /// All-in micro-dollars a minute.
    #[serde(default)]
    pub per_minute_all_in_micros: f64,
    /// Slot time only.
    #[serde(default)]
    pub per_minute_compute_micros: f64,
    /// Share on spot hosts.
    #[serde(default)]
    pub spot_share: f64,
    /// A price in the month is not verified.
    #[serde(default)]
    pub unverified_prices: bool,
    /// Slot utilization of the paid time.
    #[serde(default)]
    pub utilization: f64,
}

/// What the manager writes for `cua ci status` and `cua ci add`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TargetReport {
    /// When (RFC 3339).
    #[serde(default)]
    pub at: String,
    /// The pool.
    #[serde(default)]
    pub status: PoolStatusView,
    /// The month's cost.
    #[serde(default)]
    pub cost: CostReportView,
}

/// One row of a teardown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeardownItem {
    /// The cloud id.
    #[serde(default)]
    pub id: String,
    /// Name.
    #[serde(default)]
    pub name: String,
    /// `deleted`, `would_delete`, `kept`, `held`, `forgotten`, `failed`.
    #[serde(default)]
    pub action: String,
    /// Why.
    #[serde(default)]
    pub reason: String,
}

/// A request to end the target's computers (through the ledger: only the
/// resources Cua recorded and tagged).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeardownRequest {
    /// When asked (RFC 3339).
    #[serde(default)]
    pub requested_at: String,
    /// Report what would be deleted, delete nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// The answer to a teardown request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeardownResult {
    /// The request it answers.
    #[serde(default)]
    pub requested_at: String,
    /// When done.
    #[serde(default)]
    pub at: String,
    /// What happened to each host.
    #[serde(default)]
    pub items: Vec<TeardownItem>,
}

impl TeardownResult {
    /// Hosts that could not be ended, or are held by a minimum allocation.
    pub fn left(&self) -> Vec<&TeardownItem> {
        self.items
            .iter()
            .filter(|i| matches!(i.action.as_str(), "failed" | "held" | "kept"))
            .collect()
    }
}

impl TargetStore {
    fn report_path(&self, name: &str) -> PathBuf {
        self.dir(name).join("report.json")
    }

    /// The manager writes its report (every reconcile).
    pub fn write_report(&self, name: &str, r: &TargetReport) -> Result<()> {
        let p = self.report_path(name);
        cua_home::guard_write(&p)?;
        std::fs::create_dir_all(self.dir(name))?;
        write_json(&p, r)
    }

    /// The last report, if the manager wrote one.
    pub fn read_report(&self, name: &str) -> Option<TargetReport> {
        serde_json::from_str(&std::fs::read_to_string(self.report_path(name)).ok()?).ok()
    }

    /// The CLI asks for a teardown (replacing any older answer).
    pub fn request_teardown(&self, name: &str, req: &TeardownRequest) -> Result<()> {
        let d = self.dir(name);
        cua_home::guard_write(&d)?;
        std::fs::create_dir_all(&d)?;
        let _ = std::fs::remove_file(d.join("teardown.result.json"));
        write_json(&d.join("teardown.request.json"), req)
    }

    /// The manager takes the request (it is removed once read).
    pub fn take_teardown_request(&self, name: &str) -> Option<TeardownRequest> {
        let p = self.dir(name).join("teardown.request.json");
        let r = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
        let _ = std::fs::remove_file(&p);
        Some(r)
    }

    /// Whether a request waits.
    pub fn teardown_pending(&self, name: &str) -> bool {
        self.dir(name).join("teardown.request.json").exists()
    }

    /// The manager answers.
    pub fn write_teardown_result(&self, name: &str, r: &TeardownResult) -> Result<()> {
        write_json(&self.dir(name).join("teardown.result.json"), r)
    }

    /// The answer, once there is one.
    pub fn read_teardown_result(&self, name: &str) -> Option<TeardownResult> {
        serde_json::from_str(
            &std::fs::read_to_string(self.dir(name).join("teardown.result.json")).ok()?,
        )
        .ok()
    }
}

/// Makes the job runner of a cloud target. The capacity managers live in
/// `cua-byoc`; the CLI plugs them in here.
pub trait TargetRunners: Send + Sync {
    /// The runner for `target`, or `None` when this build cannot run it.
    fn runner_for(&self, target: &TargetState, paths: &CiPaths) -> Option<Arc<dyn JobRunner>>;
}

// ---------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------

/// `$0.0042 a minute` from micro-dollars a minute.
pub fn per_minute(micros: f64) -> String {
    let d = micros / 1_000_000.0;
    if d >= 0.01 {
        format!("${d:.3} a minute")
    } else {
        format!("${d:.4} a minute")
    }
}

/// The cost lines of a report (empty before any job ran).
pub fn cost_lines(c: &CostReportView) -> Vec<String> {
    if c.jobs == 0 {
        return vec!["Cost a minute: shown here after the first jobs".into()];
    }
    let mut v = vec![format!(
        "Cost a minute: {} all in, {} for the job time alone ({} jobs, {:.0} minutes this month)",
        per_minute(c.per_minute_all_in_micros),
        per_minute(c.per_minute_compute_micros),
        c.jobs,
        c.billed_minutes
    )];
    v.push(format!(
        "Computers were busy {:.0} percent of the time they were paid for{}",
        c.utilization * 100.0,
        if c.spot_share > 0.0 {
            format!(", {:.0} percent on spot", c.spot_share * 100.0)
        } else {
            String::new()
        }
    ));
    if c.unverified_prices {
        v.push("Some prices behind this are estimates, not yet confirmed by your bill".into());
    }
    v
}

/// The lines `cua ci status` and `cua ci add` print for a target.
pub fn target_lines(t: &TargetState, r: Option<&TargetReport>) -> Vec<String> {
    let mut v = Vec::new();
    match t.kind {
        Kind::Host if !t.managed => {
            v.push(format!(
                "  target    {} ({}), a computer you reach over SSH",
                t.name,
                t.address.clone().unwrap_or_default()
            ));
        }
        k => {
            v.push(format!(
                "  target    {}{}  budget {} a month, hard cap {} an hour{}",
                t.name,
                t.region
                    .as_ref()
                    .map(|r| format!(" in {r}"))
                    .unwrap_or_default(),
                format_dollars((t.budget_usd_month * 1_000_000.0) as u64),
                format_dollars((t.max_per_hour_usd * 1_000_000.0) as u64),
                if t.scale_to_zero {
                    ", no computers run while idle"
                } else {
                    ""
                }
            ));
            let _ = k;
            if let Some(r) = r {
                let s = &r.status;
                v.push(format!(
                    "            {} computer{} running, {} of {} slots free; spent or committed {} of {} ({})",
                    s.hosts.len(),
                    if s.hosts.len() == 1 { "" } else { "s" },
                    s.free_slots,
                    s.free_slots + s.busy_slots,
                    format_dollars(s.month_committed_micros),
                    format_dollars(s.monthly_micros),
                    if s.budget_state.is_empty() { "ok" } else { &s.budget_state }
                ));
                if let Some(c) = s.autopilot.as_ref().and_then(|a| a.classes.first()) {
                    v.push(format!(
                        "            autopilot ({}): {} slot(s) ready now, idle computers kept {} minute(s){}; `cua ci why` says why",
                        t.goal_or_default(),
                        c.warm_now,
                        c.idle_ttl_secs.div_ceil(60),
                        if c.phase == "learning" { ", still learning" } else { "" }
                    ));
                }
                for l in cost_lines(&r.cost) {
                    v.push(format!("            {l}"));
                }
            } else {
                v.push(
                    "            no report yet: `cua ci serve` writes it once the service runs"
                        .into(),
                );
            }
        }
    }
    v
}

/// Whether a token looks like something a provider would hand out (not
/// empty, one line, no spaces): a typo guard, not a validity check.
pub fn plausible_token(t: &str) -> bool {
    let t = t.trim();
    t.len() >= 8 && t.len() <= 4096 && !t.contains(char::is_whitespace)
}

/// Checks a Hetzner project token against the API (nothing is created).
/// Other token providers have no cheap read to try, so they return `Ok`.
pub async fn verify_token(kind: Kind, token: &str) -> std::result::Result<(), String> {
    if kind != Kind::Hetzner {
        return Ok(());
    }
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let r = http
        .get("https://api.hetzner.cloud/v1/locations")
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|e| format!("could not reach Hetzner: {e}"))?;
    match r.status().as_u16() {
        200 => Ok(()),
        401 | 403 => {
            Err("Hetzner refused the token (it needs Read and Write on the project)".into())
        }
        s => Err(format!("Hetzner answered {s}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_parse() {
        for (w, k) in [
            ("aws", Kind::Aws),
            ("GCP", Kind::Gcp),
            ("azure", Kind::Azure),
            ("hetzner", Kind::Hetzner),
            ("modal", Kind::Modal),
            ("macstadium", Kind::Macstadium),
            ("host", Kind::Host),
        ] {
            assert_eq!(Kind::parse(w).unwrap(), k);
            assert_eq!(Kind::parse(k.as_str()).unwrap(), k);
        }
        assert!(
            Kind::parse("oracle")
                .unwrap_err()
                .contains("host USER@ADDRESS")
        );
        assert!(Kind::Aws.is_federated() && !Kind::Aws.is_token());
        assert!(Kind::Hetzner.is_token() && Kind::Macstadium.is_token() && Kind::Modal.is_token());
        assert!(!Kind::Host.is_cloud() && Kind::Aws.is_cloud());
        assert_eq!(Kind::Hetzner.token_service(), Some("cua-ci-hetzner-token"));
        assert_eq!(Kind::Aws.token_service(), None);
    }

    #[test]
    fn names() {
        assert_eq!(target_name(Kind::Aws, None).unwrap(), "aws");
        assert_eq!(
            target_name(Kind::Host, Some("root@Build-1.example.com:2222")).unwrap(),
            "host-build-1-example-com"
        );
        assert!(target_name(Kind::Host, None).is_err());
        assert!(target_name(Kind::Aws, Some("x")).is_err());
        assert!(
            valid_name("host-a1") && !valid_name("Host") && !valid_name("-a") && !valid_name("")
        );
        assert!(!valid_name("a/b") && !valid_name("../x"));
    }

    #[test]
    fn caps_are_both_required_and_sensible() {
        assert_eq!(default_max_per_hour(50.0), 2.5);
        assert_eq!(default_max_per_hour(10.0), 1.0);
        assert_eq!(default_max_per_hour(333.33), 16.67);
        validate_caps(50.0, 2.5).unwrap();
        assert!(validate_caps(0.0, 1.0).is_err());
        assert!(validate_caps(50.0, 0.0).is_err());
        assert!(
            validate_caps(50.0, 60.0)
                .unwrap_err()
                .contains("more than the monthly")
        );
        assert!(validate_caps(f64::NAN, 1.0).is_err());
        assert!(validate_caps(f64::INFINITY, 1.0).is_err());
        assert!(validate_caps(2_000_000.0, 1.0).is_err());
        assert_eq!(parse_usd("$12.506").unwrap(), 12.51);
        assert_eq!(parse_usd(" 50 ").unwrap(), 50.0);
        assert!(parse_usd("0").is_err() && parse_usd("-3").is_err() && parse_usd("x").is_err());
    }

    #[test]
    fn a_new_cloud_target_starts_safe() {
        let t = TargetState::new(Kind::Aws, "aws");
        assert_eq!(t.budget_usd_month, 50.0);
        assert_eq!(t.max_per_hour_usd, 2.5);
        assert!(t.scale_to_zero);
        t.validate().unwrap();
        let mut bad = t.clone();
        bad.max_per_hour_usd = 0.0;
        assert!(bad.validate().is_err());
        let mut bad = t.clone();
        bad.max_slots = 0;
        assert!(bad.validate().is_err());
        // A warm reserve is off by default and cannot exceed the slots.
        assert_eq!(t.warm_slots, 0);
        let mut warm = t.clone();
        warm.warm_slots = warm.max_slots;
        warm.validate().unwrap();
        warm.warm_slots = warm.max_slots + 1;
        assert!(warm.validate().unwrap_err().contains("warm slots"));
        // A state file from before the field still loads, with it off.
        let old = serde_json::to_string(&t)
            .unwrap()
            .replace(",\"warm_slots\":0", "");
        assert!(!old.contains("warm_slots"));
        assert_eq!(
            serde_json::from_str::<TargetState>(&old)
                .unwrap()
                .warm_slots,
            0
        );
        let h = TargetState::new(Kind::Host, "host-a");
        assert!(h.validate().unwrap_err().contains("SSH address"));
        let mut h = h;
        h.address = Some("root@a".into());
        h.validate().unwrap();
    }

    #[test]
    fn the_service_registration_of_a_cloud_pool_is_capped_and_scales_to_zero() {
        let mut t = TargetState::new(Kind::Hetzner, "hetzner");
        t.region = Some("fsn1".into());
        let r = t.registration("1");
        r.validate().unwrap();
        assert_eq!(r.kind, MachineKind::Byoc);
        assert_eq!(r.classes.len(), 1);
        assert_eq!(r.classes[0].class, Class::Linux);
        assert_eq!(r.classes[0].max_slots, 8);
        let b = r.byoc.unwrap();
        assert_eq!(
            (b.provider.as_str(), b.region.as_str(), b.scale_to_zero),
            ("hetzner", "fsn1", true)
        );
        // $2.50 an hour over 8 slots.
        assert_eq!(r.cost.per_slot_hour_micros, 312_500);
        let mut m = TargetState::new(Kind::Aws, "aws");
        m.macos = true;
        m.max_slots = 2;
        let r = m.registration("1");
        r.validate().unwrap();
        assert_eq!(r.classes[0].class, Class::Macos);
        assert_eq!(r.byoc.unwrap().provider, "aws-mac");
        let mac = TargetState::new(Kind::Macstadium, "macstadium").registration("1");
        assert_eq!(mac.classes[0].class, Class::Macos);
        assert_eq!(mac.classes[0].max_slots, 2);
    }

    #[test]
    fn the_account_budget_is_the_sum_of_the_clouds() {
        let mut a = TargetState::new(Kind::Aws, "aws");
        a.budget_usd_month = 80.0;
        let h = TargetState::new(Kind::Hetzner, "hetzner");
        let host = TargetState::new(Kind::Host, "host-x");
        assert_eq!(account_budget_micros(&[a, h, host]), 130_000_000);
        assert_eq!(account_budget_micros(&[]), 0);
    }

    #[test]
    fn regions_follow_the_clock() {
        assert_eq!(parse_utc_offset("+0200"), Some(120));
        assert_eq!(parse_utc_offset("-0800"), Some(-480));
        assert_eq!(parse_utc_offset("+0530"), Some(330));
        for bad in ["", "0200", "+02", "+2500", "+0261", "x"] {
            assert_eq!(parse_utc_offset(bad), None, "{bad}");
        }
        assert_eq!(nearest_region(Kind::Aws, -480), Some("us-west-2"));
        assert_eq!(nearest_region(Kind::Aws, -300), Some("us-east-2"));
        assert_eq!(nearest_region(Kind::Aws, 60), Some("eu-central-1"));
        assert_eq!(nearest_region(Kind::Aws, 540), Some("ap-southeast-1"));
        assert_eq!(nearest_region(Kind::Hetzner, 120), Some("fsn1"));
        assert_eq!(nearest_region(Kind::Hetzner, -360), Some("ash"));
        assert_eq!(nearest_region(Kind::Gcp, 60), Some("europe-west1"));
        assert_eq!(nearest_region(Kind::Azure, 330), Some("southeastasia"));
        assert_eq!(nearest_region(Kind::Modal, 0), None);
        assert_eq!(nearest_region(Kind::Host, 0), None);
    }

    #[test]
    fn ssh_addresses_cannot_smuggle_options_or_commands() {
        assert_eq!(
            parse_ssh_address("root@build.example.com").unwrap(),
            ("root@build.example.com".into(), None)
        );
        assert_eq!(
            parse_ssh_address("10.0.0.5:2222").unwrap(),
            ("10.0.0.5".into(), Some(2222))
        );
        assert_eq!(parse_ssh_address("me@box").unwrap().0, "me@box");
        for bad in [
            "",
            "-oProxyCommand=evil",
            "-x@host",
            "host;rm -rf",
            "host name",
            "host`id`",
            "a@b@c",
            "u@-host",
            "host:0",
            "host:99999",
            "host:abc",
            "::1",
            "host$(x)",
            "ho'st",
            "host\\n",
            "@host",
        ] {
            assert!(parse_ssh_address(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn ssh_runs_with_a_terminal_and_ends_options_before_the_address() {
        let a = ssh_args("root@box:2200", "echo hi").unwrap();
        assert_eq!(
            a,
            [
                "-t",
                "-o",
                "ServerAliveInterval=30",
                "-p",
                "2200",
                "--",
                "root@box",
                "echo hi"
            ]
        );
        let a = ssh_args("box", "x").unwrap();
        assert_eq!(a.last().unwrap(), "x");
        assert!(
            a.iter().position(|v| v == "--").unwrap() < a.iter().position(|v| v == "box").unwrap()
        );
        assert!(ssh_args("-oProxyCommand=x", "y").is_err());
    }

    #[test]
    fn the_host_script_installs_only_when_missing_and_quotes_the_name() {
        let s = host_install_script(None);
        assert!(s.contains("command -v cua") && s.contains("cua ci connect --no-browser"));
        assert!(s.contains("https://cua.ai/install.sh"));
        let s = host_install_script(Some("it's; rm -rf /"));
        assert!(s.contains("--name 'it'\\''s; rm -rf /'"), "{s}");
        assert!(HOST_REMOVE_SCRIPT.contains("cua ci disconnect --yes"));
    }

    #[test]
    fn the_store_round_trips_privately_and_lists_by_name() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        let s = TargetStore::new(&paths);
        assert!(s.list().is_empty());
        assert_eq!(s.get("aws").unwrap(), None);
        let mut t = TargetState::new(Kind::Aws, "aws");
        t.region = Some("us-east-2".into());
        s.save(&t).unwrap();
        s.save(&TargetState::new(Kind::Hetzner, "hetzner")).unwrap();
        assert_eq!(s.get("aws").unwrap(), Some(t.clone()));
        assert_eq!(
            s.list().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["aws", "hetzner"]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let m = std::fs::metadata(d.path().join("ci/targets/aws/target.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
        assert!(s.get("../x").is_err());
        assert!(s.remove("../x").is_err());
        assert!(s.remove("aws").unwrap());
        assert!(!s.remove("aws").unwrap());
        assert_eq!(s.list().len(), 1);
        // An invalid state is not saved.
        let mut bad = TargetState::new(Kind::Aws, "aws");
        bad.budget_usd_month = 0.0;
        assert!(s.save(&bad).is_err());
        assert!(s.get("aws").unwrap().is_none());
    }

    #[test]
    fn report_and_teardown_files_pass_between_the_manager_and_the_cli() {
        let d = tempfile::tempdir().unwrap();
        let s = TargetStore::new(&CiPaths::new(d.path().join("ci")));
        assert!(s.read_report("aws").is_none());
        let r = TargetReport {
            at: "t".into(),
            status: PoolStatusView {
                target: "aws".into(),
                free_slots: 3,
                busy_slots: 1,
                monthly_micros: 50_000_000,
                ..Default::default()
            },
            cost: CostReportView {
                jobs: 4,
                billed_minutes: 20.0,
                per_minute_all_in_micros: 4200.0,
                ..Default::default()
            },
        };
        s.write_report("aws", &r).unwrap();
        assert_eq!(s.read_report("aws"), Some(r));
        assert!(!s.teardown_pending("aws"));
        s.request_teardown(
            "aws",
            &TeardownRequest {
                requested_at: "t1".into(),
                dry_run: false,
            },
        )
        .unwrap();
        assert!(s.teardown_pending("aws"));
        assert!(s.read_teardown_result("aws").is_none());
        let req = s.take_teardown_request("aws").unwrap();
        assert_eq!(req.requested_at, "t1");
        assert!(!s.teardown_pending("aws") && s.take_teardown_request("aws").is_none());
        let res = TeardownResult {
            requested_at: "t1".into(),
            at: "t2".into(),
            items: vec![
                TeardownItem {
                    id: "i-1".into(),
                    action: "deleted".into(),
                    ..Default::default()
                },
                TeardownItem {
                    id: "i-2".into(),
                    action: "held".into(),
                    reason: "24 hour minimum".into(),
                    ..Default::default()
                },
            ],
        };
        s.write_teardown_result("aws", &res).unwrap();
        assert_eq!(s.read_teardown_result("aws").unwrap().left().len(), 1);
        // A new request clears the old answer.
        s.request_teardown("aws", &TeardownRequest::default())
            .unwrap();
        assert!(s.read_teardown_result("aws").is_none());
    }

    #[test]
    fn costs_read_in_plain_words() {
        assert_eq!(per_minute(4200.0), "$0.0042 a minute");
        assert_eq!(per_minute(15_000.0), "$0.015 a minute");
        let none = cost_lines(&CostReportView::default());
        assert!(none[0].contains("after the first jobs"));
        let c = CostReportView {
            jobs: 10,
            billed_minutes: 62.0,
            per_minute_all_in_micros: 6000.0,
            per_minute_compute_micros: 4000.0,
            utilization: 0.31,
            spot_share: 0.8,
            unverified_prices: true,
            ..Default::default()
        };
        let t = cost_lines(&c).join("\n");
        assert!(t.contains("$0.0060 a minute all in, $0.0040 a minute for the job time alone (10 jobs, 62 minutes"), "{t}");
        assert!(
            t.contains("31 percent of the time") && t.contains("80 percent on spot"),
            "{t}"
        );
        assert!(t.contains("estimates"), "{t}");
        assert!(!t.contains('\u{b7}'));
    }

    #[test]
    fn target_lines_say_budget_cap_and_state() {
        let mut t = TargetState::new(Kind::Aws, "aws");
        t.region = Some("us-east-2".into());
        let none = target_lines(&t, None).join("\n");
        assert!(none.contains("aws in us-east-2  budget $50.00 a month, hard cap $2.50 an hour, no computers run while idle"), "{none}");
        assert!(none.contains("no report yet"), "{none}");
        let r = TargetReport {
            at: String::new(),
            status: PoolStatusView {
                hosts: vec![HostView::default()],
                free_slots: 5,
                busy_slots: 3,
                month_committed_micros: 11_200_000,
                monthly_micros: 50_000_000,
                budget_state: "ok".into(),
                ..Default::default()
            },
            cost: CostReportView::default(),
        };
        let with = target_lines(&t, Some(&r)).join("\n");
        assert!(
            with.contains(
                "1 computer running, 5 of 8 slots free; spent or committed $11.20 of $50.00 (ok)"
            ),
            "{with}"
        );
        let mut h = TargetState::new(Kind::Host, "host-x");
        h.address = Some("root@x".into());
        assert!(
            target_lines(&h, None)
                .join("\n")
                .contains("a computer you reach over SSH")
        );
    }

    #[test]
    fn tokens_are_checked_for_shape_only() {
        assert!(plausible_token("abcdefgh12345678"));
        assert!(
            !plausible_token("short")
                && !plausible_token("has space inside")
                && !plausible_token("")
        );
    }

    #[test]
    fn the_goal_and_a_fixed_warm_are_checked_and_old_files_still_load() {
        let mut t = TargetState::new(Kind::Aws, "aws");
        assert_eq!(t.goal_or_default(), "balanced");
        assert_eq!(t.fixed_warm(), None, "unset: the autopilot decides");
        t.goal = "fastest".into();
        t.warm = Some(0);
        t.validate().unwrap();
        assert_eq!(t.fixed_warm(), Some(0), "`--warm 0` is a choice, not unset");
        t.goal = "quick".into();
        assert!(
            t.validate()
                .unwrap_err()
                .contains("cheapest, balanced or fastest")
        );
        t.goal = String::new();
        t.warm = Some(t.max_slots + 1);
        assert!(t.validate().unwrap_err().contains("warm slots"));
        // Before the autopilot a warm reserve was `warm_slots`: it still counts.
        let mut old = TargetState::new(Kind::Aws, "aws");
        old.warm_slots = 2;
        assert_eq!(old.fixed_warm(), Some(2));
        let json = serde_json::to_string(&TargetState::new(Kind::Aws, "aws")).unwrap();
        assert!(
            !json.contains("\"goal\"") && !json.contains("\"warm\":"),
            "{json}"
        );
    }

    fn pilot_report() -> TargetReport {
        TargetReport {
            status: PoolStatusView {
                autopilot: Some(AutopilotView {
                    goal: "balanced".into(),
                    warm_override: None,
                    classes: vec![ClassPilotView {
                        class: "linux-x64-4c-8192m".into(),
                        phase: "steady".into(),
                        jobs_seen: 412,
                        warm_now: 2,
                        idle_ttl_secs: 480,
                        spot: true,
                        reserve_month_micros: 4_000_000,
                        weekly: vec![0; 168],
                        reasons: vec![
                            "Spot capacity is used.".into(),
                            "An idle computer is kept 8 minute(s).".into(),
                        ],
                    }],
                }),
                ..PoolStatusView::default()
            },
            ..TargetReport::default()
        }
    }

    #[test]
    fn why_explains_the_plan_in_words_and_status_points_to_it() {
        let mut t = TargetState::new(Kind::Aws, "aws");
        t.budget_usd_month = 50.0;
        t.max_per_hour_usd = 2.5;
        let none = why_lines(&t, None).join("\n");
        assert!(
            none.contains("balanced goal") && none.contains("No plan yet"),
            "{none}"
        );
        let r = pilot_report();
        let text = why_lines(&t, Some(&r)).join("\n");
        assert!(
            text.contains("2 slot(s) ready now, idle computers kept 8 minute(s), spot capacity"),
            "{text}"
        );
        assert!(
            text.contains("An idle computer is kept 8 minute(s)."),
            "{text}"
        );
        assert!(
            text.contains("never passes the budget") && text.contains("cold"),
            "{text}"
        );
        t.warm = Some(1);
        assert!(why_lines(&t, Some(&r)).join("\n").contains("--warm"));
        let status = target_lines(&t, Some(&r)).join("\n");
        assert!(
            status.contains("autopilot (balanced): 2 slot(s) ready now")
                && status.contains("cua ci why"),
            "{status}"
        );
        for bad in ['\u{b7}', '\u{2014}'] {
            assert!(!text.contains(bad) && !status.contains(bad));
        }
        // A report written before the autopilot existed still reads.
        let old: TargetReport = serde_json::from_str("{\"status\":{\"target\":\"aws\"}}").unwrap();
        assert!(old.status.autopilot.is_none());
    }
}
