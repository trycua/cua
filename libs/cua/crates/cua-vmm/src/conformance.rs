//! Runtime conformance: what every runtime owes a Space, written once.
//!
//! A Space behaves the same on every runtime and kind (gVisor and runc in
//! the built-in Linux runtime or on a Linux engine, Apple's Containerization
//! and Cua's own microVM monitor on a Mac, Firecracker, QEMU, Lume, and the
//! machines of a pool): the same network policy, DNS and egress, registry
//! credentials, child Spaces from a home, downloads from the guest, and
//! readiness. This module is the registry of those promises:
//!
//! * [`Case`] lists them. The same list runs against every [`Entry`]: the
//!   hermetic tests run it on the backends that have a fake (Apple's
//!   `container` has one), and `libs/cua/scripts/conformance/run.py` runs
//!   it live on the runtimes this computer has (opt in; it cleans up what it
//!   makes), writing the matrix.
//! * [`capabilities`] is what a backend says about itself, and nothing
//!   else: the guests and architectures it runs, whether it can pause,
//!   checkpoint and fork, whether it needs KVM or nested virtualization,
//!   and what it does with the secure default network. What a backend does
//!   not offer it refuses at once with a plain reason ([`Expect::Refused`]),
//!   never late. The network answer is derived from
//!   [`crate::network::network_support`], so the two cannot disagree.
//! * [`entries`] is the roster of runtimes under test. A drift guard
//!   (`every_backend_has_a_conformance_entry`) fails the build when a
//!   backend is added without one, and [`entries_json`] is what the live
//!   driver reads (a test keeps the committed file in step).
//!
//! The shared layers that make the cases pass the same way everywhere are
//! not here: the secure-default policy compiler and its enforcement points
//! ([`crate::network`], the container engine's nftables),
//! the registry credential chain and layer cache (`cua-image`, reached
//! through [`crate::RootfsResolver`]), the child-Space channel the host
//! opens into the guest's `cua-spacesd` (`cua-daemon`'s children bridge: it
//! dials the guest, so it needs nothing from a runtime but an address), and
//! the readiness probes ([`crate::probe`]). A runtime supplies only how the
//! guest is reached.

use serde_json::{Value, json};

use crate::network::{self, NetworkSupport, Placement};
use crate::types::BackendKind;

/// A promise every runtime keeps, or refuses up front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Case {
    /// The secure default network holds: the internet answers; the host,
    /// the local network, cloud metadata and other trees' Spaces do not.
    Policy,
    /// A Space resolves names: a public name resolves and an install-style
    /// https download works, on the secure network and on the default one;
    /// in a Space whose network is `none` the lookup fails at once.
    Dns,
    /// The guest downloads a large package from the internet (the coding
    /// app's package).
    Download,
    /// A private image is pulled with the registry credential the daemon
    /// holds, and no credential is written outside the Cua home.
    Registry,
    /// A home creates, lists, uses and deletes a child Space through the
    /// daemon's authenticated channel, within its budget, and it is
    /// audited.
    Children,
    /// A coding agent session starts in the guest (the app installs, the
    /// scripted model answers).
    Agent,
    /// Time from the create call to a Space that answers is measured.
    Ready,
    /// Stop, resume, and what the backend declares (pause, checkpoint,
    /// fork) work; what it does not declare is refused in plain words.
    Lifecycle,
    /// Deleting leaves nothing: no Space, no process, no container, no
    /// network.
    Teardown,
    /// A warm claim: a Space made from a member the daemon kept ready
    /// answers with its own token, nothing else opens it (not the member's
    /// pool secret), and it behaves as a cold one; a runtime without warm
    /// claims says so and starts cold.
    Warm,
}

impl Case {
    /// Every case, in the order the live run takes them.
    pub const ALL: [Case; 10] = [
        Case::Ready,
        Case::Policy,
        Case::Dns,
        Case::Download,
        Case::Registry,
        Case::Children,
        Case::Agent,
        Case::Lifecycle,
        Case::Warm,
        Case::Teardown,
    ];

    /// The word the matrix and the driver use.
    pub fn as_str(self) -> &'static str {
        match self {
            Case::Policy => "policy",
            Case::Dns => "dns",
            Case::Download => "download",
            Case::Registry => "registry",
            Case::Children => "children",
            Case::Agent => "agent",
            Case::Ready => "ready",
            Case::Lifecycle => "lifecycle",
            Case::Teardown => "teardown",
            Case::Warm => "warm",
        }
    }

    /// What it checks, in a sentence.
    pub fn what(self) -> &'static str {
        match self {
            Case::Policy => {
                "the default network holds: internet yes; host, LAN, cloud metadata, other trees no, and each refusal is in the network log with its destination and time"
            }
            Case::Dns => {
                "a Space resolves names and downloads over https, on the secure and the default network; with no network the lookup fails at once"
            }
            Case::Download => "the guest downloads the coding app's package",
            Case::Registry => "a private image is pulled with the host-held registry credential",
            Case::Children => "a home creates, lists, uses and deletes a child Space, audited",
            Case::Agent => "a coding agent session starts and answers",
            Case::Ready => "time from create to a Space that answers",
            Case::Lifecycle => "stop and resume, and the declared pause, checkpoint and fork",
            Case::Teardown => "delete leaves no Space, process, container or network",
            Case::Warm => {
                "a Space claimed from a warm member answers with its own token and only that one, or the runtime says it starts cold"
            }
        }
    }
}

/// What a runtime does with a [`Case`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// It passes.
    Pass,
    /// It is refused at once, saying this (plain words, what to do
    /// instead). A refusal that comes late, or says something else, fails
    /// the case.
    Refused(&'static str),
}

impl Expect {
    /// `pass` or `refused`.
    pub fn word(self) -> &'static str {
        match self {
            Expect::Pass => "pass",
            Expect::Refused(_) => "refused",
        }
    }
}

/// What a backend declares about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Guest operating systems it runs.
    pub guests: &'static [&'static str],
    /// Guest architectures it runs natively on an Apple silicon or
    /// arm64 host, and (after a slash) on an x86-64 host's own.
    pub archs: &'static [&'static str],
    /// A running guest can be paused in memory.
    pub pause: bool,
    /// A running guest can be checkpointed (memory and disk).
    pub checkpoint: bool,
    /// A checkpoint can be forked into new Spaces.
    pub fork: bool,
    /// It needs a usable `/dev/kvm`.
    pub kvm: bool,
    /// It runs where the host is itself a VM (nested virtualization
    /// needed): only a backend that boots a guest kernel and cannot fall
    /// back says `true`.
    pub nested_virt: bool,
    /// Guests the host cannot reach by address need the runtime's own
    /// forward (`true`), else the guest is dialed at its own address.
    pub forwarded: bool,
    /// A member kept ready can be claimed by renaming it to a Space and
    /// giving it that Space's token ([`crate::Runtime::claim`]). Without it
    /// a pool is refused and Spaces start cold.
    pub claim: bool,
    /// A member answers calls made with its pool secret before it is
    /// claimed (a container does), so its coding apps can be installed
    /// ahead (the `warm-pools` experiment). Without it they go in after the
    /// claim, as without a pool.
    pub prepare: bool,
}

/// What `backend` declares. No wildcard: a new backend does not build
/// until it says.
pub fn capabilities(backend: BackendKind) -> Capabilities {
    match backend {
        // gVisor or runc in a container engine: pause with the engine,
        // checkpoint and fork as gVisor and the engine allow.
        BackendKind::Container | BackendKind::Managed => Capabilities {
            guests: &["linux"],
            archs: &["aarch64", "x86_64"],
            pause: true,
            checkpoint: true,
            fork: true,
            kvm: false,
            nested_virt: false,
            forwarded: true,
            claim: true,
            prepare: true,
        },
        BackendKind::Qemu => Capabilities {
            guests: &["linux", "windows", "android", "macos"],
            archs: &["aarch64", "x86_64"],
            pause: true,
            checkpoint: true,
            fork: true,
            kvm: false,
            nested_virt: false,
            forwarded: true,
            claim: true,
            prepare: false,
        },
        BackendKind::QemuDocker => Capabilities {
            guests: &[],
            archs: &[],
            pause: false,
            checkpoint: false,
            fork: false,
            kvm: false,
            nested_virt: false,
            forwarded: true,
            claim: false,
            prepare: false,
        },
        BackendKind::Lume => Capabilities {
            guests: &["macos"],
            archs: &["aarch64"],
            pause: false,
            checkpoint: false,
            fork: true,
            kvm: false,
            nested_virt: false,
            forwarded: false,
            claim: true,
            prepare: false,
        },
        BackendKind::Firecracker => Capabilities {
            guests: &["linux"],
            archs: &["aarch64", "x86_64"],
            pause: true,
            checkpoint: true,
            fork: true,
            kvm: true,
            nested_virt: true,
            forwarded: false,
            claim: true,
            prepare: false,
        },
        // Apple's runtime helper owns the VM: no pause, no snapshots.
        BackendKind::Containerization => Capabilities {
            guests: &["linux"],
            archs: &["aarch64", "x86_64"],
            pause: false,
            checkpoint: false,
            fork: false,
            kvm: false,
            nested_virt: false,
            forwarded: false,
            claim: true,
            prepare: false,
        },
        BackendKind::Vz => Capabilities {
            guests: &["linux"],
            archs: &["aarch64"],
            pause: true,
            checkpoint: true,
            fork: true,
            kvm: false,
            nested_virt: false,
            forwarded: false,
            claim: true,
            prepare: false,
        },
    }
}

/// One runtime under test: the words that select it (`--kind`,
/// `--runtime`), the backend that runs it, the hosts it runs on, and what a
/// live run needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The row of the matrix.
    pub id: &'static str,
    /// The backend that runs it.
    pub backend: BackendKind,
    /// `--kind` (`container`, `vm`, `microvm`).
    pub kind: &'static str,
    /// `--runtime` (`gvisor`, `runc`, `vz`, `containerization`, ...).
    pub runtime: &'static str,
    /// Host operating systems it runs on (`linux`, `macos`, `windows`).
    pub hosts: &'static [&'static str],
    /// The guest the cases run in.
    pub guest: &'static str,
    /// What must be installed for a live run, in a sentence.
    pub needs: &'static str,
    /// The cases a live run takes (a macOS guest has no coding agent).
    pub cases: &'static [Case],
    /// Why a live run is impossible, when it is (no runtime of this build
    /// starts it): the entry is kept so a new backend cannot hide.
    pub not_runnable: Option<&'static str>,
}

const LINUX_CASES: &[Case] = &Case::ALL;
const VM_CASES: &[Case] = &[
    Case::Ready,
    Case::Policy,
    Case::Dns,
    Case::Download,
    Case::Lifecycle,
    Case::Warm,
    Case::Teardown,
];
/// A macOS VM is claimed only from a pool kept warm on purpose, and a Mac
/// runs two of them: its warm start is covered by the Lume tests, not by a
/// third guest in the live run.
const MACOS_CASES: &[Case] = &[
    Case::Ready,
    Case::Policy,
    Case::Dns,
    Case::Download,
    Case::Lifecycle,
    Case::Teardown,
];

/// The roster: every backend at least once.
pub fn entries() -> &'static [Entry] {
    const E: &[Entry] = &[
        Entry {
            id: "builtin-gvisor",
            backend: BackendKind::Managed,
            kind: "container",
            runtime: "gvisor",
            hosts: &["macos", "windows"],
            guest: "linux",
            needs: "the built-in Linux runtime (downloaded on first use)",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "builtin-runc",
            backend: BackendKind::Managed,
            kind: "container",
            runtime: "runc",
            hosts: &["macos", "windows"],
            guest: "linux",
            needs: "the built-in Linux runtime (downloaded on first use)",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "engine-gvisor",
            backend: BackendKind::Container,
            kind: "container",
            runtime: "gvisor",
            hosts: &["linux"],
            guest: "linux",
            needs: "Docker or containerd with gVisor (runsc) and nftables",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "engine-runc",
            backend: BackendKind::Container,
            kind: "container",
            runtime: "runc",
            hosts: &["linux"],
            guest: "linux",
            needs: "Docker or containerd and nftables",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "firecracker-microvm",
            backend: BackendKind::Firecracker,
            kind: "microvm",
            runtime: "firecracker",
            hosts: &["linux"],
            guest: "linux",
            needs: "Linux with /dev/kvm and Firecracker",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "vz-microvm",
            backend: BackendKind::Vz,
            kind: "microvm",
            runtime: "vz",
            hosts: &["macos"],
            guest: "linux",
            needs: "an Apple silicon Mac with cua-vz and cua-microvm-init next to `cua`",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "containerization-microvm",
            backend: BackendKind::Containerization,
            kind: "microvm",
            runtime: "containerization",
            hosts: &["macos"],
            guest: "linux",
            needs: "macOS 26 on Apple silicon (Cua installs Apple's container itself)",
            cases: LINUX_CASES,
            not_runnable: None,
        },
        Entry {
            id: "qemu-vm",
            backend: BackendKind::Qemu,
            kind: "vm",
            runtime: "qemu",
            hosts: &["linux", "macos", "windows"],
            guest: "linux",
            needs: "qemu-system and a Linux VM image (a containerDisk)",
            cases: VM_CASES,
            not_runnable: None,
        },
        Entry {
            id: "lume-macos-vm",
            backend: BackendKind::Lume,
            kind: "vm",
            runtime: "lume",
            hosts: &["macos"],
            guest: "macos",
            needs: "an Apple silicon Mac with Lume and a macOS image",
            cases: MACOS_CASES,
            not_runnable: None,
        },
        Entry {
            id: "qemu-docker",
            backend: BackendKind::QemuDocker,
            kind: "vm",
            runtime: "",
            hosts: &[],
            guest: "linux",
            needs: "nothing: no runtime of this build starts QEMU inside a container",
            cases: &[],
            not_runnable: Some("no runtime of this build starts QEMU inside a container"),
        },
    ];
    E
}

/// The entry with this id.
pub fn entry(id: &str) -> Option<&'static Entry> {
    entries().iter().find(|e| e.id == id)
}

/// What the secure default network does with `backend` on host `os`, as a
/// [`Case::Policy`] expectation: [`Expect::Pass`] when the host enforces the
/// rules, else the reason it is refused (or runs without network).
pub fn policy_expectation(backend: BackendKind, os: &str) -> Expect {
    match network::network_support(backend, os, Placement::Local) {
        NetworkSupport::Enforced { .. } => Expect::Pass,
        NetworkSupport::NoNetworkOnly { why } | NetworkSupport::Refuse { why } => {
            Expect::Refused(why)
        }
    }
}

/// What `entry` is expected to do with `case` on host `os`.
pub fn expect(entry: &Entry, case: Case, os: &str) -> Expect {
    if !entry.cases.contains(&case) {
        return Expect::Refused("not applicable to this guest");
    }
    match case {
        Case::Policy => policy_expectation(entry.backend, os),
        Case::Warm if !capabilities(entry.backend).claim => Expect::Refused(
            "this runtime has no warm claims: a pool is refused and its Spaces start cold",
        ),
        _ => Expect::Pass,
    }
}

/// The roster as JSON for the live driver (`scripts/conformance/run.py`),
/// with what each entry expects on each host OS. A test keeps
/// `scripts/conformance/entries.json` equal to this.
pub fn entries_json() -> Value {
    let rows: Vec<Value> = entries()
        .iter()
        .map(|e| {
            let c = capabilities(e.backend);
            let hosts: Vec<Value> = e
                .hosts
                .iter()
                .map(|os| {
                    let cases: Vec<Value> = Case::ALL
                        .iter()
                        .map(|case| {
                            let x = expect(e, *case, os);
                            json!({
                                "case": case.as_str(),
                                "expect": x.word(),
                                "why": match x {
                                    Expect::Refused(w) => Value::from(w),
                                    Expect::Pass => Value::Null,
                                },
                            })
                        })
                        .collect();
                    json!({ "os": os, "cases": cases })
                })
                .collect();
            json!({
                "id": e.id,
                "backend": e.backend.as_str(),
                "kind": e.kind,
                "runtime": e.runtime,
                "guest": e.guest,
                "needs": e.needs,
                "not_runnable": e.not_runnable,
                "capabilities": {
                    "guests": c.guests,
                    "archs": c.archs,
                    "pause": c.pause,
                    "checkpoint": c.checkpoint,
                    "fork": c.fork,
                    "kvm": c.kvm,
                    "nested_virt": c.nested_virt,
                    "claim": c.claim,
                    "prepare": c.prepare,
                },
                "hosts": hosts,
            })
        })
        .collect();
    json!({ "cases": Case::ALL.iter().map(|c| json!({"case": c.as_str(), "what": c.what()})).collect::<Vec<_>>(), "entries": rows })
}

/// The matrix of what each runtime promises, as a Markdown table (for the
/// docs; a test keeps them in step).
pub fn promises_table_markdown() -> String {
    let mut out = String::from(
        "| Runtime | Guest | Default network | Pause | Checkpoint and fork | KVM | Warm start |\n|---|---|---|---|---|---|---|\n",
    );
    for e in entries().iter().filter(|e| e.not_runnable.is_none()) {
        let c = capabilities(e.backend);
        let net: Vec<String> = e
            .hosts
            .iter()
            .map(|os| {
                let x = match policy_expectation(e.backend, os) {
                    Expect::Pass => "enforced".to_string(),
                    Expect::Refused(_) => "refused".to_string(),
                };
                format!("{x} on {os}")
            })
            .collect();
        let yes = |b: bool| if b { "yes" } else { "no" };
        let warm = match (c.claim, c.prepare) {
            (true, true) => "claim, apps ready ahead",
            (true, false) => "claim",
            _ => "no",
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} |\n",
            e.id,
            e.guest,
            net.join(", "),
            yes(c.pause),
            yes(c.checkpoint && c.fork),
            yes(c.kvm),
            warm,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::BACKENDS;
    use std::collections::HashSet;

    /// Adding a backend or a kind means registering it here: a runtime with
    /// no conformance entry would be a runtime nobody checks.
    #[test]
    fn every_backend_has_a_conformance_entry() {
        for b in BACKENDS {
            assert!(
                entries().iter().any(|e| e.backend == b),
                "backend {b} has no conformance entry: add one to cua_vmm::conformance::entries()"
            );
        }
        // And the other way: an entry names a backend the build knows.
        for e in entries() {
            assert!(
                BACKENDS.contains(&e.backend),
                "{}: backend {} is not in egress::BACKENDS",
                e.id,
                e.backend
            );
        }
    }

    #[test]
    fn every_placement_word_is_covered() {
        // Each `--kind` word a Space can be created with has an entry, and
        // each runtime word of those kinds does too.
        let kinds: HashSet<&str> = entries().iter().map(|e| e.kind).collect();
        for k in ["container", "vm", "microvm"] {
            assert!(kinds.contains(k), "--kind {k} has no conformance entry");
        }
        let runtimes: HashSet<&str> = entries().iter().map(|e| e.runtime).collect();
        for r in [
            "gvisor",
            "runc",
            "qemu",
            "lume",
            "firecracker",
            "vz",
            "containerization",
        ] {
            assert!(
                runtimes.contains(r),
                "--runtime {r} has no conformance entry"
            );
        }
    }

    #[test]
    fn ids_are_unique_and_entries_are_well_formed() {
        let mut seen = HashSet::new();
        for e in entries() {
            assert!(seen.insert(e.id), "duplicate id {}", e.id);
            assert!(!e.needs.is_empty(), "{}: say what a live run needs", e.id);
            let c = capabilities(e.backend);
            if e.not_runnable.is_none() {
                assert!(!e.hosts.is_empty(), "{}: no host", e.id);
                assert!(
                    c.guests.contains(&e.guest),
                    "{}: its backend does not declare a {} guest",
                    e.id,
                    e.guest
                );
                assert!(!e.cases.is_empty(), "{}: no cases", e.id);
            }
            for case in e.cases {
                assert!(Case::ALL.contains(case));
            }
        }
    }

    /// The declared capabilities cannot promise what the support table
    /// refuses, and a refusal always says why in plain words.
    #[test]
    fn declared_policy_matches_the_support_table() {
        for e in entries() {
            for os in e.hosts {
                let x = expect(e, Case::Policy, os);
                let table = network::network_support(e.backend, os, Placement::Local);
                match (x, table) {
                    (Expect::Pass, NetworkSupport::Enforced { .. }) => {}
                    (
                        Expect::Refused(w),
                        NetworkSupport::Refuse { why } | NetworkSupport::NoNetworkOnly { why },
                    ) => {
                        assert_eq!(w, why);
                        assert!(w.len() > 20, "{}: say why: {w:?}", e.id);
                    }
                    (Expect::Refused(w), _) if !e.cases.contains(&Case::Policy) => {
                        assert!(!w.is_empty())
                    }
                    other => panic!("{} on {os}: {other:?}", e.id),
                }
            }
        }
    }

    /// A backend that cannot snapshot says so: pause, checkpoint and fork
    /// are consistent (a fork needs a checkpoint to fork from).
    #[test]
    fn capabilities_are_consistent() {
        for b in BACKENDS {
            let c = capabilities(b);
            if c.fork && !c.checkpoint {
                // Lume forks by cloning its disk, with no checkpoint.
                assert_eq!(b, BackendKind::Lume, "{b}: a fork needs a checkpoint");
            }
            if c.kvm {
                assert!(
                    c.guests.contains(&"linux"),
                    "{b}: KVM is for Linux guests here"
                );
            }
        }
        // Apple's Containerization: the runtime helper owns the VM.
        let a = capabilities(BackendKind::Containerization);
        assert!(!a.pause && !a.checkpoint && !a.fork);
    }

    /// The roster the live driver reads is the one committed next to it.
    #[test]
    fn the_driver_reads_the_registry() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/conformance/entries.json");
        let want = serde_json::to_string_pretty(&entries_json()).unwrap() + "\n";
        if std::env::var_os("CUA_REGEN_CONFORMANCE").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            have, want,
            "scripts/conformance/entries.json is stale: run `CUA_REGEN_CONFORMANCE=1 cargo test -p cua-vmm conformance`"
        );
    }

    /// The docs table is the generated one.
    #[test]
    fn the_docs_table_is_generated() {
        let doc = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../docs/content/docs/spaces/concepts/runtime-conformance.mdx");
        let text = std::fs::read_to_string(&doc).unwrap_or_default();
        let table = promises_table_markdown();
        assert!(
            text.contains(&table),
            "{} is missing the generated table:\n{table}",
            doc.display()
        );
    }

    /// Rules by name (`example.com`, `*.example.com`) are kept exactly where
    /// the Cua network gateway is the guest's resolver, and are refused with
    /// the same words everywhere else: no runtime silently ignores a name.
    #[test]
    fn named_rules_are_kept_by_the_gateway_and_refused_elsewhere() {
        use crate::egress::EgressPolicy;
        use crate::network::{NetworkContext, NetworkSettings};
        let mut settings = NetworkSettings::internet();
        settings.custom = EgressPolicy {
            allow: vec!["example.com".into()],
            deny: vec!["*.ads.example.net".into()],
        };
        let ctx = NetworkContext::default();
        // No runtime other than the gateway's compiles names into address rules.
        let refused = network::compile_mode(&settings, &ctx, network::Mode::Secure)
            .expect_err("a name is never compiled into an address rule");
        assert_eq!(refused.0, network::DOMAINS_GATEWAY_ONLY);
        for e in entries() {
            for os in e.hosts {
                let behind_gateway = matches!(
                    network::network_support(e.backend, os, Placement::Local),
                    NetworkSupport::Enforced { point } if point == crate::netgw::POINT
                );
                let named = network::compile_named(&settings, &ctx, network::Mode::Secure);
                if behind_gateway {
                    let (_, domains) =
                        named.unwrap_or_else(|x| panic!("{} on {os}: {}", e.id, x.0));
                    assert_eq!(domains.allow, ["example.com"], "{} on {os}", e.id);
                    assert_eq!(domains.deny, ["*.ads.example.net"], "{} on {os}", e.id);
                }
                // The old default never carries names to a runtime.
                let (_, old) = network::compile_named(&settings, &ctx, network::Mode::Legacy)
                    .expect("legacy drops names");
                assert!(
                    old.is_empty(),
                    "{} on {os}: names under the old default",
                    e.id
                );
            }
        }
    }

    /// A runtime that cannot claim says so in the matrix and is not asked
    /// for a warm case; one that can, is. Every runnable Linux guest takes
    /// the warm case; the macOS VM's is covered by its own tests.
    #[test]
    fn the_warm_case_follows_what_the_runtime_declares() {
        for e in entries() {
            let c = capabilities(e.backend);
            // A member that can be prepared can be claimed.
            assert!(!c.prepare || c.claim, "{}", e.id);
            if e.cases.contains(&Case::Warm) {
                assert!(
                    c.claim,
                    "{} is asked for warm claims but declares none",
                    e.id
                );
                assert_eq!(expect(e, Case::Warm, "linux"), Expect::Pass, "{}", e.id);
            }
        }
        assert!(entry("builtin-gvisor").unwrap().cases.contains(&Case::Warm));
        assert!(capabilities(BackendKind::Container).prepare);
        assert!(
            !capabilities(BackendKind::Firecracker).prepare,
            "paused members are unreachable until claimed"
        );
        assert!(matches!(
            expect(entry("lume-macos-vm").unwrap(), Case::Warm, "macos"),
            Expect::Refused(_)
        ));
        assert!(!capabilities(BackendKind::QemuDocker).claim);
    }
}
