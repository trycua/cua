//! Workload detection: from a request to what the solver needs to know.
//!
//! Explicit hints always win (kind, size, duration, latency, an explicit
//! host). Otherwise:
//!
//! - **kind and machine** from the image ([`ImageFacts`], resolved by the
//!   caller from the image catalog): macOS guests need a Mac; Windows and
//!   disk-only images are VMs and need a hypervisor (`/dev/kvm` on Linux);
//!   an image with a container rootfs is a container; one arch only pins
//!   the arch;
//! - **size** from what Spaces of this image used before ([`UsageBook`]:
//!   p95 of observed CPU and memory, plus headroom), else from the profile
//!   and the image (a headless rollout image books 1 vCPU / 1 GiB; a
//!   desktop 2 vCPU / 4 GiB; Windows and macOS 4 vCPU / 8 GiB);
//! - **count and grouping** from the call (`fork_many` groups stay whole on
//!   one machine; `--count` and compat bursts are one splittable group);
//! - **latency class**: a persistent agent is steady; more than one Space,
//!   or a job or compat burst, is a batch; one Space someone waits on is
//!   interactive;
//! - **duration** from the image's past lifetimes (median), else by class;
//! - **affinity**: the machines that hold the checkpoint the Spaces fork
//!   from (no copy needed).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::model::*;

/// What the image catalog says about an image (filled by the caller).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageFacts {
    /// `linux`, `macos`, `windows`.
    pub os: String,
    /// Architectures it is published for (`amd64`, `arm64`).
    #[serde(default)]
    pub arches: Vec<String>,
    /// It has a container rootfs.
    #[serde(default)]
    pub rootfs: bool,
    /// It has a VM disk.
    #[serde(default)]
    pub disk: bool,
    /// It runs a desktop (not a headless or `-slim` rollout image).
    #[serde(default)]
    pub desktop: bool,
    /// Its size, GiB (the base every host pulls once).
    #[serde(default)]
    pub size_gb: u64,
    /// On a registry only the user's network reaches.
    #[serde(default)]
    pub private: bool,
}

/// Who asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Caller {
    /// A person or an agent creating one Space (`create_space`, `cua spaces
    /// create`).
    #[default]
    Interactive,
    /// A batch: `fork_many`, `cua run --count`, a job.
    Batch,
    /// A compat gateway (E2B/Daytona/Modal SDK calls through `cua run
    /// --compat`): creates arrive in bursts.
    Compat,
    /// A persistent agent's Space.
    PersistentAgent,
}

/// One request for Spaces.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// An id for the group (a world, a request id).
    pub id: String,
    /// The image reference.
    pub image: String,
    /// What the catalog says about it.
    #[serde(default)]
    pub facts: ImageFacts,
    /// `container`, `vm`, `microvm`, or empty (auto).
    #[serde(default)]
    pub kind: String,
    /// vCPUs asked for.
    #[serde(default)]
    pub cpus: Option<u32>,
    /// Memory asked for, MiB.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// `rollout` (1 vCPU / 1 GiB, no desktop) or empty.
    #[serde(default)]
    pub profile: String,
    /// How many Spaces.
    pub count: u32,
    /// They are forks of one checkpoint that should share a machine.
    #[serde(default)]
    pub whole: bool,
    /// Who asked.
    #[serde(default)]
    pub caller: Caller,
    /// Expected lifetime, when the caller knows it (a TTL, a job timeout).
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Latency class, when the caller says.
    #[serde(default)]
    pub latency: Option<Latency>,
    /// Machines holding the checkpoint it forks from.
    #[serde(default)]
    pub holders: Vec<String>,
    /// Seconds to copy that checkpoint elsewhere.
    #[serde(default)]
    pub sync_secs: u32,
    /// An explicit machine.
    #[serde(default)]
    pub pin_host: String,
    /// An accelerator.
    #[serde(default)]
    pub gpu: String,
}

/// Observed usage per image: what Spaces of it really used and how long
/// they lived. Kept in the cua home and refined after every Space.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageBook {
    /// By image key ([`image_key`]).
    pub images: BTreeMap<String, Usage>,
}

/// Samples of one image (the most recent [`USAGE_SAMPLES`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Peak CPU of each Space, thousandths.
    pub millicpus: Vec<u64>,
    /// Peak memory of each Space, MiB.
    pub memory_mb: Vec<u64>,
    /// Lifetime of each Space, seconds.
    pub lifetime_secs: Vec<u64>,
}

/// Samples kept per image.
pub const USAGE_SAMPLES: usize = 64;
/// Headroom over the observed p95 when booking.
pub const USAGE_HEADROOM: f64 = 1.25;
/// Samples needed before observed usage replaces the defaults.
pub const USAGE_MIN_SAMPLES: usize = 3;

/// The key usage is kept under: the image without its digest.
pub fn image_key(image: &str) -> String {
    image.split('@').next().unwrap_or(image).trim().to_string()
}

impl UsageBook {
    /// Records one finished Space: its peak CPU when it was measured, its
    /// memory and how long it lived.
    pub fn record(
        &mut self,
        image: &str,
        millicpus: Option<u64>,
        memory_mb: u64,
        lifetime_secs: u64,
    ) {
        let u = self.images.entry(image_key(image)).or_default();
        let push = |v: &mut Vec<u64>, x: u64| {
            v.push(x);
            if v.len() > USAGE_SAMPLES {
                v.remove(0);
            }
        };
        if let Some(c) = millicpus {
            push(&mut u.millicpus, c);
        }
        push(&mut u.memory_mb, memory_mb);
        push(&mut u.lifetime_secs, lifetime_secs);
    }

    /// Booked size from observations: p95 with headroom; CPU only when it
    /// was measured often enough (`None` in either: keep the default).
    pub fn size(&self, image: &str) -> Option<(Option<u64>, u64)> {
        let u = self.images.get(&image_key(image))?;
        if u.memory_mb.len() < USAGE_MIN_SAMPLES {
            return None;
        }
        let cpu = (u.millicpus.len() >= USAGE_MIN_SAMPLES).then(|| {
            ((percentile(&u.millicpus, 0.95) as f64 * USAGE_HEADROOM).ceil() as u64)
                .max(MIN_MILLICPUS)
        });
        let mem = (percentile(&u.memory_mb, 0.95) as f64 * USAGE_HEADROOM).ceil() as u64;
        Some((cpu, mem.max(MIN_MEMORY_MB)))
    }

    /// Median lifetime (`None`: too few).
    pub fn lifetime(&self, image: &str) -> Option<u64> {
        let u = self.images.get(&image_key(image))?;
        (u.lifetime_secs.len() >= USAGE_MIN_SAMPLES).then(|| percentile(&u.lifetime_secs, 0.5))
    }
}

/// Smallest booking: a quarter core.
pub const MIN_MILLICPUS: u64 = 250;
/// Smallest booking: 256 MiB.
pub const MIN_MEMORY_MB: u64 = 256;

fn percentile(v: &[u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i.min(s.len() - 1)]
}

/// Default lifetimes by class when nothing was observed.
pub fn default_duration(l: Latency) -> u64 {
    match l {
        Latency::Interactive => 3600,
        Latency::Batch => 1800,
        Latency::Steady => 30 * 24 * 3600,
    }
}

/// The kind an image runs as (`auto` resolved).
pub fn kind_of(r: &Request) -> String {
    if !r.kind.is_empty() && r.kind != "auto" {
        return r.kind.clone();
    }
    let f = &r.facts;
    if f.os == "macos" || f.os == "windows" || (f.disk && !f.rootfs) {
        "vm".into()
    } else {
        "container".into()
    }
}

/// The demand of one request.
pub fn detect(r: &Request, book: &UsageBook) -> DemandGroup {
    let kind = kind_of(r);
    let f = &r.facts;
    let macos = f.os == "macos";
    let windows = f.os == "windows";
    let latency = r.latency.unwrap_or(match r.caller {
        Caller::PersistentAgent => Latency::Steady,
        Caller::Batch | Caller::Compat => Latency::Batch,
        Caller::Interactive if r.count > 1 => Latency::Batch,
        Caller::Interactive => Latency::Interactive,
    });
    let (def_cpu, def_mem) = if r.profile == "rollout" {
        (1000, 1024)
    } else if macos || windows {
        (4000, 8192)
    } else if kind == "vm" {
        (2000, 4096)
    } else if !f.desktop {
        (1000, 1024)
    } else {
        (2000, 4096)
    };
    let (cpu, mem) = match (r.cpus, r.memory_mb) {
        (Some(c), Some(m)) => (u64::from(c) * 1000, m),
        (c, m) => {
            let (lc, lm) = book
                .size(&r.image)
                .map(|(c, m)| (c.unwrap_or(def_cpu), m))
                .unwrap_or((def_cpu, def_mem));
            (
                c.map(|c| u64::from(c) * 1000).unwrap_or(lc),
                m.unwrap_or(lm),
            )
        }
    };
    let arch = if f.arches.len() == 1 {
        f.arches[0].clone()
    } else if macos {
        "arm64".into()
    } else {
        String::new()
    };
    let duration = r
        .duration_secs
        .or_else(|| book.lifetime(&r.image))
        .unwrap_or_else(|| default_duration(latency));
    DemandGroup {
        id: if r.id.is_empty() {
            image_key(&r.image)
        } else {
            r.id.clone()
        },
        count: r.count.max(1),
        // A Space's own disk is a copy-on-write overlay over the host's
        // shared base: 1 GiB booked, more for a VM's writes.
        per: Shape::new(cpu, mem, if kind == "vm" { 4 } else { 1 }),
        req: Requirements {
            needs_kvm: (kind == "vm" || kind == "microvm") && !macos,
            kind,
            allow_metal: false,
            arch,
            host_os: if macos { "macos".into() } else { String::new() },
            private_image: f.private,
            gpu: r.gpu.clone(),
            image: image_key(&r.image),
            base_gb: f.size_gb,
        },
        whole: r.whole && r.count > 1,
        duration_secs: duration,
        latency,
        holders: r.holders.clone(),
        sync_secs: r.sync_secs,
        pin_host: r.pin_host.clone(),
        soft: false,
        soft_usd: 0.0,
    }
}

/// Requests arriving close together become one solve (an E2B create loop,
/// `asyncio.gather` of creates): same image, size and caller are merged
/// into one group of their total count.
pub fn coalesce(requests: &[Request]) -> Vec<Request> {
    let mut out: Vec<Request> = Vec::new();
    for r in requests {
        if r.whole || !r.pin_host.is_empty() || !r.holders.is_empty() {
            out.push(r.clone());
            continue;
        }
        if let Some(m) = out.iter_mut().find(|m| {
            !m.whole
                && m.pin_host.is_empty()
                && m.holders.is_empty()
                && m.image == r.image
                && m.kind == r.kind
                && m.cpus == r.cpus
                && m.memory_mb == r.memory_mb
                && m.profile == r.profile
                && m.caller == r.caller
                && m.gpu == r.gpu
        }) {
            m.count += r.count;
            // Several Spaces at once: a burst, whoever asked.
            if m.latency.is_none() && m.caller == Caller::Interactive {
                m.latency = Some(Latency::Batch);
            }
            m.duration_secs = match (m.duration_secs, r.duration_secs) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
        } else {
            out.push(r.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(image: &str, facts: ImageFacts) -> Request {
        Request {
            image: image.into(),
            facts,
            count: 1,
            ..Default::default()
        }
    }

    fn linux_slim() -> ImageFacts {
        ImageFacts {
            os: "linux".into(),
            arches: vec!["amd64".into(), "arm64".into()],
            rootfs: true,
            disk: true,
            desktop: false,
            size_gb: 2,
            private: false,
        }
    }

    #[test]
    fn images_decide_kind_machine_and_size() {
        let book = UsageBook::default();
        let d = detect(&req("ghcr.io/trycua/linux:24.04-slim", linux_slim()), &book);
        assert_eq!(d.req.kind, "container");
        assert!(!d.req.needs_kvm);
        assert_eq!(d.req.arch, "");
        assert_eq!(d.per, Shape::new(1000, 1024, 1));
        assert_eq!(d.latency, Latency::Interactive);
        let win = ImageFacts {
            os: "windows".into(),
            arches: vec!["amd64".into()],
            disk: true,
            desktop: true,
            size_gb: 20,
            ..Default::default()
        };
        let d = detect(&req("windows:11", win), &book);
        assert_eq!(d.req.kind, "vm");
        assert!(d.req.needs_kvm);
        assert_eq!(d.req.arch, "amd64");
        assert_eq!(d.per.millicpus, 4000);
        let mac = ImageFacts {
            os: "macos".into(),
            disk: true,
            desktop: true,
            ..Default::default()
        };
        let d = detect(&req("macos:26", mac), &book);
        assert_eq!(d.req.host_os, "macos");
        assert!(!d.req.needs_kvm, "a Mac runs it with its own hypervisor");
        assert_eq!(d.req.arch, "arm64");
        // A microVM of a Linux image needs KVM on a Linux host.
        let mut r = req("ghcr.io/trycua/linux:24.04-slim", linux_slim());
        r.kind = "microvm".into();
        assert!(detect(&r, &book).req.needs_kvm);
        // Private registries never go to a cloud.
        let mut f = linux_slim();
        f.private = true;
        assert!(detect(&req("registry.lan/x:1", f), &book).req.private_image);
    }

    #[test]
    fn hints_win_and_learning_refines() {
        let mut book = UsageBook::default();
        let image = "ghcr.io/trycua/linux:24.04-slim";
        // Too few samples: defaults.
        book.record(image, Some(300), 400, 600);
        assert_eq!(detect(&req(image, linux_slim()), &book).per.memory_mb, 1024);
        for _ in 0..9 {
            book.record(&format!("{image}@sha256:abc"), Some(300), 400, 600);
        }
        let d = detect(&req(image, linux_slim()), &book);
        assert_eq!(d.per.memory_mb, 500, "p95 400 MiB + 25%");
        assert_eq!(d.per.millicpus, 375);
        assert_eq!(d.duration_secs, 600, "median lifetime");
        // Explicit size and duration win.
        let mut r = req(image, linux_slim());
        r.cpus = Some(2);
        r.memory_mb = Some(2048);
        r.duration_secs = Some(99);
        let d = detect(&r, &book);
        assert_eq!(d.per, Shape::new(2000, 2048, 1));
        assert_eq!(d.duration_secs, 99);
        // Samples are bounded.
        for _ in 0..200 {
            book.record(image, Some(1), 1, 1);
        }
        // Memory alone learned (no CPU measured): CPU stays the default.
        let mut mem_only = UsageBook::default();
        for _ in 0..5 {
            mem_only.record("img:1", None, 60, 100);
        }
        let d = detect(&req("img:1", linux_slim()), &mem_only);
        assert_eq!(d.per, Shape::new(1000, 256, 1), "60 MiB + 25% < 256 floor");
        assert_eq!(book.images[image].memory_mb.len(), USAGE_SAMPLES);
    }

    #[test]
    fn callers_decide_latency_and_bursts_coalesce() {
        let book = UsageBook::default();
        let mut r = req("ghcr.io/trycua/linux:24.04-slim", linux_slim());
        r.caller = Caller::PersistentAgent;
        let d = detect(&r, &book);
        assert_eq!(d.latency, Latency::Steady);
        assert_eq!(d.duration_secs, 30 * 24 * 3600);
        r.caller = Caller::Compat;
        assert_eq!(detect(&r, &book).latency, Latency::Batch);
        // An E2B create loop: 32 single creates become one group of 32.
        let one = req("ghcr.io/trycua/linux:24.04-slim", linux_slim());
        let merged = coalesce(&vec![one.clone(); 32]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].count, 32);
        assert_eq!(detect(&merged[0], &book).latency, Latency::Batch);
        // Fork groups stay apart and whole.
        let mut w = one.clone();
        w.whole = true;
        w.count = 8;
        w.holders = vec!["studio".into()];
        let merged = coalesce(&[w.clone(), w, one]);
        assert_eq!(merged.len(), 3);
        assert!(detect(&merged[0], &book).whole);
    }
}
