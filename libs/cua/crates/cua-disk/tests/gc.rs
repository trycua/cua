//! Accounting and garbage collection against a temp cua home, a fake
//! container engine and a fake Lume, with a fake clock (explicit `now` and
//! explicit last-used times). Nothing here touches the host's engine, Lume or
//! real cua home.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cua_disk::docker::{DockerApi, DockerContainer, DockerImage, DockerVolume};
use cua_disk::lume::{LumeApi, LumeVm};
use cua_disk::{Budget, CacheConfig, Category, GcOptions, Layout, Scanner, collect, plan};
use cua_vmm::disk::{GIB, mark_used_at};

const MIB: u64 = 1 << 20;

fn t(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// "Now" for every test: day 20 000 since the epoch.
fn now() -> SystemTime {
    t(20_000 * 86_400)
}

fn days_ago(d: u64) -> SystemTime {
    now() - Duration::from_secs(d * 86_400)
}

/// A cached containerDisk of `mib` MiB last used at `used`.
fn cached_disk(l: &Layout, hex: &str, mib: u64, used: SystemTime) -> PathBuf {
    let dir = l.images().join("disks").join(hex);
    std::fs::create_dir_all(&dir).unwrap();
    let disk = dir.join("disk.qcow2");
    cua_disk::qcow2::write_header(&disk, None).unwrap();
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(&disk)
        .unwrap();
    // Real blocks, so allocated sizes are what they claim.
    use std::io::Write;
    let mut w = std::io::BufWriter::new(f);
    let chunk = vec![7u8; MIB as usize];
    for _ in 0..mib {
        w.write_all(&chunk).unwrap();
    }
    w.flush().unwrap();
    drop(w);
    mark_used_at(&dir, used);
    let refs = l.images().join("refs");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::write(
        refs.join(format!("{hex}.json")),
        format!(r#"{{"reference":"ghcr.io/x/{hex}:1","digest":"sha256:{hex}"}}"#),
    )
    .unwrap();
    dir
}

/// A stopped QEMU sandbox whose overlay is backed by `backing`.
fn qemu_sandbox(l: &Layout, name: &str, backing: &Path) {
    let dir = l.qemu().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let disk = dir.join("disk.qcow2");
    cua_disk::qcow2::write_header(&disk, Some(backing)).unwrap();
    let st = cua_vmm::qemu::QemuState {
        name: name.into(),
        kind: cua_vmm::qemu::EntryKind::Instance,
        arch: cua_vmm::Arch::Aarch64,
        os: cua_vmm::GuestOs::Linux,
        disk,
        disk_format: "qcow2".into(),
        install_iso: None,
        seed_iso: None,
        firmware: cua_vmm::qemu::Firmware::Bios,
        cpus: 2,
        memory_mb: 2048,
        ports: Default::default(),
        vnc_display: None,
        qmp_port: None,
        pid: None,
        accel: String::new(),
        ssh: None,
        restrict_network: false,
        extra_args: vec![],
        snapshots: vec![],
        paused: false,
        source: None,
        created_at: 1,
        gpu: None,
    };
    std::fs::write(dir.join("state.json"), serde_json::to_vec(&st).unwrap()).unwrap();
}

fn config(budget: Budget) -> CacheConfig {
    CacheConfig {
        budget,
        ..CacheConfig::default()
    }
}

fn opts(budget: Option<Budget>, all: bool, dry_run: bool) -> GcOptions {
    GcOptions {
        all,
        dry_run,
        budget,
        grace: Duration::from_secs(600),
        now: now(),
    }
}

#[tokio::test]
async fn lru_evicts_oldest_unreferenced_until_under_budget() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let used = cached_disk(&l, "aaa", 3, days_ago(30)); // oldest, but referenced
    let old = cached_disk(&l, "bbb", 3, days_ago(20));
    let mid = cached_disk(&l, "ccc", 3, days_ago(10));
    let new = cached_disk(&l, "ddd", 3, days_ago(1));
    // Used within the grace period: stays even though over budget.
    let fresh = cached_disk(&l, "eee", 3, now() - Duration::from_secs(60));
    qemu_sandbox(&l, "box", &used.join("disk.qcow2"));

    let s = Scanner::new(l.clone(), config(Budget::Bytes(7 * MIB)), None, None);
    let r = s.scan_at(now()).await;
    let disks: Vec<_> = r
        .items
        .iter()
        .filter(|i| i.kind == "containerdisk")
        .collect();
    assert_eq!(disks.len(), 5);
    let a = disks.iter().find(|i| i.location.ends_with("aaa")).unwrap();
    assert_eq!(a.referenced_by, vec!["box".to_string()]);
    assert!(r.cache_bytes >= 15 * MIB);
    // The sandbox itself is accounted, not evictable.
    let sb = r.items.iter().find(|i| i.name == "box").unwrap();
    assert_eq!(sb.category, Category::Sandboxes);
    assert!(!sb.evictable);

    // Plan only (pure): oldest unreferenced first until <= 7 MiB.
    let p = plan(&r, r.budget_bytes, false, now(), Duration::from_secs(600));
    let names: Vec<&str> = p
        .iter()
        .map(|(n, _)| r.items[*n].location.as_str())
        .collect();
    // 15 MiB -> 12 -> 9 -> 6: three evictions, oldest first.
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(
        names[0].ends_with("bbb") && names[1].ends_with("ccc") && names[2].ends_with("ddd"),
        "{names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("aaa") || n.ends_with("eee"))
    );

    let g = collect(&s, opts(None, false, false)).await;
    assert!(g.skipped.is_none());
    assert!(!old.exists() && !mid.exists());
    assert!(used.exists(), "referenced by a stopped sandbox");
    assert!(fresh.exists(), "inside the grace period");
    assert!(g.after <= g.before - 6 * MIB);
    assert_eq!(g.kept_referenced, 1);
    // Refs of evicted images are gone too.
    assert!(!l.images().join("refs/bbb.json").exists());
    assert!(l.images().join("refs/aaa.json").exists());
    assert!(!new.exists());
    let r2 = s.scan_at(now()).await;
    assert!(r2.cache_bytes <= 7 * MIB, "{}", r2.cache_bytes);
    // Under budget now: a second run removes nothing.
    assert!(
        collect(&s, opts(None, false, false))
            .await
            .removed
            .is_empty()
    );
}

#[tokio::test]
async fn all_evicts_every_eligible_entry_and_dry_run_touches_nothing() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let used = cached_disk(&l, "aaa", 1, days_ago(30));
    let a = cached_disk(&l, "bbb", 1, days_ago(2));
    let b = cached_disk(&l, "ccc", 1, days_ago(3));
    qemu_sandbox(&l, "keep", &used.join("disk.qcow2"));
    let s = Scanner::new(l.clone(), config(Budget::Off), None, None);

    let dry = collect(&s, opts(None, true, true)).await;
    assert_eq!(dry.removed.len(), 2);
    assert!(a.exists() && b.exists());
    // Budget off: nothing but orphans without --all.
    let none = collect(&s, opts(None, false, false)).await;
    assert!(none.removed.is_empty());
    let all = collect(&s, opts(None, true, false)).await;
    assert_eq!(all.removed.len(), 2);
    assert!(!a.exists() && !b.exists() && used.exists());
    assert!(l.qemu().join("keep").exists());
}

#[tokio::test]
async fn orphans_go_and_in_progress_downloads_stay() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let blobs = l.images().join("blobs/sha256");
    std::fs::create_dir_all(&blobs).unwrap();
    let stale = blobs.join("aa.partial");
    let live = blobs.join("bb.partial");
    std::fs::write(&stale, vec![0u8; 4096]).unwrap();
    std::fs::write(&live, vec![0u8; 4096]).unwrap();
    mark_used_at(&stale, days_ago(3));
    mark_used_at(&live, now() - Duration::from_secs(5));
    // An extraction another process holds the lock for.
    let locked = l.images().join("disks/fff");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join(".lock"), b"").unwrap();
    mark_used_at(&locked, days_ago(9));
    // A frozen layer no entry uses, and one a checkpoint uses.
    let layers = l.qemu().join("_layers");
    std::fs::create_dir_all(&layers).unwrap();
    let orphan_layer = layers.join("gone-1.qcow2");
    let kept_layer = layers.join("ck-1.qcow2");
    cua_disk::qcow2::write_header(&orphan_layer, None).unwrap();
    cua_disk::qcow2::write_header(&kept_layer, None).unwrap();
    qemu_sandbox(&l, "forked", &kept_layer);

    let s = Scanner::new(l.clone(), config(Budget::Off), None, None);
    let g = collect(&s, opts(None, false, false)).await;
    let reasons: Vec<&str> = g.removed.iter().map(|r| r.reason.as_str()).collect();
    assert!(reasons.iter().all(|r| *r == "orphan"), "{reasons:?}");
    assert!(!stale.exists() && live.exists());
    assert!(locked.exists());
    assert!(!orphan_layer.exists() && kept_layer.exists());
    // Even --all leaves a locked extraction and a live download alone.
    collect(&s, opts(None, true, false)).await;
    assert!(locked.exists() && live.exists());
}

// ------------------------------------------------------------ fake engine

#[derive(Default)]
struct FakeDocker {
    images: Mutex<Vec<DockerImage>>,
    containers: Vec<DockerContainer>,
    volumes: Mutex<Vec<DockerVolume>>,
    removed: Mutex<Vec<String>>,
}

fn labels(kv: &[(&str, &str)]) -> HashMap<String, String> {
    kv.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[async_trait]
impl DockerApi for FakeDocker {
    async fn images(&self) -> Result<Vec<DockerImage>, String> {
        Ok(self.images.lock().unwrap().clone())
    }
    async fn containers(&self) -> Result<Vec<DockerContainer>, String> {
        Ok(self.containers.clone())
    }
    async fn managed_volumes(&self) -> Result<Vec<DockerVolume>, String> {
        Ok(self
            .volumes
            .lock()
            .unwrap()
            .iter()
            .filter(|v| cua_disk::docker::is_managed(&v.labels))
            .cloned()
            .collect())
    }
    async fn remove_image(&self, reference: &str) -> Result<(), String> {
        let mut imgs = self.images.lock().unwrap();
        let Some(pos) = imgs
            .iter()
            .position(|i| i.id == reference || i.tags.iter().any(|t| t == reference))
        else {
            return Err(format!("no such image {reference}"));
        };
        // Like the engine: refuse an image a container uses.
        if self.containers.iter().any(|c| c.image_id == imgs[pos].id) {
            return Err("image is being used by a container".into());
        }
        imgs.remove(pos);
        self.removed.lock().unwrap().push(reference.to_string());
        Ok(())
    }
    async fn remove_volume(&self, name: &str) -> Result<(), String> {
        self.volumes.lock().unwrap().retain(|v| v.name != name);
        self.removed.lock().unwrap().push(format!("volume:{name}"));
        Ok(())
    }
}

fn image(
    id: &str,
    tags: &[&str],
    l: HashMap<String, String>,
    size: u64,
    created: u64,
) -> DockerImage {
    DockerImage {
        id: id.into(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        labels: l,
        size,
        created,
    }
}

#[tokio::test]
async fn docker_gc_never_touches_unlabelled_or_referenced_objects() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let old = 1_000_000; // unix seconds, long before `now()`
    let home = cua_vmm::container::home_tag_for(d.path());
    let managed = |kind: &str| {
        labels(&[
            ("ai.cua.managed", "true"),
            ("ai.cua.kind", kind),
            ("ai.cua.home", home.as_str()),
        ])
    };
    let other_home = labels(&[
        ("ai.cua.managed", "true"),
        ("ai.cua.kind", "build-image"),
        ("ai.cua.home", "0123456789ab"),
    ]);
    // The SDK pulled `alpine:3.20` (ledger) and `node:22` (the user later
    // re-pulled it: the engine's id no longer matches the record).
    let ledger = cua_vmm::container::ledger::PullLedger::new(l.docker_pulls());
    ledger.record("alpine:3.20", "sha256:pulled");
    mark_used_at(&ledger.file("alpine:3.20"), days_ago(40));
    ledger.record("node:22", "sha256:node-old");
    let fake = Arc::new(FakeDocker {
        images: Mutex::new(vec![
            // The user's own images: no label, no ledger record.
            image("sha256:user", &["python:3.12"], labels(&[]), GIB, old),
            image(
                "sha256:user2",
                &["myapp:dev"],
                labels(&[("maintainer", "me")]),
                GIB,
                old,
            ),
            image("sha256:node-new", &["node:22"], labels(&[]), GIB, old),
            // SDK build images: one used by a container, one not.
            image(
                "sha256:b1",
                &["cua-vmm/build:cua-b-1"],
                managed("build-image"),
                GIB,
                old,
            ),
            image(
                "sha256:b2",
                &["cua-vmm/build:cua-b-2"],
                managed("build-image"),
                GIB,
                old,
            ),
            // Another cua home's build image, and one from before the home
            // label (it belongs to the default home, not this temp one).
            image(
                "sha256:b3",
                &["cua-vmm/build:cua-b-3"],
                other_home,
                GIB,
                old,
            ),
            image(
                "sha256:b4",
                &["cua-vmm/build:cua-b-4"],
                labels(&[("ai.cua.managed", "true"), ("ai.cua.kind", "build-image")]),
                GIB,
                old,
            ),
            // A user checkpoint (SDK-labelled, but user data).
            image(
                "sha256:ck",
                &["cua-vmm/checkpoint:mine"],
                managed("checkpoint"),
                GIB,
                old,
            ),
            // Pulled by the SDK.
            image("sha256:pulled", &["alpine:3.20"], labels(&[]), GIB, old),
        ]),
        containers: vec![
            DockerContainer {
                id: "c1".into(),
                name: "named-box".into(),
                image_id: "sha256:b1".into(),
                labels: labels(&[
                    ("ai.cua.managed", "true"),
                    ("ai.cua.kind", "sandbox"),
                    ("ai.cua.home", home.as_str()),
                ]),
                state: "exited".into(),
                size_rw: 5 * MIB,
                ..Default::default()
            },
            // The user's container of their own image.
            DockerContainer {
                id: "c2".into(),
                name: "users-thing".into(),
                image_id: "sha256:user".into(),
                state: "exited".into(),
                ..Default::default()
            },
        ],
        volumes: Mutex::new(vec![
            DockerVolume {
                name: "users-volume".into(),
                labels: labels(&[]),
            },
            DockerVolume {
                name: "cua-scratch".into(),
                labels: labels(&[("ai.cua.managed", "true"), ("ai.cua.home", home.as_str())]),
            },
        ]),
        removed: Mutex::new(vec![]),
    });
    let s = Scanner::new(
        l.clone(),
        config(Budget::Bytes(0)),
        Some(fake.clone() as Arc<dyn DockerApi>),
        None,
    );
    let r = s.scan_at(now()).await;
    // Only SDK objects are listed.
    let listed: Vec<&str> = r.items.iter().map(|i| i.name.as_str()).collect();
    for user in ["python:3.12", "myapp:dev", "users-thing", "users-volume"] {
        assert!(
            !listed.contains(&user),
            "{user} must not be listed: {listed:?}"
        );
    }
    assert!(listed.contains(&"named-box"));
    let g = collect(&s, opts(None, true, false)).await;
    let removed = fake.removed.lock().unwrap().clone();
    assert_eq!(
        {
            let mut v = removed.clone();
            v.sort();
            v
        },
        vec![
            "alpine:3.20".to_string(),
            "cua-vmm/build:cua-b-2".to_string(),
            "volume:cua-scratch".to_string()
        ],
        "{g:?}"
    );
    let left: Vec<String> = fake
        .images
        .lock()
        .unwrap()
        .iter()
        .map(|i| i.id.clone())
        .collect();
    for keep in [
        "sha256:b3",
        "sha256:b4",
        "sha256:user",
        "sha256:user2",
        "sha256:node-new",
        "sha256:b1",
        "sha256:ck",
    ] {
        assert!(left.contains(&keep.to_string()), "{keep} was removed");
    }
    // The removed pull left the ledger; the stale node:22 record (its image
    // was replaced by the user) is dropped as an orphan record, never its
    // image.
    let recs: Vec<String> = ledger.records().into_iter().map(|r| r.reference).collect();
    assert!(recs.is_empty(), "{recs:?}");
}

// ------------------------------------------------------------ fake Lume

struct FakeLume {
    vms: Mutex<Vec<LumeVm>>,
}

#[async_trait]
impl LumeApi for FakeLume {
    async fn vms(&self) -> Result<Vec<LumeVm>, String> {
        Ok(self.vms.lock().unwrap().clone())
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        self.vms.lock().unwrap().retain(|v| v.name != name);
        Ok(())
    }
}

#[tokio::test]
async fn lume_bases_the_sdk_pulled_are_evicted_and_nothing_else() {
    use cua_vmm::lume::{OwnedKind, OwnedVms};
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let owned = OwnedVms::new(l.lume_owned());
    owned.mark(
        "cua-base-old",
        OwnedKind::Base,
        Some("ghcr.io/trycua/macos:15"),
    );
    owned.mark(
        "cua-base-busy",
        OwnedKind::Base,
        Some("ghcr.io/trycua/macos:26"),
    );
    owned.mark("my-mac", OwnedKind::Instance, Some("cua-base-old"));
    owned.mark("deleted-elsewhere", OwnedKind::Base, None);
    for n in [
        "cua-base-old",
        "cua-base-busy",
        "my-mac",
        "deleted-elsewhere",
    ] {
        mark_used_at(&owned.dir().join(format!("{n}.json")), days_ago(30));
    }
    let vm = |n: &str, s: &str| LumeVm {
        name: n.into(),
        status: s.into(),
        allocated: 27 * GIB,
    };
    let fake = Arc::new(FakeLume {
        vms: Mutex::new(vec![
            vm("cua-base-old", "stopped"),
            vm("cua-base-busy", "pulling"),
            vm("my-mac", "stopped"),
            // The user's own VM (no ownership record).
            vm("users-vm", "stopped"),
        ]),
    });
    let s = Scanner::new(
        l.clone(),
        config(Budget::Bytes(GIB)),
        None,
        Some(fake.clone() as Arc<dyn LumeApi>),
    );
    let g = collect(&s, opts(None, false, false)).await;
    let left: Vec<String> = fake
        .vms
        .lock()
        .unwrap()
        .iter()
        .map(|v| v.name.clone())
        .collect();
    assert_eq!(left, vec!["cua-base-busy", "my-mac", "users-vm"], "{g:?}");
    assert!(!owned.owns("cua-base-old") && !owned.owns("deleted-elsewhere"));
    assert!(owned.owns("my-mac"));
}

#[tokio::test]
async fn report_totals_cover_every_category() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    cached_disk(&l, "aaa", 1, days_ago(1));
    std::fs::create_dir_all(l.logs()).unwrap();
    std::fs::write(l.daemon_log(), vec![b'x'; 8192]).unwrap();
    std::fs::create_dir_all(l.trajectories().join("m/1")).unwrap();
    std::fs::write(l.trajectories().join("m/1/shot.png"), vec![0u8; 8192]).unwrap();
    std::fs::write(d.path().join("credentials.json"), b"{}").unwrap();
    let s = Scanner::new(l, config(Budget::Auto), None, None);
    let r = s.scan_at(now()).await;
    let totals = r.totals();
    assert_eq!(totals.len(), Category::ALL.len());
    let get = |c: Category| totals.iter().find(|t| t.category == c).unwrap().bytes;
    assert!(get(Category::Images) >= MIB);
    assert!(get(Category::Logs) >= 8192);
    assert!(get(Category::Data) >= 8192);
    assert!(r.budget_bytes.is_some());
}

#[tokio::test]
async fn host_install_cache_is_counted_and_evicted_lru_but_downloads_in_flight_stay() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let dir = l.installables();
    assert_eq!(dir, d.path().join("cache").join("installables"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str, mib: u64, used: SystemTime| -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, vec![3u8; (mib * MIB) as usize]).unwrap();
        mark_used_at(&p, used);
        p
    };
    let old = file(&format!("{}.tar.gz", "a".repeat(64)), 3, days_ago(9));
    let mid = file(&format!("{}.zip", "b".repeat(64)), 3, days_ago(5));
    let fresh = file(&format!("{}.tar.gz", "c".repeat(64)), 3, now());
    let pulling = file(&format!("{}.zip.part-0000abcd", "d".repeat(64)), 1, now());
    let abandoned = file(
        &format!("{}.zip.part-0000ef01", "e".repeat(64)),
        1,
        days_ago(3),
    );
    // Another cache next to it is data, not the install cache.
    std::fs::create_dir_all(d.path().join("cache").join("other")).unwrap();

    let s = Scanner::new(l.clone(), config(Budget::Bytes(4 * MIB)), None, None);
    let r = s.scan_at(now()).await;
    let ours: Vec<_> = r
        .items
        .iter()
        .filter(|i| i.category == Category::Installables)
        .collect();
    assert_eq!(ours.len(), 5, "{ours:?}");
    assert!(Category::Installables.is_cache());
    assert!(r.cache_bytes >= 11 * MIB);
    assert!(
        r.items
            .iter()
            .any(|i| i.category == Category::Data && i.name == "other")
    );
    let totals = r.totals();
    let inst = totals
        .iter()
        .find(|t| t.category == Category::Installables)
        .unwrap();
    assert_eq!(inst.items, 5);

    // Budget 4 MiB: the abandoned download is an orphan, then LRU evicts
    // the oldest archives; the fresh archive (grace) and the download in
    // flight stay.
    let g = collect(&s, opts(None, false, false)).await;
    assert!(
        !abandoned.exists() && !old.exists() && !mid.exists(),
        "{g:?}"
    );
    assert!(fresh.exists() && pulling.exists(), "{g:?}");

    // `--all` still keeps what is in flight or inside the grace period.
    let g = collect(&s, opts(Some(Budget::Off), true, false)).await;
    assert!(fresh.exists() && pulling.exists(), "{g:?}");
    assert!(d.path().join("cache").join("other").exists());
}

#[tokio::test]
async fn the_icon_cache_is_listed_and_pruned() {
    let d = tempfile::tempdir().unwrap();
    let l = Layout::new(d.path());
    let dir = l.icons();
    assert_eq!(dir, d.path().join("cache").join("icons"));
    std::fs::create_dir_all(&dir).unwrap();
    let icon = |name: &str, used: SystemTime| -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, vec![7u8; 4096]).unwrap();
        mark_used_at(&p, used);
        p
    };
    let old = icon("0123456789abcdef0123456789abcdef.png", days_ago(9));
    let old_1x = icon("0123456789abcdef0123456789abcdef@1x.png", days_ago(9));
    let svg = icon("fedcba9876543210fedcba9876543210.svg", days_ago(8));

    let s = Scanner::new(l.clone(), config(Budget::Bytes(GIB)), None, None);
    let r = s.scan_at(now()).await;
    let ours: Vec<_> = r
        .items
        .iter()
        .filter(|i| i.category == Category::Icons)
        .collect();
    assert_eq!(ours.len(), 3, "{ours:?}");
    assert!(Category::Icons.is_cache() && ours.iter().all(|i| i.evictable));
    // Not double counted as `cache/` data.
    assert!(
        !r.items
            .iter()
            .any(|i| i.category == Category::Data && i.name == "icons")
    );

    // `cua cache prune --all` evicts them.
    let g = collect(&s, opts(Some(Budget::Off), true, false)).await;
    assert!(!old.exists() && !old_1x.exists() && !svg.exists(), "{g:?}");
}
