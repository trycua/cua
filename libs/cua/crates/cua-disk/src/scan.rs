//! Accounting: one [`Item`] per thing the SDK keeps on disk.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cua_vmm::disk::{self, CacheConfig, Space};
use serde::Serialize;

use crate::docker::{self, DockerApi};
use crate::layout::Layout;
use crate::lume::LumeApi;

/// What an item is, for the per-category totals of `cua cache du`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// The image cache: containerDisk disks, rootfs trees, layer blobs.
    Images,
    /// Container images the SDK pulled or built, in the engine.
    DockerImages,
    /// Lume base VMs the SDK pulled.
    LumeBases,
    /// Local build outputs.
    Builds,
    /// Install archives teleported apps fetch once on the host and send to
    /// local Spaces (`cache/installables`).
    Installables,
    /// App icons every UI shares (`cache/icons`, small; LRU-capped by the
    /// icon cache itself too).
    Icons,
    /// Sandboxes: VM disks, container layers, Lume VMs (never evicted).
    Sandboxes,
    /// Checkpoints and the frozen layers they share (never evicted).
    Checkpoints,
    /// Logs.
    Logs,
    /// User data: state files, trajectories, skills, bench registry, bench
    /// runs (never evicted).
    #[default]
    Data,
    /// Caches of the Python adapters (Hyper-V, Android, Tart, URL images).
    Legacy,
}

impl Category {
    /// Every category, in display order.
    pub const ALL: [Category; 11] = [
        Category::Images,
        Category::DockerImages,
        Category::LumeBases,
        Category::Builds,
        Category::Installables,
        Category::Icons,
        Category::Sandboxes,
        Category::Checkpoints,
        Category::Logs,
        Category::Data,
        Category::Legacy,
    ];

    /// Kebab-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Images => "images",
            Category::DockerImages => "docker-images",
            Category::LumeBases => "lume-bases",
            Category::Builds => "builds",
            Category::Installables => "installables",
            Category::Icons => "icons",
            Category::Sandboxes => "sandboxes",
            Category::Checkpoints => "checkpoints",
            Category::Logs => "logs",
            Category::Data => "data",
            Category::Legacy => "legacy",
        }
    }

    /// Whether items of this category count against the cache budget.
    pub fn is_cache(self) -> bool {
        matches!(
            self,
            Category::Images
                | Category::DockerImages
                | Category::LumeBases
                | Category::Builds
                | Category::Installables
                | Category::Icons
        )
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How an item is removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// Not removable by GC.
    #[default]
    None,
    /// A file or directory.
    Path(PathBuf),
    /// An engine image by reference or id (never forced).
    DockerImage(String),
    /// An engine volume.
    DockerVolume(String),
    /// A Lume VM the SDK created.
    LumeVm(String),
    /// A stale record only (its object is already gone).
    Record(PathBuf),
}

/// One thing on disk.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Item {
    /// Category.
    pub category: Category,
    /// Short kind (`containerdisk`, `rootfs`, `blob`, `qemu`, `container`,
    /// `lume`, `build-image`, `pulled-image`, `log`, ...).
    pub kind: String,
    /// Display name (sandbox name, image reference, directory name).
    pub name: String,
    /// Path, engine id or VM name.
    pub location: String,
    /// Bytes on disk (allocated blocks where the filesystem reports them).
    pub bytes: u64,
    /// Unix seconds of the last use, when known.
    pub last_used: Option<u64>,
    /// Unix seconds of creation, when known.
    pub created: Option<u64>,
    /// Sandbox status (`running`, `stopped`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// What still uses it (sandbox names).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub referenced_by: Vec<String>,
    /// Cache that GC may evict once unreferenced.
    pub evictable: bool,
    /// Garbage GC always removes (stale partial downloads, unreferenced
    /// frozen layers, records of deleted objects).
    pub orphan: bool,
    /// Being written by another process right now (never touched).
    pub in_progress: bool,
    /// An ephemeral sandbox.
    pub ephemeral: bool,
    /// How GC removes it.
    #[serde(skip)]
    pub target: Target,
}

/// Per-category totals.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CategoryTotal {
    /// Category.
    pub category: Category,
    /// Bytes.
    pub bytes: u64,
    /// Items.
    pub items: usize,
}

/// Everything the scan found.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    /// The cua home.
    pub home: PathBuf,
    /// Space on the cua home's volume.
    pub space: Option<Space>,
    /// Configured budget (`auto`, `off`, a size).
    pub budget: String,
    /// Resolved budget in bytes (`None`: no limit).
    pub budget_bytes: Option<u64>,
    /// Bytes of cache (the categories the budget covers).
    pub cache_bytes: u64,
    /// Items.
    pub items: Vec<Item>,
    /// What could not be inspected (engine down, Lume not serving).
    pub notes: Vec<String>,
}

impl Report {
    /// Per-category totals, every category included.
    pub fn totals(&self) -> Vec<CategoryTotal> {
        let mut m: BTreeMap<Category, CategoryTotal> = Category::ALL
            .iter()
            .map(|c| {
                (
                    *c,
                    CategoryTotal {
                        category: *c,
                        ..Default::default()
                    },
                )
            })
            .collect();
        for i in &self.items {
            let t = m.get_mut(&i.category).expect("every category");
            t.bytes = t.bytes.saturating_add(i.bytes);
            t.items += 1;
        }
        m.into_values().collect()
    }

    /// Total bytes of every item.
    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Collects a [`Report`].
pub struct Scanner {
    layout: Layout,
    config: CacheConfig,
    docker: Option<Arc<dyn DockerApi>>,
    lume: Option<Arc<dyn LumeApi>>,
    docker_note: Option<String>,
    lume_note: Option<String>,
}

/// How old a `.partial` download must be before it counts as abandoned.
pub const STALE_PARTIAL: Duration = Duration::from_secs(24 * 3600);

impl Scanner {
    /// A scanner over explicit sources (tests, embedders).
    pub fn new(
        layout: Layout,
        config: CacheConfig,
        docker: Option<Arc<dyn DockerApi>>,
        lume: Option<Arc<dyn LumeApi>>,
    ) -> Self {
        Self {
            layout,
            config,
            docker,
            lume,
            docker_note: None,
            lume_note: None,
        }
    }

    /// The host's sources: the container engine and `lume serve` when they
    /// already answer (neither is started).
    pub async fn system(layout: Layout) -> Self {
        let config = CacheConfig::load();
        let docker = docker::connect().await;
        let lume = crate::lume::connect().await;
        let mut s = Self::new(layout, config, docker, lume);
        if s.docker.is_none() {
            s.docker_note = Some(
                "container engine not reachable: SDK container images and containers are not counted"
                    .into(),
            );
        }
        if s.lume.is_none() && cfg!(target_os = "macos") {
            s.lume_note =
                Some("lume serve not running: Lume VMs the SDK created are not sized".into());
        }
        s
    }

    /// The layout.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The configuration.
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// The engine, when connected.
    pub fn docker(&self) -> Option<&Arc<dyn DockerApi>> {
        self.docker.as_ref()
    }

    /// Lume, when connected.
    pub fn lume(&self) -> Option<&Arc<dyn LumeApi>> {
        self.lume.as_ref()
    }

    /// Scans everything at `now`.
    pub async fn scan_at(&self, now: SystemTime) -> Report {
        let mut items = Vec::new();
        let mut notes: Vec<String> = self
            .docker_note
            .iter()
            .chain(self.lume_note.iter())
            .cloned()
            .collect();
        let refs = self.scan_qemu(&mut items);
        self.scan_images(&refs, now, &mut items);
        self.scan_builds(&mut items);
        self.scan_installables(now, &mut items);
        self.scan_icons(&mut items);
        if let Some(d) = &self.docker
            && let Err(e) = self.scan_docker(d.as_ref(), &mut items).await
        {
            notes.push(format!("container engine: {e}"));
        }
        self.scan_lume(&mut items, &mut notes).await;
        self.scan_logs(&mut items);
        self.scan_data(&mut items);
        let space = disk::space(self.layout.home()).ok();
        let cache_bytes: u64 = items
            .iter()
            .filter(|i| i.category.is_cache())
            .map(|i| i.bytes)
            .sum();
        let budget_bytes = self.config.budget.resolve(
            space.map(|s| s.available).unwrap_or(u64::MAX / 4),
            cache_bytes,
        );
        Report {
            home: self.layout.home().to_path_buf(),
            space,
            budget: self.config.budget.to_string(),
            budget_bytes,
            cache_bytes,
            items,
            notes,
        }
    }

    /// Bytes on disk per sandbox name (VM disks, container layers, Lume
    /// VMs): only the sandbox sources are read, not the image cache, so
    /// `cua sb ls` can show a SIZE column cheaply.
    pub async fn sandbox_sizes(&self) -> HashMap<String, u64> {
        let mut items = Vec::new();
        let mut notes = Vec::new();
        self.scan_qemu(&mut items);
        if let Some(d) = &self.docker {
            let _ = self.scan_docker(d.as_ref(), &mut items).await;
        }
        self.scan_lume(&mut items, &mut notes).await;
        let mut out: HashMap<String, u64> = HashMap::new();
        for i in items
            .into_iter()
            .filter(|i| i.category == Category::Sandboxes)
        {
            *out.entry(i.name).or_default() += i.bytes;
        }
        out
    }

    /// Scans everything now.
    pub async fn scan(&self) -> Report {
        self.scan_at(SystemTime::now()).await
    }

    /// QEMU entries; returns every file some entry's disk chain uses, with
    /// the entry names using it.
    fn scan_qemu(&self, items: &mut Vec<Item>) -> HashMap<PathBuf, Vec<String>> {
        use cua_vmm::qemu::{EntryKind, QemuState};
        let root = self.layout.qemu();
        let mut used: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let Ok(rd) = std::fs::read_dir(&root) else {
            return used;
        };
        let mut layer_files = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let dir = e.path();
            if name == "_layers" {
                if let Ok(l) = std::fs::read_dir(&dir) {
                    layer_files.extend(l.flatten().map(|f| f.path()));
                }
                continue;
            }
            let Ok(raw) = std::fs::read(dir.join("state.json")) else {
                continue;
            };
            let Ok(st) = serde_json::from_slice::<QemuState>(&raw) else {
                continue;
            };
            for f in crate::qcow2::chain(&st.disk) {
                let f = std::fs::canonicalize(&f).unwrap_or(f);
                used.entry(f).or_default().push(st.name.clone());
            }
            let running = st.pid.is_some_and(cua_vmm::host::pid_alive);
            let (category, kind) = match st.kind {
                EntryKind::Instance if name.starts_with("cua-build-") => {
                    (Category::Builds, "build-vm")
                }
                EntryKind::Instance => (Category::Sandboxes, "qemu"),
                EntryKind::Base => (Category::Checkpoints, "qemu-base"),
                EntryKind::Checkpoint => (Category::Checkpoints, "qemu-checkpoint"),
            };
            items.push(Item {
                category,
                kind: kind.into(),
                name: st.name.clone(),
                location: dir.display().to_string(),
                bytes: disk::allocated_size(&dir),
                last_used: disk::last_used(&dir).map(unix),
                created: Some(st.created_at),
                status: Some(if running { "running" } else { "stopped" }.into()),
                ephemeral: st.name.starts_with("cua-eph-"),
                ..Default::default()
            });
        }
        for f in layer_files {
            let canon = std::fs::canonicalize(&f).unwrap_or_else(|_| f.clone());
            let by = used.get(&canon).cloned().unwrap_or_default();
            items.push(Item {
                category: Category::Checkpoints,
                kind: "qemu-layer".into(),
                name: f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                location: f.display().to_string(),
                bytes: disk::allocated_size(&f),
                last_used: disk::last_used(&f).map(unix),
                orphan: by.is_empty(),
                referenced_by: by,
                target: Target::Path(f.clone()),
                ..Default::default()
            });
        }
        used
    }

    fn scan_images(
        &self,
        used: &HashMap<PathBuf, Vec<String>>,
        now: SystemTime,
        items: &mut Vec<Item>,
    ) {
        let root = self.layout.images();
        let refs = cua_image::ImageCache::new(&root).refs();
        let names_for = |hex: &str| -> String {
            let n: Vec<&str> = refs
                .iter()
                .filter(|(_, d)| d.ends_with(hex))
                .map(|(r, _)| r.as_str())
                .collect();
            if n.is_empty() {
                hex.chars().take(12).collect()
            } else {
                n.join(", ")
            }
        };
        let users_of = |dir: &Path| -> Vec<String> {
            let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            let mut by: BTreeSet<String> = BTreeSet::new();
            for (f, names) in used {
                if f.starts_with(&canon) || f.starts_with(dir) {
                    by.extend(names.iter().cloned());
                }
            }
            by.into_iter().collect()
        };
        for (sub, kind, done) in [
            ("disks", "containerdisk", "disk.qcow2"),
            ("rootfs", "rootfs", ".complete"),
        ] {
            let Ok(rd) = std::fs::read_dir(root.join(sub)) else {
                continue;
            };
            for e in rd.flatten() {
                let dir = e.path();
                let hex = e.file_name().to_string_lossy().to_string();
                let in_progress = dir.join(".lock").exists() || !dir.join(done).exists();
                let by = users_of(&dir);
                let stale = in_progress
                    && !dir.join(".lock").exists()
                    && disk::last_used(&dir)
                        .and_then(|t| now.duration_since(t).ok())
                        .is_some_and(|age| age > STALE_PARTIAL);
                items.push(Item {
                    category: Category::Images,
                    kind: kind.into(),
                    name: names_for(&hex),
                    location: dir.display().to_string(),
                    bytes: disk::allocated_size(&dir),
                    last_used: disk::last_used(&dir).map(unix),
                    evictable: true,
                    // An abandoned extraction (no lock, never completed).
                    orphan: stale && by.is_empty(),
                    in_progress: in_progress && !stale,
                    referenced_by: by,
                    target: Target::Path(dir),
                    ..Default::default()
                });
            }
        }
        if let Ok(rd) = std::fs::read_dir(root.join("blobs").join("sha256")) {
            for e in rd.flatten() {
                let p = e.path();
                let partial = p.extension().is_some_and(|x| x == "partial");
                let age = disk::last_used(&p).and_then(|t| now.duration_since(t).ok());
                let stale = partial && age.is_some_and(|a| a > STALE_PARTIAL);
                items.push(Item {
                    category: Category::Images,
                    kind: if partial { "partial" } else { "blob" }.into(),
                    name: e.file_name().to_string_lossy().chars().take(19).collect(),
                    location: p.display().to_string(),
                    bytes: disk::allocated_size(&p),
                    last_used: disk::last_used(&p).map(unix),
                    evictable: !partial,
                    orphan: stale,
                    in_progress: partial && !stale,
                    target: Target::Path(p),
                    ..Default::default()
                });
            }
        }
    }

    fn scan_builds(&self, items: &mut Vec<Item>) {
        let Ok(rd) = std::fs::read_dir(self.layout.build()) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            // The build SSH key is reused by every VM build.
            if name.starts_with("id_ed25519") {
                continue;
            }
            items.push(Item {
                category: Category::Builds,
                kind: "build-output".into(),
                name,
                location: p.display().to_string(),
                bytes: disk::allocated_size(&p),
                last_used: disk::last_used(&p).map(unix),
                evictable: true,
                target: Target::Path(p),
                ..Default::default()
            });
        }
    }

    /// The host install cache (`<sha256>.<ext>`, verified before it is
    /// renamed into place; `<name>.part-<rand>` while it downloads). A
    /// download in flight is never touched; one abandoned for longer than
    /// [`STALE_PARTIAL`] is an orphan.
    fn scan_installables(&self, now: SystemTime, items: &mut Vec<Item>) {
        let Ok(rd) = std::fs::read_dir(self.layout.installables()) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            // Files only: nothing else is written here, so a directory or a
            // link is not the SDK's to remove.
            if !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let partial = name.contains(".part-");
            let stale = partial
                && disk::last_used(&p)
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|age| age > STALE_PARTIAL);
            items.push(Item {
                category: Category::Installables,
                kind: if partial { "partial" } else { "installable" }.into(),
                name,
                location: p.display().to_string(),
                bytes: disk::allocated_size(&p),
                last_used: disk::last_used(&p).map(unix),
                evictable: !partial,
                orphan: stale,
                in_progress: partial && !stale,
                target: Target::Path(p),
                ..Default::default()
            });
        }
    }

    /// The app icon cache (`cache/icons`): small PNG and SVG files, each
    /// evictable (a later lookup fetches it again).
    fn scan_icons(&self, items: &mut Vec<Item>) {
        let Ok(rd) = std::fs::read_dir(self.layout.icons()) else {
            return;
        };
        for e in rd.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let p = e.path();
            items.push(Item {
                category: Category::Icons,
                kind: "icon".into(),
                name: e.file_name().to_string_lossy().into_owned(),
                location: p.display().to_string(),
                bytes: disk::allocated_size(&p),
                last_used: disk::last_used(&p).map(unix),
                evictable: true,
                target: Target::Path(p),
                ..Default::default()
            });
        }
    }

    async fn scan_docker(&self, d: &dyn DockerApi, items: &mut Vec<Item>) -> Result<(), String> {
        let containers = d.containers().await?;
        let images = d.images().await?;
        let ledger = cua_vmm::container::ledger::PullLedger::new(self.layout.docker_pulls());
        let records = ledger.records();
        // Several cua homes can share one engine: objects labelled with
        // another home belong to that home's accounting and GC. Objects
        // from before the label belong to the default home (`~/.cua`).
        let mine = cua_vmm::container::home_tag_for(self.layout.home());
        let default_home = self.layout.home() == cua_vmm::host::home_dir().join(".cua");
        let this_home = |labels: &HashMap<String, String>| match labels.get(docker::LABEL_HOME) {
            Some(h) => *h == mine,
            None => default_home,
        };
        let mut users: HashMap<&str, Vec<String>> = HashMap::new();
        for c in &containers {
            users
                .entry(c.image_id.as_str())
                .or_default()
                .push(c.name.clone());
        }
        // Containers the SDK created.
        for c in &containers {
            let ours = docker::is_managed(&c.labels)
                || c.labels
                    .get(docker::LABEL_SANDBOX)
                    .is_some_and(|v| v == "true");
            if !ours || !this_home(&c.labels) {
                continue;
            }
            let kind = c
                .labels
                .get(docker::LABEL_KIND)
                .cloned()
                .unwrap_or_else(|| "sandbox".into());
            items.push(Item {
                category: if kind == "build" {
                    Category::Builds
                } else {
                    Category::Sandboxes
                },
                kind: format!("container-{kind}"),
                name: c.name.clone(),
                location: c.id.chars().take(12).collect(),
                bytes: c.size_rw,
                created: c
                    .labels
                    .get(docker::LABEL_CREATED)
                    .and_then(|v| v.parse().ok())
                    .or(Some(c.created)),
                status: Some(c.state.clone()),
                ephemeral: c
                    .labels
                    .get(docker::LABEL_EPHEMERAL)
                    .is_some_and(|v| v == "true")
                    || c.name.starts_with("cua-eph-"),
                ..Default::default()
            });
        }
        // Images the SDK wrote (labelled or in its own repositories) or
        // pulled (ledger). Nothing else is listed or touched.
        for i in &images {
            let by = users.get(i.id.as_str()).cloned().unwrap_or_default();
            let ours_repo = i
                .tags
                .iter()
                .any(|t| t.starts_with(docker::MANAGED_REPO_PREFIX));
            let record = records.iter().find(|r| r.image_id == i.id);
            if (docker::is_managed(&i.labels) || ours_repo) && !this_home(&i.labels) {
                continue;
            }
            let kind = if docker::is_managed(&i.labels) {
                i.labels
                    .get(docker::LABEL_KIND)
                    .cloned()
                    .unwrap_or_else(|| "build-image".into())
            } else if ours_repo {
                let t = i
                    .tags
                    .iter()
                    .find(|t| t.starts_with(docker::MANAGED_REPO_PREFIX))
                    .expect("checked");
                let (repo, tag) = t.rsplit_once(':').unwrap_or((t.as_str(), ""));
                cua_vmm::container::image_kind(repo, tag).to_string()
            } else if record.is_some() {
                "pulled-image".into()
            } else {
                continue;
            };
            let checkpoint = kind == "checkpoint";
            // Remove by the SDK's own reference: an image the user also
            // tagged keeps its other names.
            let target = match (&kind[..], record) {
                ("pulled-image", Some(r)) => Target::DockerImage(r.reference.clone()),
                _ => Target::DockerImage(
                    i.tags
                        .iter()
                        .find(|t| t.starts_with(docker::MANAGED_REPO_PREFIX))
                        .cloned()
                        .unwrap_or_else(|| i.id.clone()),
                ),
            };
            let last_used = record
                .and_then(|r| r.last_used)
                .map(unix)
                .or(Some(i.created));
            items.push(Item {
                category: if checkpoint {
                    Category::Checkpoints
                } else {
                    Category::DockerImages
                },
                kind,
                name: i
                    .tags
                    .first()
                    .cloned()
                    .or_else(|| record.map(|r| r.reference.clone()))
                    .unwrap_or_else(|| i.id.chars().take(19).collect()),
                location: i.id.clone(),
                bytes: i.size,
                last_used,
                created: Some(i.created),
                evictable: !checkpoint,
                referenced_by: by,
                target,
                ..Default::default()
            });
        }
        // Ledger records whose image is gone (removed outside the SDK).
        for r in &records {
            if !images.iter().any(|i| i.id == r.image_id) {
                items.push(Item {
                    category: Category::DockerImages,
                    kind: "stale-record".into(),
                    name: r.reference.clone(),
                    location: ledger.dir().display().to_string(),
                    orphan: true,
                    target: Target::Record(ledger.file(&r.reference)),
                    ..Default::default()
                });
            }
        }
        let volumes = d.managed_volumes().await?;
        for v in volumes.into_iter().filter(|v| this_home(&v.labels)) {
            let by: Vec<String> = containers
                .iter()
                .filter(|c| c.volumes.contains(&v.name))
                .map(|c| c.name.clone())
                .collect();
            items.push(Item {
                category: Category::Sandboxes,
                kind: "volume".into(),
                name: v.name.clone(),
                location: v.name.clone(),
                orphan: by.is_empty(),
                referenced_by: by,
                target: Target::DockerVolume(v.name),
                ..Default::default()
            });
        }
        Ok(())
    }

    async fn scan_lume(&self, items: &mut Vec<Item>, notes: &mut Vec<String>) {
        use cua_vmm::lume::{OwnedKind, OwnedVms};
        let owned = OwnedVms::new(self.layout.lume_owned());
        let records = owned.list();
        let mut vms = Vec::new();
        if let Some(l) = &self.lume {
            match l.vms().await {
                Ok(v) => vms = v,
                Err(e) => notes.push(format!("lume: {e}")),
            }
        }
        let listed = self.lume.is_some() && !notes.iter().any(|n| n.starts_with("lume:"));
        for r in records {
            let vm = vms.iter().find(|v| v.name == r.name);
            if vm.is_none() && listed {
                // Deleted outside the SDK: only the record is left.
                items.push(Item {
                    category: Category::LumeBases,
                    kind: "stale-record".into(),
                    name: r.name.clone(),
                    orphan: true,
                    target: Target::Record(owned.dir().join(format!("{}.json", r.name))),
                    ..Default::default()
                });
                continue;
            }
            let status = vm.map(|v| v.status.clone());
            let (category, kind) = match r.kind {
                OwnedKind::Base => (Category::LumeBases, "lume-base"),
                OwnedKind::Instance => (Category::Sandboxes, "lume"),
                OwnedKind::Checkpoint => (Category::Checkpoints, "lume-checkpoint"),
            };
            let busy = status.as_deref().is_some_and(|s| s != "stopped");
            items.push(Item {
                category,
                kind: kind.into(),
                name: r.name.clone(),
                location: r.source.clone().unwrap_or_default(),
                bytes: vm.map(|v| v.allocated).unwrap_or(0)
                    + disk::allocated_size(&self.layout.lume().join(&r.name)),
                last_used: r.last_used.map(unix),
                created: Some(r.created_at),
                status,
                evictable: r.kind == OwnedKind::Base && listed,
                in_progress: r.kind == OwnedKind::Base && busy,
                ephemeral: r.name.starts_with("cua-eph-"),
                target: if r.kind == OwnedKind::Base {
                    Target::LumeVm(r.name.clone())
                } else {
                    Target::None
                },
                ..Default::default()
            });
        }
    }

    fn scan_logs(&self, items: &mut Vec<Item>) {
        let files_in = |d: &Path| -> Vec<PathBuf> {
            std::fs::read_dir(d)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file())
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut files: Vec<PathBuf> = files_in(&self.layout.logs());
        // The Lume installer's /tmp logs, tracked when cua installed Lume.
        if self.layout.lume().join("installed-by-cua").exists() {
            for p in cua_vmm::lume::LUME_DAEMON_LOGS {
                let p = PathBuf::from(p);
                files.extend(disk::logs::rotated(&p, 9));
                if p.exists() {
                    files.push(p);
                }
            }
        }
        for p in [
            self.layout.host().join("driver.log"),
            self.layout.lume().join("serve.log"),
        ] {
            let mut all = vec![p.clone()];
            all.extend(disk::logs::rotated(&p, 9));
            files.extend(all.into_iter().filter(|f| f.exists()));
        }
        for g in self.layout.spacesd_logs() {
            if g.is_dir() {
                files.extend(files_in(&g));
            } else if g.exists() {
                files.push(g.clone());
                files.extend(disk::logs::rotated(&g, 9));
            }
        }
        for f in files {
            items.push(Item {
                category: Category::Logs,
                kind: "log".into(),
                name: f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                location: f.display().to_string(),
                bytes: disk::allocated_size(&f),
                last_used: disk::last_used(&f).map(unix),
                ..Default::default()
            });
        }
    }

    fn scan_data(&self, items: &mut Vec<Item>) {
        let home = self.layout.home();
        let mut push = |category: Category, kind: &str, p: PathBuf| {
            if p.exists() {
                items.push(Item {
                    category,
                    kind: kind.into(),
                    name: p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    location: p.display().to_string(),
                    bytes: disk::allocated_size(&p),
                    last_used: disk::last_used(&p).map(unix),
                    ..Default::default()
                });
            }
        };
        push(Category::Data, "state", self.layout.sandboxes());
        push(Category::Data, "trajectories", self.layout.trajectories());
        push(Category::Data, "skills", self.layout.skills());
        push(Category::Data, "screenshots", self.layout.screenshots());
        push(
            Category::Data,
            "bench-registry",
            self.layout.bench_registry(),
        );
        if let Some(p) = self.layout.bench_runs() {
            push(Category::Data, "bench-runs", p);
        }
        for l in self.layout.legacy() {
            push(Category::Legacy, "legacy", l);
        }
        // Everything else directly under the cua home (credentials, small
        // registries, unknown files from other versions).
        let known = Layout::known_top_level();
        if let Ok(rd) = std::fs::read_dir(home) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if known.contains(&n.as_str()) {
                    continue;
                }
                push(Category::Data, "other", e.path());
            }
        }
        // `cache/` besides the install cache (accounted above).
        if let Ok(rd) = std::fs::read_dir(home.join("cache")) {
            for e in rd.flatten() {
                if e.file_name() != "installables" && e.file_name() != "icons" {
                    push(Category::Data, "other", e.path());
                }
            }
        }
        // `vmm/lume` outside per-VM directories (serve.log is a log).
        let lume = self.layout.lume();
        if let Ok(rd) = std::fs::read_dir(&lume) {
            let owned: BTreeSet<String> = cua_vmm::lume::OwnedVms::new(self.layout.lume_owned())
                .list()
                .into_iter()
                .map(|v| v.name)
                .collect();
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n == "owned" || n.starts_with("serve.log") || owned.contains(&n) {
                    continue;
                }
                push(Category::Sandboxes, "lume-dir", e.path());
            }
        }
    }
}
