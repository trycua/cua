//! Fixed scenarios: the benchmark's workloads as solver inputs. The tests
//! pin the solver's decision for each (golden files), and `cua cloud plan
//! --scenario` style tooling and docs can show them.

use super::*;

/// Hetzner offers (fsn1, list prices incl. IPv4, 2026-10-21).
pub fn hetzner_offers() -> Vec<Offer> {
    let t = |name: &str, cpus: u64, gb: u64, usd: f64, arch: &str, dedicated: bool| Offer {
        provider: "hetzner".into(),
        type_name: name.into(),
        region: "fsn1".into(),
        arch: arch.into(),
        capacity: host_capacity(
            cpus,
            gb,
            if dedicated {
                80 + cpus * 20
            } else {
                40 + cpus * 20
            },
        ),
        kvm: false,
        kinds: vec!["container".into()],
        gpu: String::new(),
        usd_per_hour: usd,
        billing_step_secs: 3600,
        billing_min_secs: 3600,
        boot_secs: 50,
        one_space_each: false,
        max_new: 10,
        rtt_ms: 0,
        vcpus: cpus as u32,
        quota: if dedicated {
            "hetzner/dedicated"
        } else {
            "hetzner/shared"
        }
        .into(),
    };
    vec![
        t("cx23", 2, 4, 0.0114, "amd64", false),
        t("cx33", 4, 8, 0.0194, "amd64", false),
        t("cx43", 8, 16, 0.0354, "amd64", false),
        t("cx53", 16, 32, 0.0674, "amd64", false),
        t("cax11", 2, 4, 0.0122, "arm64", false),
        t("cax21", 4, 8, 0.0194, "arm64", false),
        t("cax31", 8, 16, 0.0354, "arm64", false),
        t("cax41", 16, 32, 0.0684, "arm64", false),
        t("ccx33", 8, 32, 0.1074, "amd64", true),
    ]
}

/// AWS offers (us-west-2 on-demand Linux, 2026-10-21).
pub fn aws_offers() -> Vec<Offer> {
    let t = |name: &str, cpus: u64, gb: u64, usd: f64, arch: &str, kvm: bool| Offer {
        provider: "aws".into(),
        type_name: name.into(),
        region: "us-west-2".into(),
        arch: arch.into(),
        capacity: host_capacity(cpus, gb, 60 + cpus * 10),
        kvm,
        kinds: if kvm {
            vec!["container".into(), "vm".into()]
        } else {
            vec!["container".into()]
        },
        gpu: String::new(),
        usd_per_hour: usd,
        billing_step_secs: 1,
        billing_min_secs: 60,
        boot_secs: if kvm { 300 } else { 80 },
        one_space_each: false,
        max_new: 20,
        rtt_ms: 0,
        vcpus: cpus as u32,
        quota: "aws/standard".into(),
    };
    vec![
        t("c7g.large", 2, 4, 0.0725, "arm64", false),
        t("c7g.xlarge", 4, 8, 0.145, "arm64", false),
        t("c7g.2xlarge", 8, 16, 0.29, "arm64", false),
        t("c7g.4xlarge", 16, 32, 0.58, "arm64", false),
        t("m7g.xlarge", 4, 16, 0.1632, "arm64", false),
        t("c6g.metal", 64, 128, 2.176, "arm64", true),
    ]
}

/// Modal (per Space, us-east; $0.0000131/core-s with 2 vCPU per core,
/// $0.00000222/GiB-s, modal.com/pricing 2026-10-21).
pub fn modal_offers() -> Vec<Offer> {
    let t = |name: &str, cpus: u64, gb: u64| Offer {
        provider: "modal".into(),
        type_name: name.into(),
        region: "us-east".into(),
        arch: "amd64".into(),
        capacity: Shape::new(cpus * 1000, gb * 1024, 64),
        kvm: false,
        kinds: vec!["container".into()],
        gpu: String::new(),
        usd_per_hour: (cpus as f64 / 2.0) * 0.0000131 * 3600.0 + gb as f64 * 0.00000222 * 3600.0,
        billing_step_secs: 1,
        billing_min_secs: 0,
        boot_secs: 3,
        one_space_each: true,
        max_new: 100,
        rtt_ms: 0,
        vcpus: cpus as u32,
        quota: String::new(),
    };
    vec![t("1x1", 1, 1), t("2x4", 2, 4), t("4x8", 4, 8)]
}

/// What a host of `cpus` vCPUs and `gb` GiB accepts: vCPUs overcommitted
/// 4x for mostly-idle Spaces, 1 GiB kept for the host itself.
pub fn host_capacity(cpus: u64, gb: u64, disk_gb: u64) -> Shape {
    Shape::new(cpus * 1000 * 4, gb * 1024 - 1024, disk_gb)
}

fn studio(name: &str, free_cpus: u64, free_gb: u64) -> Host {
    Host {
        name: name.into(),
        free: Shape::new(free_cpus * 1000, free_gb * 1024, 300),
        total: Shape::new(48_000, 96 * 1024, 900),
        arch: "arm64".into(),
        os: "macos".into(),
        kinds: vec!["container".into(), "vm".into(), "microvm".into()],
        kvm: true,
        ready: true,
        ..Default::default()
    }
}

fn g(id: &str, n: u32, cpu: u64, mem: u64, dur: u64, lat: Latency) -> DemandGroup {
    DemandGroup {
        id: id.into(),
        count: n,
        per: Shape::new(cpu, mem, 1),
        duration_secs: dur,
        latency: lat,
        req: Requirements {
            image: "ghcr.io/trycua/linux:24.04-slim".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// One scenario: demand, machines, offers and limits.
pub type Scenario = (Vec<DemandGroup>, Vec<Host>, Vec<Offer>, Constraints);

/// Every scenario: name → (groups, hosts, offers, constraints).
pub fn all() -> Vec<(&'static str, Scenario)> {
    let clouds = || {
        let mut v = hetzner_offers();
        v.extend(aws_offers());
        v.extend(modal_offers());
        v
    };
    let base = Constraints {
        time_budget_ms: 2_000,
        ..Default::default()
    };
    // (b) 256-rollout burst: 32 worlds x 8 siblings, 1 vCPU / 1 GiB booked,
    // 30 minutes, one Mac with room for 40.
    let rl = (0..32)
        .map(|w| DemandGroup {
            whole: true,
            ..g(
                &format!("world-{w:02}"),
                8,
                1000,
                1024,
                1800,
                Latency::Batch,
            )
        })
        .collect();
    // (a) E2B quickstart from a laptop whose Mac is busy.
    let quick = vec![g(
        "e2b-quickstart",
        1,
        2000,
        2048,
        300,
        Latency::Interactive,
    )];
    // (c) An always-on agent (30 days).
    let agent = vec![DemandGroup {
        latency: Latency::Steady,
        ..g("agent", 1, 2000, 4096, 30 * 24 * 3600, Latency::Steady)
    }];
    // (d) Mixed: two Macs and a Linux box, 100 batch Spaces plus 3 VMs.
    let mut mixed = vec![g("eval", 100, 1000, 1024, 3600, Latency::Batch)];
    let mut vm = g("windows", 3, 4000, 8192, 3600, Latency::Interactive);
    vm.req.kind = "vm".into();
    vm.req.needs_kvm = true;
    vm.req.image = "windows".into();
    mixed.push(vm);
    let linux_box = Host {
        name: "linux-box".into(),
        free: Shape::new(16_000, 24 * 1024, 200),
        total: Shape::new(32_000, 32 * 1024, 400),
        arch: "amd64".into(),
        os: "linux".into(),
        kinds: vec!["container".into(), "vm".into()],
        kvm: true,
        ready: true,
        ..Default::default()
    };
    vec![
        (
            "rl-burst-256",
            (rl, vec![studio("studio", 40, 48)], clouds(), base.clone()),
        ),
        (
            "e2b-quickstart-busy-mac",
            (
                quick.clone(),
                vec![studio("studio", 0, 2)],
                clouds(),
                base.clone(),
            ),
        ),
        (
            "e2b-quickstart-free-mac",
            (quick, vec![studio("studio", 8, 32)], clouds(), base.clone()),
        ),
        ("always-on-agent", (agent, vec![], clouds(), base.clone())),
        (
            "mixed-devices-cloud",
            (
                mixed,
                vec![
                    studio("studio-a", 8, 24),
                    studio("studio-b", 4, 12),
                    linux_box,
                ],
                clouds(),
                base.clone(),
            ),
        ),
        (
            "burst-under-budget",
            (
                vec![g("burst", 200, 1000, 1024, 3600, Latency::Batch)],
                vec![],
                clouds(),
                Constraints {
                    max_usd_per_hour: Some(0.15),
                    ..base.clone()
                },
            ),
        ),
        (
            "pinned-aws-type",
            (
                vec![g("pinned", 20, 1000, 1024, 1800, Latency::Batch)],
                vec![],
                clouds(),
                Constraints {
                    pins: Pins {
                        provider: Some("aws".into()),
                        type_name: Some("c7g.xlarge".into()),
                        ..Default::default()
                    },
                    ..base
                },
            ),
        ),
    ]
}
