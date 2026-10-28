//! Where a Space or job goes: in-memory scoring over what each host last
//! reported ([`cua_proto::env::v1::HostNodeStatus`]), Modal-style. The
//! scheduler ranks the hosts that fit, asks the best one directly, and the
//! host accepts or rejects from its own free resources; a rejection moves
//! on to the next. Nothing here touches a database.

use cua_proto::env::v1 as pb;

/// How to choose among hosts that fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// The least loaded host first: spreads work (and the blast radius of
    /// a host failure). The default.
    #[default]
    Spread,
    /// The most loaded host that still fits first: packs work, so other
    /// hosts stay idle (and can sleep or be removed).
    Pack,
}

impl Policy {
    /// `spread` or `pack`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "spread" => Some(Self::Spread),
            "pack" | "binpack" => Some(Self::Pack),
            _ => None,
        }
    }
}

/// What the work needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Need {
    /// The image is on a registry only this network reaches
    /// ([`image_is_private`]): elastic capacity cannot pull it.
    pub private_image: bool,
    /// `container`, `vm`, `microvm`, or empty (any).
    pub kind: String,
    /// A runtime word, or empty (any).
    pub runtime: String,
    /// The host OS it needs (`macos` for a macOS guest, or what the
    /// submitter asked for), or empty.
    pub host_os: String,
    /// The host CPU architecture it needs, or empty.
    pub arch: String,
    /// The one host it must go to (by name), `cloud:<provider>` for any host
    /// of that cloud ([`cloud_pin`]), or empty.
    pub on: String,
    /// Its warm-pool shape ([`warm_key`]); empty: never warm.
    pub shape: String,
    /// CPU (thousandths).
    pub millicpus: u32,
    /// Memory (MiB).
    pub memory_mb: u64,
    /// The accelerator it needs: `gpu` (any), `apple`, `nvidia`, or empty
    /// (none), matched against `HostNodeStatus.accelerators`
    /// ([`crate::model::accelerator_fits`]).
    pub accelerator: String,
    /// It asks for host services (`HostJobSpec.services`): only a host that
    /// reports `HostNodeStatus.job_services` may run it, because an older
    /// host would drop the field and run the job without them.
    pub services: bool,
}

/// The provider of a `cloud:<provider>` placement (`cloud:aws`: any host
/// that cloud supplied to the pool), if `on` is one.
pub fn cloud_pin(on: &str) -> Option<&str> {
    on.strip_prefix("cloud:").filter(|p| !p.is_empty())
}

/// One host as the scheduler sees it now.
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    /// Its name.
    pub name: String,
    /// Healthy and not draining.
    pub ready: bool,
    /// Its OS (`linux`, `macos`, `windows`).
    pub os: String,
    /// What it last reported (`None`: a host that does not report).
    pub status: Option<pb::HostNodeStatus>,
    /// CPU the scheduler already sent it since that report (thousandths).
    pub pending_millicpus: u64,
    /// Memory sent since the report (MiB).
    pub pending_memory_mb: u64,
    /// Jobs sent since the report.
    pub pending_jobs: u32,
    /// Elastic capacity (a cloud provider that bills by the second): used
    /// only when no host fits, or when asked for by name.
    pub elastic: bool,
    /// Takes only work that names it (`on`): a machine that does not
    /// provide Spaces still runs its own user's local runs.
    pub pinned: bool,
    /// The cloud that supplied this host (`aws`), or empty: what a
    /// `cloud:<provider>` job (`Need::on`) matches.
    pub provider: String,
    /// The share (0..1) of its CPUs other work keeps busy right now (the
    /// load average). Only lowers its score, among hosts that fit; it never
    /// makes work misfit.
    pub busy: f64,
}

/// How far elastic (not yet running) capacity scores below every host that
/// fits now.
pub const ELASTIC_PENALTY: f64 = 3.0;
/// How much a fully busy host's score drops (a tie-break: a host with
/// more room left still wins unless the difference is small).
const BUSY_WEIGHT: f64 = 0.1;

impl Candidate {
    /// CPU free after what is pending.
    pub fn free_millicpus(&self) -> i64 {
        self.status
            .as_ref()
            .map_or(i64::MAX, |s| i64::from(s.millicpus_free))
            - self.pending_millicpus as i64
    }

    /// Memory free after what is pending.
    pub fn free_memory_mb(&self) -> i64 {
        self.status
            .as_ref()
            .map_or(i64::MAX, |s| s.memory_free_mb as i64)
            - self.pending_memory_mb as i64
    }

    /// The host's measured VM capacity, when `need` is a `vm` Space and the
    /// host measures it (`HostMeasured.vm_memory_known`).
    fn vm_measured(&self, need: &Need) -> Option<&pb::HostMeasured> {
        (need.kind == "vm")
            .then_some(self.measured())
            .flatten()
            .filter(|m| m.vm_memory_known)
    }

    /// What the host measured, when it reports it.
    fn measured(&self) -> Option<&pb::HostMeasured> {
        self.status.as_ref()?.measured.as_ref()
    }

    /// The CPU `need` takes here (thousandths): a `vm` Space's vCPUs share
    /// host CPUs by the host's VM overcommit.
    pub fn cpu_need(&self, need: &Need) -> u32 {
        match self.measured() {
            Some(m) if need.kind == "vm" && m.vcpu_overcommit > 1.0 => {
                (f64::from(need.millicpus) / f64::from(m.vcpu_overcommit)).ceil() as u32
            }
            _ => need.millicpus,
        }
    }

    /// The memory `need` takes here (MiB): for a `vm` Space on a host that
    /// measures its VMs, what a running VM there uses (copy-on-write
    /// snapshot memory shared), at most its guest RAM; else its guest RAM.
    pub fn memory_need(&self, need: &Need) -> u64 {
        match self.vm_measured(need) {
            Some(s) => crate::node::expected_vm_mb(need.memory_mb, s.vm_typical_mb),
            None => need.memory_mb,
        }
    }

    /// Memory free for `need` after what is pending: measured VM memory for
    /// a `vm` Space on a host that measures it, else unreserved memory.
    pub fn free_memory_for(&self, need: &Need) -> i64 {
        match self.vm_measured(need) {
            Some(s) => s.vm_memory_free_mb as i64 - self.pending_memory_mb as i64,
            None => self.free_memory_mb(),
        }
    }

    /// Why `need` does not fit here, if it does not.
    pub fn misfit(&self, need: &Need) -> Option<String> {
        self.misfit_with(need, true)
    }

    /// Whether `need` matches this host (kind, runtime, OS, name) but the
    /// host has no room for it now: a shortage of CPU, memory or job
    /// slots, as opposed to a host that can never take it.
    pub fn lacks_room(&self, need: &Need) -> bool {
        self.misfit_with(need, true).is_some() && self.misfit_with(need, false).is_none()
    }

    fn misfit_with(&self, need: &Need, check_room: bool) -> Option<String> {
        if let Some(p) = cloud_pin(&need.on) {
            if self.provider != p {
                return Some(format!("{} is not a {p} host", self.name));
            }
        } else if !need.on.is_empty() && need.on != self.name {
            return Some(format!("{} is not {}", self.name, need.on));
        }
        if self.pinned && need.on != self.name && cloud_pin(&need.on).is_none() {
            return Some(format!(
                "{} takes only work that names it (it does not provide Spaces)",
                self.name
            ));
        }
        if !self.ready {
            return Some(format!("{} is not ready", self.name));
        }
        if !need.host_os.is_empty() && !self.os.is_empty() && self.os != need.host_os {
            return Some(format!(
                "{} runs {}, not {}",
                self.name, self.os, need.host_os
            ));
        }
        if !need.arch.is_empty()
            && let Some(s) = &self.status
            && !s.arch.is_empty()
            && crate::model::normalize_arch(&s.arch) != crate::model::normalize_arch(&need.arch)
        {
            return Some(format!("{} is {}, not {}", self.name, s.arch, need.arch));
        }
        let Some(s) = &self.status else {
            // A host that does not report capacity: only plain requests.
            return (!need.kind.is_empty() && need.kind != "container" && need.kind != "vm")
                .then(|| format!("{} does not report what it creates", self.name));
        };
        if !s.problem.is_empty() {
            return Some(format!("{}: {}", self.name, s.problem));
        }
        if s.draining {
            return Some(format!("{} is draining", self.name));
        }
        if self.elastic && need.private_image {
            return Some(format!(
                "{} cannot pull an image from a registry on your own network",
                self.name
            ));
        }
        if need.kind == crate::model::KIND_PROCESS {
            return Some(crate::model::process_refused());
        }
        if need.kind == crate::model::KIND_MACHINE && !s.kinds.contains(&need.kind) {
            return Some(format!(
                "{} is not dedicated (only a machine created for this work, such as a `cua cloud add \
                 aws` host, offers its own Space)",
                self.name
            ));
        }
        if !need.kind.is_empty() && !s.kinds.contains(&need.kind) {
            return Some(format!(
                "{} does not create {} Spaces",
                self.name, need.kind
            ));
        }
        if need.services && !s.job_services {
            return Some(format!(
                "{} does not support job services (it predates HostJobSpec.services and would \
                 run the job without them); upgrade cua on that host",
                self.name
            ));
        }
        if !crate::model::accelerator_fits(&need.accelerator, &s.accelerators) {
            return Some(if s.accelerators.is_empty() {
                format!("{} has no {} accelerator", self.name, need.accelerator)
            } else {
                format!(
                    "{} has {}, not a {} accelerator",
                    self.name,
                    s.accelerators.join(", "),
                    need.accelerator
                )
            });
        }
        if !need.runtime.is_empty() && !s.runtimes.contains(&need.runtime) {
            return Some(format!("{} does not offer {}", self.name, need.runtime));
        }
        if !check_room {
            return None;
        }
        if s.jobs_max > 0 && s.jobs_running + self.pending_jobs >= s.jobs_max {
            return Some(format!("{} runs its most jobs", self.name));
        }
        if self.measured().is_some_and(|m| m.memory_pressure >= 2) {
            return Some(format!("{}'s memory pressure is critical", self.name));
        }
        if self.free_millicpus() < i64::from(self.cpu_need(need)) {
            return Some(format!("{} has not enough CPU free", self.name));
        }
        if self.free_memory_for(need) < self.memory_need(need) as i64 {
            return Some(match self.vm_measured(need) {
                Some(_) => format!(
                    "{} has not enough VM memory free ({} MiB; a VM there uses about {} MiB)",
                    self.name,
                    self.free_memory_for(need).max(0),
                    self.memory_need(need)
                ),
                None => format!("{} has not enough memory free", self.name),
            });
        }
        None
    }

    /// Whether a ready warm-pool member of `need`'s shape waits here.
    pub fn warm_for(&self, need: &Need) -> bool {
        !need.shape.is_empty()
            && self
                .status
                .as_ref()
                .and_then(|s| s.warm.get(&need.shape))
                .is_some_and(|n| *n > 0)
    }

    /// The share of the host left free after placing `need` (0..1).
    fn free_after(&self, need: &Need) -> f64 {
        let Some(s) = &self.status else {
            return 0.0;
        };
        let cpu = if s.millicpus_total == 0 {
            0.0
        } else {
            (self.free_millicpus() - i64::from(self.cpu_need(need))) as f64
                / f64::from(s.millicpus_total)
        };
        let mem = if s.memory_total_mb == 0 {
            0.0
        } else {
            (self.free_memory_for(need) - self.memory_need(need) as i64) as f64
                / s.memory_total_mb as f64
        };
        cpu.min(mem).clamp(0.0, 1.0)
    }

    /// Higher is better; `None` when `need` does not fit.
    pub fn score(&self, need: &Need, policy: Policy) -> Option<f64> {
        if self.misfit(need).is_some() {
            return None;
        }
        // A host that does not report goes after every host that does.
        if self.status.is_none() {
            return Some(-1.0);
        }
        Some(self.policy_score(self.free_after(need), need, policy))
    }

    /// The score of a host that fits, given the share of it left free after
    /// the placement (`free_after`, 0..1): the policy's term plus the terms
    /// every placement shares (memory pressure, a warm member, elastic
    /// capacity last). The one scoring model: [`Candidate::score`] and
    /// batch placement ([`crate::batch`]) both use it.
    pub fn policy_score(&self, free_after: f64, need: &Need, policy: Policy) -> f64 {
        let mut s = match policy {
            Policy::Spread => free_after,
            Policy::Pack => 1.0 - free_after,
        };
        // A host already compressing or swapping goes after the others.
        if self.measured().is_some_and(|m| m.memory_pressure == 1) {
            s -= 1.0;
        }
        // A ready warm-pool member of this shape: no cold start there.
        if self.warm_for(need) {
            s += 2.0;
        }
        // Other work keeps its CPUs busy: a soft tie-break among hosts that
        // fit, never a refusal.
        s -= BUSY_WEIGHT * self.busy.clamp(0.0, 1.0);
        // Elastic capacity goes after every host that fits.
        if self.elastic { s - ELASTIC_PENALTY } else { s }
    }
}

/// The warm-pool shape of a Space:
/// `<image>|<kind>|<runtime>|<cpus>|<memory_mb>`, as hosts report ready
/// members (`HostNodeStatus.warm`). An empty part is auto (kind, runtime)
/// or the default size (`cpus` and `memory_mb` 0).
pub fn warm_key(image: &str, kind: &str, runtime: &str, cpus: u32, memory_mb: u64) -> String {
    let w = |s: &str| crate::model::normalize_auto(s);
    let n = |v: u64| if v == 0 { String::new() } else { v.to_string() };
    format!(
        "{}|{}|{}|{}|{}",
        image.trim(),
        w(kind),
        w(runtime),
        n(u64::from(cpus)),
        n(memory_mb)
    )
}

/// Whether `image` names a registry only machines on your own network
/// reach: an IP address, `localhost`, or a host name without a dot.
pub fn image_is_private(image: &str) -> bool {
    let Some((first, _)) = image.split_once('/') else {
        return false; // Docker Hub
    };
    // As Docker reads a reference: the first part is a registry only when
    // it has a dot or a port, or is `localhost` (else it is a Docker Hub
    // namespace).
    if !(first.contains('.') || first.contains(':') || first == "localhost") {
        return false;
    }
    let host = first.split(':').next().unwrap_or(first);
    host == "localhost" || !host.contains('.') || host.parse::<std::net::IpAddr>().is_ok()
}

/// The hosts `need` fits on, best first (ties by name, so the order is
/// stable).
pub fn rank(cands: &[Candidate], need: &Need, policy: Policy) -> Vec<usize> {
    let mut scored: Vec<(usize, f64)> = cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.score(need, policy).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| cands[a.0].name.cmp(&cands[b.0].name))
    });
    scored.into_iter().map(|(i, _)| i).collect()
}

/// Why nothing fits, for people: each host's reason.
pub fn why_nothing_fits(cands: &[Candidate], need: &Need) -> String {
    if cands.is_empty() {
        return "the cluster has no hosts (add one with `cua cluster invite`)".into();
    }
    cands
        .iter()
        .filter_map(|c| c.misfit(need))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether some host matches `need` but has no room for it now: then
/// the refusal is a capacity one (try later, free something, add a
/// machine), not "nothing matches".
pub fn nothing_has_room(cands: &[Candidate], need: &Need) -> bool {
    cands.iter().any(|c| c.lacks_room(need))
}

/// Memory for people: `256 MiB` under a GiB, else GiB with one decimal at
/// most (`4 GiB`, `1.5 GiB`).
fn memory_of(mb: u64) -> String {
    if mb < 1024 {
        return format!("{mb} MiB");
    }
    let g = (mb as f64 / 1024.0 * 10.0).round() / 10.0;
    if g.fract() == 0.0 {
        format!("{g:.0} GiB")
    } else {
        format!("{g:.1} GiB")
    }
}

/// CPUs from thousandths, one decimal at most.
fn cpus_of(millis: u64) -> String {
    let c = (millis as f64 / 100.0).round() / 10.0;
    if c.fract() == 0.0 {
        format!("{c:.0}")
    } else {
        format!("{c:.1}")
    }
}

/// The capacity refusal for people: one sentence that says what is short on
/// each host that matches but has no room, and the fix. `local` is what
/// this machine's Spaces hold, for the host named `local_name`.
pub fn no_room_message(
    cands: &[Candidate],
    need: &Need,
    local_name: &str,
    local: crate::sysinfo::Reserved,
) -> String {
    let ask = format!(
        "a {}-CPU, {} Space",
        cpus_of(u64::from(need.millicpus)),
        memory_of(need.memory_mb)
    );
    let mut parts = Vec::new();
    for c in cands.iter().filter(|c| c.lacks_room(need)) {
        let Some(s) = &c.status else { continue };
        let mut why = Vec::new();
        if c.free_millicpus() < i64::from(need.millicpus) {
            if c.name == local_name {
                why.push(format!(
                    "{} of {} CPUs reserved by {} Space{}",
                    cpus_of(u64::from(local.millicpus)),
                    cpus_of(u64::from(local.total_millicpus)),
                    local.spaces,
                    if local.spaces == 1 { "" } else { "s" }
                ));
            } else {
                why.push(format!(
                    "{} of {} CPUs free",
                    cpus_of(c.free_millicpus().max(0) as u64),
                    cpus_of(u64::from(s.millicpus_total))
                ));
            }
        }
        if c.free_memory_mb() < need.memory_mb as i64 {
            why.push(format!(
                "{} of {} memory free",
                memory_of(c.free_memory_mb().max(0) as u64),
                memory_of(s.memory_total_mb)
            ));
        }
        if why.is_empty() {
            why.push(c.misfit(need).unwrap_or_default());
        }
        parts.push((c.name.clone(), why.join(" and ")));
    }
    let fix = "fix: delete a Space, ask for less, or add a machine (`cua host setup` on another computer)";
    match parts.as_slice() {
        [(name, why)] if name == local_name => {
            format!("this machine has no room for {ask} now ({why}). {fix}")
        }
        [(name, why)] => format!("{name} has no room for {ask} now ({why}). {fix}"),
        _ => {
            let each = parts
                .iter()
                .map(|(n, w)| format!("{n}: {w}"))
                .collect::<Vec<_>>()
                .join("; ");
            format!("no machine has room for {ask} now ({each}). {fix}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, cpu_free: u32, mem_free: u64) -> Candidate {
        Candidate {
            name: name.into(),
            ready: true,
            os: "linux".into(),
            status: Some(pb::HostNodeStatus {
                millicpus_total: 16000,
                millicpus_free: cpu_free,
                memory_total_mb: 32768,
                memory_free_mb: mem_free,
                kinds: vec!["container".into(), "vm".into()],
                runtimes: vec!["gvisor".into(), "runc".into(), "qemu".into()],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn need(cpu: u32, mem: u64) -> Need {
        Need {
            millicpus: cpu,
            memory_mb: mem,
            ..Default::default()
        }
    }

    #[test]
    fn jobs_that_ask_for_services_skip_hosts_that_would_drop_them() {
        let mut new = host("new", 16000, 32768);
        if let Some(s) = new.status.as_mut() {
            s.job_services = true;
        }
        let old = host("old", 16000, 32768); // does not report job_services
        let hosts = vec![new, old];
        let plain = need(1000, 1024);
        assert_eq!(rank(&hosts, &plain, Policy::Spread).len(), 2);
        let svc = Need {
            services: true,
            ..plain
        };
        let ranked = rank(&hosts, &svc, Policy::Spread);
        assert_eq!(ranked.len(), 1);
        assert_eq!(hosts[ranked[0]].name, "new");
        let why = hosts[1].misfit(&svc).unwrap();
        assert!(why.contains("does not support job services"), "{why}");
    }

    #[test]
    fn dedicated_hosts_and_accelerators_narrow_the_hosts() {
        let mut mac = host("mac", 16000, 32768);
        if let Some(s) = mac.status.as_mut() {
            s.accelerators = vec!["apple:M2 Max:38-core GPU:96 GB".into()];
        }
        let mut gpu = host("gpu", 16000, 32768);
        if let Some(s) = gpu.status.as_mut() {
            s.kinds.push("machine".into());
            s.dedicated = true;
            s.accelerators = vec!["nvidia:L4:23 GB".into()];
        }
        let plain = host("plain", 16000, 32768);
        let hosts = vec![mac, gpu, plain];
        let names = |n: &Need| -> Vec<&str> {
            let mut v: Vec<&str> = rank(&hosts, n, Policy::Spread)
                .into_iter()
                .map(|i| hosts[i].name.as_str())
                .collect();
            v.sort();
            v
        };
        // The removed native kind goes nowhere.
        let process = Need {
            kind: "process".into(),
            ..need(1000, 1024)
        };
        assert!(names(&process).is_empty());
        assert!(
            hosts[0]
                .misfit(&process)
                .unwrap()
                .contains("every job runs in a Space")
        );
        // A machine's own Space only where the machine is dedicated.
        let machine = Need {
            kind: "machine".into(),
            ..need(1000, 1024)
        };
        assert_eq!(names(&machine), ["gpu"]);
        assert!(hosts[0].misfit(&machine).unwrap().contains("not dedicated"));
        // Any kind (the host picks): every host, narrowed by accelerator.
        let auto = need(1000, 1024);
        assert_eq!(names(&auto), ["gpu", "mac", "plain"]);
        let any_gpu = Need {
            accelerator: "gpu".into(),
            ..auto.clone()
        };
        assert_eq!(names(&any_gpu), ["gpu", "mac"]);
        let apple = Need {
            accelerator: "apple".into(),
            ..auto.clone()
        };
        assert_eq!(names(&apple), ["mac"]);
        let nvidia = Need {
            accelerator: "nvidia".into(),
            ..auto
        };
        assert_eq!(names(&nvidia), ["gpu"]);
        assert!(hosts[0].misfit(&nvidia).unwrap().contains("apple:M2 Max"));
    }

    #[test]
    fn spread_prefers_the_least_loaded_and_pack_the_most() {
        let hosts = vec![
            host("busy", 2000, 4096),
            host("idle", 16000, 32768),
            host("half", 8000, 16384),
        ];
        let n = need(1000, 1024);
        let spread: Vec<&str> = rank(&hosts, &n, Policy::Spread)
            .into_iter()
            .map(|i| hosts[i].name.as_str())
            .collect();
        assert_eq!(spread, ["idle", "half", "busy"]);
        let pack: Vec<&str> = rank(&hosts, &n, Policy::Pack)
            .into_iter()
            .map(|i| hosts[i].name.as_str())
            .collect();
        assert_eq!(pack, ["busy", "half", "idle"]);
    }

    /// A host with a ready warm-pool member of the shape goes first, even
    /// when it is busier; one with a member of another shape does not.
    #[test]
    fn a_ready_warm_member_wins_over_free_room() {
        let shape = warm_key("ghcr.io/trycua/linux:24.04-slim", "auto", "", 0, 0);
        assert_eq!(shape, "ghcr.io/trycua/linux:24.04-slim||||");
        assert_eq!(warm_key("img", "vm", "qemu", 4, 8192), "img|vm|qemu|4|8192");
        let mut warm = host("warm", 4000, 8192);
        warm.status.as_mut().unwrap().warm = [(shape.clone(), 2)].into();
        let mut other = host("other-shape", 8000, 16384);
        other.status.as_mut().unwrap().warm = [("other|vm|||".to_string(), 3)].into();
        let idle = host("idle", 16000, 32768);
        let hosts = vec![idle, other, warm];
        let n = Need {
            shape,
            ..need(1000, 1024)
        };
        let order: Vec<&str> = rank(&hosts, &n, Policy::Spread)
            .into_iter()
            .map(|i| hosts[i].name.as_str())
            .collect();
        assert_eq!(order, ["warm", "idle", "other-shape"]);
        // Warm or not, a host where it does not fit is left out.
        let big = Need {
            memory_mb: 9000,
            ..n.clone()
        };
        assert!(!rank(&hosts, &big, Policy::Spread).contains(&2));
    }

    #[test]
    fn hosts_that_do_not_fit_are_left_out_with_a_reason() {
        let mut microvm = host("fc", 16000, 32768);
        microvm
            .status
            .as_mut()
            .unwrap()
            .kinds
            .push("microvm".into());
        let mut draining = host("drain", 16000, 32768);
        draining.status.as_mut().unwrap().draining = true;
        let mut down = host("down", 16000, 32768);
        down.ready = false;
        let mac = Candidate {
            os: "macos".into(),
            ..host("mac", 16000, 32768)
        };
        let full = host("full", 500, 32768);
        let hosts = vec![microvm, draining, down, mac, full];
        let n = Need {
            kind: "microvm".into(),
            ..need(1000, 1024)
        };
        let fit: Vec<&str> = rank(&hosts, &n, Policy::Spread)
            .into_iter()
            .map(|i| hosts[i].name.as_str())
            .collect();
        assert_eq!(fit, ["fc"]);
        let macos = Need {
            host_os: "macos".into(),
            ..need(1000, 1024)
        };
        assert_eq!(rank(&hosts, &macos, Policy::Spread), vec![3]);
        // Architecture: an arm64 job goes only to an aarch64 host.
        let mut arm = host("arm", 16000, 32768);
        arm.status.as_mut().unwrap().arch = "aarch64".into();
        let mut x86 = host("x86", 16000, 32768);
        x86.status.as_mut().unwrap().arch = "x86_64".into();
        let both = vec![arm, x86];
        let a = Need {
            arch: "arm64".into(),
            ..need(1000, 1024)
        };
        assert_eq!(rank(&both, &a, Policy::Spread), vec![0]);
        assert!(both[1].misfit(&a).unwrap().contains("x86_64, not arm64"));
        let why = why_nothing_fits(&hosts[1..], &n);
        assert!(why.contains("drain is draining"), "{why}");
        assert!(why.contains("down is not ready"), "{why}");
        assert!(why.contains("does not create microvm"), "{why}");
    }

    #[test]
    fn pending_dispatches_count_against_free_capacity() {
        let mut h = host("a", 2000, 32768);
        assert!(h.misfit(&need(1000, 512)).is_none());
        h.pending_millicpus = 1500;
        assert!(h.misfit(&need(1000, 512)).unwrap().contains("CPU"));
        h.pending_millicpus = 0;
        h.status.as_mut().unwrap().jobs_max = 3;
        h.status.as_mut().unwrap().jobs_running = 2;
        h.pending_jobs = 1;
        assert!(h.misfit(&need(1, 1)).unwrap().contains("most jobs"));
    }

    #[test]
    fn a_private_image_never_goes_to_elastic_capacity() {
        assert!(super::image_is_private(
            "172.31.39.35:5000/trycua/linux:24.04"
        ));
        assert!(super::image_is_private("localhost:5000/x:1"));
        assert!(super::image_is_private("myregistry:5000/x:1"));
        assert!(!super::image_is_private("someuser/x:1"));
        assert!(!super::image_is_private("ghcr.io/trycua/linux:24.04-slim"));
        assert!(!super::image_is_private("ubuntu:24.04"));
        let mut modal = host("modal", 64_000, 1 << 20);
        modal.elastic = true;
        let mut need = need(1000, 1024);
        need.private_image = true;
        assert!(modal.misfit(&need).unwrap().contains("your own network"));
        need.private_image = false;
        assert!(modal.misfit(&need).is_none());
    }

    #[test]
    fn elastic_capacity_comes_after_hosts_unless_named() {
        let mut modal = host("modal", 64000, 1 << 20);
        modal.elastic = true;
        let hosts = vec![modal, host("mini", 1000, 2048)];
        let n = need(1000, 1024);
        assert_eq!(rank(&hosts, &n, Policy::Spread), vec![1, 0]);
        // The host is full: the job goes to the provider.
        let full = need(2000, 1024);
        assert_eq!(rank(&hosts, &full, Policy::Spread), vec![0]);
        // Asked for by name: only it.
        let named = Need {
            on: "modal".into(),
            ..need(1000, 1024)
        };
        assert_eq!(rank(&hosts, &named, Policy::Spread), vec![0]);
        assert!(hosts[1].misfit(&named).unwrap().contains("is not modal"));
    }

    /// `cloud:aws` goes to hosts that cloud supplied, and to no other host.
    #[test]
    fn a_cloud_pin_matches_only_that_clouds_hosts() {
        let mut aws = host("aws-1", 8000, 8192);
        aws.provider = "aws".into();
        let mut hz = host("hetzner-1", 8000, 8192);
        hz.provider = "hetzner".into();
        let mut local = host("mini", 8000, 8192);
        local.pinned = true;
        let hosts = vec![aws, hz, local];
        let n = Need {
            on: "cloud:aws".into(),
            ..need(1000, 1024)
        };
        assert_eq!(rank(&hosts, &n, Policy::Spread), vec![0]);
        assert!(hosts[1].misfit(&n).unwrap().contains("not a aws host"));
        assert!(hosts[2].misfit(&n).is_some());
        // Not a pin: `cloud:` alone names a host called that.
        assert_eq!(cloud_pin("cloud:"), None);
        assert_eq!(cloud_pin("aws-1"), None);
    }

    /// A `vm` Space on a host that measures its VMs: its vCPUs share host
    /// CPUs by the host's overcommit, and it needs what a VM there uses
    /// (copy-on-write forks share their snapshot) out of the measured VM
    /// memory, not its whole guest RAM out of the reservations.
    #[test]
    fn vm_spaces_use_measured_vm_memory_and_vcpu_overcommit() {
        let vm = |cpu: u32, mem: u64| Need {
            kind: "vm".into(),
            ..need(cpu, mem)
        };
        // 12 CPUs, reservations exhausted, but 20 GiB of VM memory free and
        // VMs of ~300 MiB each.
        let mut mac = host("mac", 1000, 0);
        {
            mac.status.as_mut().unwrap().measured = Some(pb::HostMeasured {
                vcpu_overcommit: 8.0,
                vm_memory_known: true,
                vm_memory_free_mb: 20 * 1024,
                vm_typical_mb: 300,
                ..Default::default()
            });
        }
        assert_eq!(mac.cpu_need(&vm(2000, 4096)), 250);
        assert_eq!(mac.memory_need(&vm(2000, 4096)), 300);
        assert!(mac.misfit(&vm(2000, 4096)).is_none());
        // A container still needs its reservation.
        assert!(
            mac.misfit(&need(500, 1024))
                .unwrap()
                .contains("memory free")
        );
        // Pending VMs count against the measured memory.
        mac.pending_memory_mb = 20 * 1024 - 200;
        let why = mac.misfit(&vm(1000, 1024)).unwrap();
        assert!(why.contains("not enough VM memory free (200 MiB"), "{why}");
        mac.pending_memory_mb = 0;
        // No measurement: the whole guest RAM, unscaled CPU.
        let plain = host("plain", 1000, 0);
        assert_eq!(plain.cpu_need(&vm(2000, 4096)), 2000);
        assert!(plain.misfit(&vm(2000, 4096)).is_some());
        // Pressure: critical refuses, warn ranks last.
        let mut hot = host("hot", 16000, 32768);
        hot.status.as_mut().unwrap().measured = Some(pb::HostMeasured {
            memory_pressure: 2,
            ..Default::default()
        });
        assert!(hot.misfit(&need(1000, 1024)).unwrap().contains("critical"));
        let mut warm = host("warm", 16000, 32768);
        warm.status.as_mut().unwrap().measured = Some(pb::HostMeasured {
            memory_pressure: 1,
            ..Default::default()
        });
        let hosts = vec![warm, host("calm", 8000, 16384)];
        assert_eq!(rank(&hosts, &need(1000, 1024), Policy::Spread), vec![1, 0]);
    }

    #[test]
    fn a_host_without_a_report_goes_last() {
        let hosts = vec![
            Candidate {
                name: "old".into(),
                ready: true,
                ..Default::default()
            },
            host("new", 1000, 1024),
        ];
        assert_eq!(rank(&hosts, &need(500, 512), Policy::Spread), vec![1, 0]);
        let mv = Need {
            kind: "microvm".into(),
            ..need(1, 1)
        };
        assert!(hosts[0].misfit(&mv).is_some());
    }

    #[test]
    fn load_only_lowers_the_score_and_never_misfits() {
        let mut busy = host("busy", 16000, 32768);
        busy.busy = 1.0;
        let idle = host("idle", 16000, 32768);
        let n = need(2000, 4096);
        // Fully loaded, still fits; ranks after an idle twin.
        assert!(busy.misfit(&n).is_none());
        let hosts = vec![busy.clone(), idle];
        assert_eq!(rank(&hosts, &n, Policy::Spread), vec![1, 0]);
        // Alone, it is the only choice.
        assert_eq!(rank(&[busy.clone()], &n, Policy::Spread), vec![0]);
        // Load does not outweigh a real difference in room.
        let roomy = vec![busy, host("tight", 4000, 32768)];
        assert_eq!(rank(&roomy, &n, Policy::Spread), vec![0, 1]);
    }

    #[test]
    fn no_room_is_told_apart_from_no_match() {
        let hosts = vec![host("this machine", 1000, 32768)];
        let n = need(2000, 4096);
        assert!(hosts[0].lacks_room(&n));
        assert!(nothing_has_room(&hosts, &n));
        // A kind the host never creates is a mismatch, not capacity.
        let mv = Need {
            kind: "microvm".into(),
            ..need(1000, 1024)
        };
        assert!(!nothing_has_room(&hosts, &mv));
        // A different host name is a mismatch too.
        let named = Need {
            on: "elsewhere".into(),
            ..n.clone()
        };
        assert!(!nothing_has_room(&hosts, &named));
        // A host that is not ready is not "no room".
        let mut down = host("down", 1000, 1024);
        down.ready = false;
        assert!(!nothing_has_room(&[down], &n));
        // Fits: neither.
        assert!(!nothing_has_room(&[host("big", 16000, 32768)], &n));
    }

    #[test]
    fn the_capacity_message_names_what_is_short_and_the_fix() {
        let local = crate::sysinfo::Reserved {
            millicpus: 6000,
            memory_mb: 20480,
            spaces: 5,
            total_millicpus: 16000,
        };
        let hosts = vec![host("this machine", 10000, 1024)];
        let m = no_room_message(&hosts, &need(2000, 4096), "this machine", local);
        assert_eq!(
            m,
            "this machine has no room for a 2-CPU, 4 GiB Space now (1 GiB of 32 GiB memory free). \
             fix: delete a Space, ask for less, or add a machine (`cua host setup` on another computer)"
        );
        let hosts = vec![host("this machine", 1500, 32768)];
        let m = no_room_message(&hosts, &need(2000, 4096), "this machine", local);
        assert!(m.starts_with(
            "this machine has no room for a 2-CPU, 4 GiB Space now (6 of 16 CPUs reserved by 5 Spaces). fix:"
        ));
        let hosts = vec![host("this machine", 1500, 32768), host("box", 0, 32768)];
        let m = no_room_message(&hosts, &need(2000, 4096), "this machine", local);
        assert!(m.starts_with("no machine has room for a 2-CPU, 4 GiB Space now (this machine: "));
        assert!(m.contains("box: 0 of 16 CPUs free"), "{m}");
    }
}
