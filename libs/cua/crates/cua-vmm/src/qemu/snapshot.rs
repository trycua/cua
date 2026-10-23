//! Memory snapshots of QEMU VMs: warm starts, memory checkpoints and forks
//! of running VMs.
//!
//! A snapshot is QEMU's migration stream written to a file (`migrate` to a
//! `file:` URI) next to a copy of the VM's writable disk layer, its UEFI
//! variables and its guest token. A VM restores it by starting paused with
//! `-incoming defer` and loading the stream (`migrate-incoming`): no boot
//! at all. Works under KVM and HVF alike.
//!
//! Golden snapshots (warm starts): the first start of a configuration
//! (image, CPUs, memory, disk size, environment, command, probes) boots a
//! *template* VM whose RAM is a shared file (`memory-backend-file`,
//! `share=on`) and whose cua-spacesd token is a secret of the golden
//! snapshot's own. Once its probes pass and cloud-init finished, the
//! template's device state is written with `x-ignore-shared` (a few MB:
//! the RAM is the file already) and the template quits. Every VM of that
//! configuration, the first one included, then restores it: guest RAM is
//! the golden RAM file mapped copy-on-write (`share=off`), so restoring
//! costs no copy and every such VM shares the pages it did not write
//! through the host page cache. The restored VM's disk is an overlay on
//! the golden disk layer.
//!
//! Identity: a restored guest is at first a copy of its snapshot's. The
//! runtime's [`GuestIdentity`](crate::GuestIdentity) (cua-spacesd's `Init`)
//! then rotates its token to the VM's own, steps its clock and makes it a
//! new machine (fresh entropy, machine-id and machine-seal key in drivers
//! that implement `new_machine`). Without one, no VM restores memory.
//!
//! Memory checkpoints and forks (fan-out): a memory checkpoint has the
//! golden shape, a RAM file `mem.ram` plus device state written with
//! `x-ignore-shared`, so every fork of it maps `mem.ram` copy-on-write
//! (`share=off`) like a warm start: forking costs no RAM copy and N forks
//! of one world share every page none of them wrote.
//!
//! - A *forkable* VM ([`StartSpec::forkable`], a "bake" VM) runs with its
//!   RAM as `mem.ram` in its own directory, mapped shared. Its checkpoint
//!   pauses it, writes its device state (a few MB), clones `mem.ram` (an
//!   APFS clone or reflink: no copy) and its disk layer, and resumes it.
//! - Any other running VM's checkpoint writes its whole RAM (live under
//!   KVM; paused under HVF, which cannot track dirty pages), copies its
//!   disk layer and resumes it; the stream is then loaded once into a
//!   paused helper QEMU whose RAM is the checkpoint's `mem.ram`, mapped
//!   shared, which writes the device state out with `x-ignore-shared`.
//!
//! A fork of a running VM takes a one-time snapshot instead (a clone of a
//! forkable VM's RAM, or a stream with the RAM restored into anonymous
//! memory). Every restored VM gets its own identity.
//!
//! Pinned checkpoints ([`crate::Runtime::pin_checkpoint`]) are the fork
//! sources to keep: `delete_checkpoint` refuses them until unpinned.
//! Golden snapshots no VM or checkpoint uses are removed least recently
//! used first beyond [`super::QemuConfig::golden_keep`].
//!
//! Layout under the QEMU root: `_golden/<key>/{disk.qcow2, efivars.qcow2,
//! mem.ram, state.mig, guest-token, id_ed25519, meta.json}`; a checkpoint's
//! or fork's snapshot lives in its own entry directory (`mem.ram`,
//! `state.mig`, `efivars.qcow2`, `guest-token`), its disk layer in the
//! shared layer store; a forkable VM's RAM is `mem.ram` in its directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{Boot, QemuRuntime, QemuState, QmpClient, img};
use crate::error::{Result, VmmError};
use crate::host;
use crate::types::*;

/// The snapshot waits (bounded) for the end of cloud-init, so a restored VM
/// resumes after the seed's `runcmd` and nothing restarts its cua-spacesd.
/// An image whose cua-spacesd started before cloud-init's files landed still
/// gets one restart there (see `cloudinit::restart_if_raced`).
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest a migration stream may take to write or load.
const MIGRATE_TIMEOUT: Duration = Duration::from_secs(600);
/// A golden snapshot being recorded (renamed to its key when done).
const REC_PREFIX: &str = ".rec-";
/// A recording older than this was left by a process that died.
const STALE_RECORDING: Duration = Duration::from_secs(3600);
/// The migration stream inside a snapshot directory.
const STATE: &str = "state.mig";
/// Guest RAM as a file: a golden snapshot's, a checkpoint's, or a forkable
/// VM's own (in its entry directory).
pub(crate) const RAM: &str = "mem.ram";
/// The guest token a snapshot's guest accepts (0600).
pub(crate) const GUEST_TOKEN: &str = "guest-token";

/// A memory snapshot a VM can restore.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// Holds `state.mig`, `guest-token`, `disk.qcow2` and `efivars.qcow2`.
    pub dir: PathBuf,
    /// Guest RAM is this file, mapped copy-on-write (golden snapshots); the
    /// stream then holds device state only. `None`: the stream holds RAM.
    #[serde(default)]
    pub ram_file: Option<PathBuf>,
    /// A golden snapshot (shared by every VM of its configuration).
    #[serde(default)]
    pub golden: bool,
    /// Restored once, then its stream is removed (a fork's own snapshot).
    #[serde(default)]
    pub one_shot: bool,
    /// Where the migration stream is when not `dir/state.mig`: a fork's
    /// one-time snapshot staged on tmpfs.
    #[serde(default)]
    pub state_file: Option<PathBuf>,
    /// The disk layer the snapshot's guest saw (default `dir/disk.qcow2`).
    /// A checkpoint's lives in the shared layer store, so forks layered on
    /// it outlive the checkpoint.
    #[serde(default)]
    pub disk: Option<PathBuf>,
    /// The snapshot's VM had a free-page-reporting balloon; VMs restoring
    /// it get the same device set.
    #[serde(default)]
    pub free_page_reporting: bool,
    /// The snapshot's VM had no display or input hardware.
    #[serde(default)]
    pub headless: bool,
    /// The VM that wrote it had the bulk-lane disk, so every VM restoring it
    /// needs that device too (a stream only loads into the device set it was
    /// written from).
    #[serde(default)]
    pub bulk: bool,
}

/// Whether the golden snapshot recorded in `dir` has the bulk-lane disk
/// (older recordings do not).
fn golden_bulk(dir: &Path) -> bool {
    std::fs::read(dir.join("meta.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v["bulk"].as_bool())
        .unwrap_or(false)
}

impl MemorySnapshot {
    pub(crate) fn state(&self) -> PathBuf {
        self.state_file
            .clone()
            .unwrap_or_else(|| self.dir.join(STATE))
    }
    pub fn disk(&self) -> PathBuf {
        self.disk
            .clone()
            .unwrap_or_else(|| self.dir.join("disk.qcow2"))
    }
    fn token(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join(GUEST_TOKEN))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

/// Whether QEMU can migrate a running guest under `accel` (it needs
/// dirty-page tracking from the accelerator).
pub fn live_migration(accel: &str) -> bool {
    // `CUA_QEMU_LIVE_MIGRATION=0` pauses the guest for the whole copy on
    // every accelerator (the HVF way).
    matches!(accel, "kvm" | "tcg")
        && std::env::var("CUA_QEMU_LIVE_MIGRATION").map_or(true, |v| v != "0")
}

/// `n` random bytes from the OS, hex encoded.
fn random_hex(n: usize) -> Result<String> {
    use std::io::Read as _;
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| VmmError::other(format!("no OS randomness for a guest secret: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// A tmpfs file with room for a `bytes` migration stream of VM `name`
/// (Linux `/dev/shm`, keeping a quarter of it free), or `None`.
fn stage_file(name: &str, bytes: u64) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0;
    let base = PathBuf::from(format!("/dev/shm/cua-qemu-{uid}"));
    std::fs::create_dir_all(&base).ok()?;
    let space = crate::disk::space(&base).ok()?;
    (space.available > bytes.saturating_add(space.total / 4))
        .then(|| base.join(format!("{name}-{}.mig", super::unique_suffix())))
}

/// Reads the data of RAM file `path` (skipping holes) so its pages are in
/// the host page cache, where every fork maps them from. A fresh clone's
/// pages are not (the cache holds its source's): without this each fork's
/// first touch of a page is a disk read. Returns the bytes read.
pub(crate) fn prefetch_ram(path: &Path) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut buf = vec![0u8; 8 << 20];
    let mut pos: u64 = 0;
    let mut read: u64 = 0;
    while pos < len {
        // Next data region (`SEEK_DATA`/`SEEK_HOLE`); a filesystem without
        // them reports the whole file as data.
        #[cfg(unix)]
        let (start, end) = {
            use std::os::fd::AsRawFd;
            let fd = f.as_raw_fd();
            let d = unsafe { libc::lseek(fd, pos as libc::off_t, libc::SEEK_DATA) };
            if d < 0 {
                break; // ENXIO: no data past `pos`
            }
            let h = unsafe { libc::lseek(fd, d, libc::SEEK_HOLE) };
            (d as u64, if h < 0 { len } else { h as u64 })
        };
        #[cfg(not(unix))]
        let (start, end) = (pos, len);
        f.seek(SeekFrom::Start(start))?;
        let mut left = end - start;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = f.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            left -= n as u64;
            read += n as u64;
        }
        pos = end.max(start + 1);
    }
    Ok(read)
}

/// Checkpoint RAM files this process has warmed (a fork warms its source's
/// once, in the background, e.g. after the host restarted).
static WARMED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Warms `path` in the background unless this process already did.
pub(crate) fn prefetch_ram_once(path: &Path) {
    {
        let mut w = WARMED.lock().unwrap_or_else(|e| e.into_inner());
        if w.iter().any(|p| p == path) {
            return;
        }
        w.push(path.to_path_buf());
    }
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        match prefetch_ram(&path) {
            Ok(n) => {
                tracing::debug!(ram = %path.display(), mb = n >> 20, ms = t.elapsed().as_millis() as u64, "warmed a checkpoint's RAM file")
            }
            Err(e) => {
                tracing::debug!(ram = %path.display(), error = %e, "could not warm a checkpoint's RAM file")
            }
        }
    });
}

/// Copies `from` to `to` (a clone on APFS, reflink or in-kernel copy on
/// Linux), readable by the owner only when `private`.
fn copy_file(from: &Path, to: &Path) -> Result<()> {
    let _ = std::fs::remove_file(to);
    std::fs::copy(from, to).map_err(|e| {
        VmmError::other(format!("copy {} to {}: {e}", from.display(), to.display()))
    })?;
    Ok(())
}

/// Copies the managed SSH key pair from `from_dir` into `to_dir`.
pub(crate) fn copy_ssh_key(from: &SshAccess, to_dir: &Path) -> Result<SshAccess> {
    let key = to_dir.join("id_ed25519");
    if from.private_key != key {
        let raw = std::fs::read(&from.private_key)?;
        host::write_private(&key, &raw)?;
        let pubk = PathBuf::from(format!("{}.pub", from.private_key.display()));
        if pubk.exists() {
            copy_file(&pubk, &to_dir.join("id_ed25519.pub"))?;
        }
    }
    Ok(SshAccess {
        private_key: key,
        ..from.clone()
    })
}

/// Whether the serial console shows cloud-init's final stage finished.
fn cloud_init_finished(serial: &Path) -> bool {
    std::fs::read(serial)
        .map(|b| {
            let s = String::from_utf8_lossy(&b);
            s.contains("Cloud-init v.") && s.contains(" finished at ")
        })
        .unwrap_or(false)
}

/// Configurations whose image's cua-spacesd cannot become a new machine
/// ([`crate::identity_gate`]): never recorded as a golden in this process,
/// so their VMs boot cold, each with secrets of its own.
static NO_GOLDEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

impl QemuRuntime {
    fn golden_root(&self) -> PathBuf {
        self.cfg.root.join("_golden")
    }

    /// The token a running entry's guest accepts, as recorded at its start.
    pub(crate) fn guest_token(&self, dir: &Path) -> Option<String> {
        std::fs::read_to_string(dir.join(GUEST_TOKEN))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    pub(crate) fn record_guest_token(&self, dir: &Path, token: Option<&str>) -> Result<()> {
        match token {
            Some(t) => host::write_private(&dir.join(GUEST_TOKEN), t.as_bytes())?,
            None => {
                let _ = std::fs::remove_file(dir.join(GUEST_TOKEN));
            }
        }
        Ok(())
    }

    /// Whether VMs restore memory at all (an identity is needed for it).
    pub fn restores_memory(&self) -> bool {
        cfg!(unix) && self.cfg.identity.is_some()
    }

    /// The golden snapshot key of a new instance `st` booting from
    /// `backing`, or `None` when it does not qualify for a warm start: one
    /// with a cua-spacesd token and readiness probes, the managed SSH key,
    /// the generated cloud-init and no GPU, extra arguments or installer.
    pub(crate) async fn golden_key(
        &self,
        st: &QemuState,
        spec: &StartSpec,
        backing: &Path,
    ) -> Option<String> {
        let token = spec.env.get(crate::cloudinit::ENV_TOKEN_VAR);
        // No accelerator this host can use (KVM refused): no warm start.
        let accel = self.accel_for(st.arch, &crate::host::kvm_usable()).ok()?;
        let skip = [
            (!self.cfg.warm, "warm starts are off"),
            (!self.restores_memory(), "no guest identity provider"),
            (spec.os != GuestOs::Linux, "not a Linux guest"),
            (token.is_none_or(|t| t.is_empty()), "no cua-spacesd token"),
            (spec.probes.is_empty(), "no readiness probe"),
            (spec.ssh.is_some(), "caller-provided SSH access"),
            (
                spec.cloud_init_user_data.is_some(),
                "caller-provided cloud-init",
            ),
            (spec.gpu.is_some(), "a GPU"),
            (!spec.extra_args.is_empty(), "extra QEMU arguments"),
            (st.install_iso.is_some(), "an installer ISO"),
            (st.arch != Arch::host(), "an emulated architecture"),
            (accel == "tcg", "no hardware acceleration"),
        ];
        if let Some((_, why)) = skip.iter().find(|(skip, _)| *skip) {
            tracing::debug!(name = st.name, why, "no warm start for this VM");
            return None;
        }
        let bin = self.binary(st.arch).await.ok()?;
        let meta = |p: &Path| {
            std::fs::metadata(p).ok().map(|m| {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("{}:{}:{mtime}", p.display(), m.len())
            })
        };
        let backing = std::fs::canonicalize(backing).unwrap_or_else(|_| backing.to_path_buf());
        let env: Vec<(&String, &String)> = spec
            .env
            .iter()
            .filter(|(k, _)| k.as_str() != crate::cloudinit::ENV_TOKEN_VAR)
            .collect();
        let mut probes: Vec<String> = spec.probes.iter().map(|p| format!("{p:?}")).collect();
        probes.sort();
        let code = match &st.firmware {
            super::Firmware::UefiPflash { code, .. } | super::Firmware::UefiBios { code } => {
                meta(code)
            }
            super::Firmware::Bios => Some("bios".into()),
        };
        let doc = json!({
            "v": 1,
            "backing": meta(&backing)?,
            "qemu": meta(&bin)?,
            "firmware": code,
            "arch": st.arch,
            "accel": accel,
            "cpus": spec.cpus,
            "memory_mb": spec.memory_mb,
            "disk_gb": spec.disk_size_gb,
            "env": env,
            "command": spec.command,
            "probes": probes,
            "restrict": spec.restrict_network,
            // A snapshot only loads into the device set it was written
            // from, so VMs with and without the bulk-lane disk need their
            // own.
            "bulk": crate::bulk::enabled(),
        });
        let h = Sha256::digest(doc.to_string().as_bytes());
        Some(h.iter().take(12).map(|b| format!("{b:02x}")).collect())
    }

    /// The golden snapshot for `key`, recorded from `st`'s configuration
    /// the first time.
    pub(crate) async fn golden(
        &self,
        st: &QemuState,
        spec: &StartSpec,
        backing: &Path,
        key: &str,
    ) -> Result<MemorySnapshot> {
        let dir = self.golden_root().join(key);
        let mut snap = MemorySnapshot {
            ram_file: Some(dir.join(RAM)),
            dir: dir.clone(),
            golden: true,
            one_shot: false,
            disk: None,
            state_file: None,
            free_page_reporting: false,
            // The template's (`record_golden`): the key covers the env.
            headless: super::no_desktop(spec),
            bulk: false,
        };
        let refused = || {
            NO_GOLDEN
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|k| k == key)
        };
        let no_golden = || {
            Err(VmmError::other(
                "this image's cua-spacesd cannot become a new machine; booting cold",
            ))
        };
        if refused() {
            return no_golden();
        }
        let _g = self.golden_lock.lock().await;
        // Again under the lock: VMs of one configuration started at once all
        // waited here, and each recorded (and was refused) in turn, about
        // 18 s apiece (16 at once took 300 s).
        if refused() {
            return no_golden();
        }
        if !dir.join("meta.json").is_file() {
            let started = std::time::Instant::now();
            self.record_golden(st, spec, backing, &dir).await?;
            tracing::info!(
                key,
                ms = started.elapsed().as_millis() as u64,
                "recorded a golden QEMU snapshot"
            );
            self.gc_golden();
        }
        crate::disk::mark_used(&dir);
        // Read once the snapshot exists: a recording just made says what it
        // holds.
        snap.bulk = golden_bulk(&dir);
        Ok(snap)
    }

    /// Boots a template VM of `st`'s configuration with a golden secret as
    /// its token, waits until it settled and writes its golden snapshot to
    /// `target`.
    async fn record_golden(
        &self,
        st: &QemuState,
        spec: &StartSpec,
        backing: &Path,
        target: &Path,
    ) -> Result<()> {
        let root = self.golden_root();
        std::fs::create_dir_all(&root)?;
        crate::disk::ensure_space(
            &root,
            spec.memory_mb.saturating_mul(1 << 20) + crate::disk::VM_CREATE_ESTIMATE,
            "record a golden VM snapshot",
        )?;
        Self::clean_stale_recordings(&root);
        let tmp = root.join(format!("{REC_PREFIX}{}", super::unique_suffix()));
        std::fs::create_dir_all(&tmp)?;
        let mut tpl = QemuState {
            disk: tmp.join("disk.qcow2"),
            seed_iso: None,
            firmware: self.copy_vars(&st.firmware, &tmp)?,
            ports: Default::default(),
            vnc_display: None,
            qmp_port: None,
            pid: None,
            snapshots: vec![],
            paused: false,
            restore_from: None,
            memory: None,
            last_start: None,
            // The recording boots the image before any Space has it (no
            // identity, no workload): the Space's own run is the one its
            // policy applies to, from its restore on.
            egress: None,
            netns: false,
            pasta_pid: None,
            shared_memory: false,
            pinned: false,
            ..st.clone()
        };
        let res: Result<()> = async {
            img::create_overlay(backing, &tpl.disk).await?;
            if let Some(gb) = spec.disk_size_gb {
                img::grow_to(&tpl.disk, gb).await?;
            }
            let ssh = crate::cloudinit::managed_ssh(&tmp)
                .await
                .ok_or_else(|| VmmError::other("no managed SSH key for the template"))?;
            // Same length as a Space token, so the seed has the same shape.
            let secret = random_hex(16)?;
            let mut tspec = spec.clone();
            tspec
                .env
                .insert(crate::cloudinit::ENV_TOKEN_VAR.to_string(), secret.clone());
            tspec.ssh = Some(ssh.clone());
            tspec.pool_member = false;
            tpl.ssh = Some(ssh);
            // The hostname every restored VM starts with: neutral, not the
            // first Space's name.
            if let Some(seed) = crate::cloudinit::seed_for_spec("cua", &tspec)? {
                let p = tmp.join("seed.iso");
                seed.write_iso(&p)?;
                tpl.seed_iso = Some(p);
            }
            host::write_private(&tmp.join(GUEST_TOKEN), secret.as_bytes())?;
            let boot = Boot {
                memory: super::Memory::File {
                    path: tmp.join(RAM),
                    shared: true,
                },
                incoming: false,
                keep_ports: false,
                // Never over a RAM file (see `LaunchConfig`); so neither
                // do the VMs restoring it.
                free_page_reporting: Some(false),
                headless: Some(super::no_desktop(&tspec)),
            };
            self.launch(&mut tpl, &tspec, boot, false).await?;
            let ep = self.endpoints_of(&tpl);
            self.wait_probes(&tpl, &ep, &tspec).await?;
            // cloud-init's last stage may restart a cua-spacesd that raced
            // it (never one ordered after cloud-init); snapshot after it,
            // with the guest serving again.
            let deadline = tokio::time::Instant::now() + SETTLE_TIMEOUT;
            let serial = tmp.join("serial.log");
            while !cloud_init_finished(&serial) && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.wait_probes(&tpl, &ep, &tspec).await?;
            // Every VM restored from it would share its cua-spacesd's
            // per-machine secrets unless that cua-spacesd renews them
            // ([`crate::identity_gate`]).
            if let Some(port) = ep.ports.get(&crate::identity_gate::SPACESD_PORT) {
                let rev = crate::identity_gate::revision_at(
                    &format!("http://127.0.0.1:{port}"),
                    Some(&secret),
                )
                .await;
                if !crate::identity_gate::renews_identity(rev) {
                    if let Some(key) = target.file_name() {
                        NO_GOLDEN
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(key.to_string_lossy().into_owned());
                    }
                    return Err(VmmError::other(format!(
                        "this image's cua-spacesd (protocol revision {}) cannot become a new \
                         machine, so its VMs boot cold rather than share one snapshot's keys",
                        rev.map_or("unknown".to_string(), |r| r.to_string())
                    )));
                }
            }
            let mut q = self.qmp_at(&tpl).await?;
            q.stop().await?;
            q.set_migrate_capabilities(&[("x-ignore-shared", true)])
                .await?;
            q.migrate_to_file(&tmp.join(STATE), MIGRATE_TIMEOUT).await?;
            let _ = q.quit().await;
            if let Some(pid) = tpl.pid
                && !Self::wait_exit(pid, Duration::from_secs(10)).await
            {
                host::signal(pid, "KILL");
                Self::wait_exit(pid, Duration::from_secs(5)).await;
            }
            tpl.pid = None;
            let _ = std::fs::remove_file(tmp.join("seed.iso"));
            std::fs::write(
                tmp.join("meta.json"),
                serde_json::to_vec_pretty(&json!({
                    "created_at": host::now_secs(),
                    "cpus": spec.cpus,
                    "memory_mb": spec.memory_mb,
                    "source": st.source,
                    "ssh": tpl.ssh,
                    "bulk": tpl.bulk,
                }))?,
            )?;
            if target.exists() {
                // Another process finished first: keep its snapshot.
                let _ = std::fs::remove_dir_all(&tmp);
            } else {
                std::fs::rename(&tmp, target)?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            let _ = self.force_stop(&mut tpl, Duration::ZERO).await;
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        Ok(())
    }

    /// Removes recordings a process that died left behind (an hour old),
    /// stopping their template VM if it still runs.
    fn clean_stale_recordings(root: &Path) {
        let Ok(rd) = std::fs::read_dir(root) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let stale = e.file_name().to_string_lossy().starts_with(REC_PREFIX)
                && std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > STALE_RECORDING);
            if !stale {
                continue;
            }
            if let Some(pid) = std::fs::read_to_string(p.join("qemu.pid"))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|pid| host::pid_alive(*pid))
                // Only the template VM itself (a pid may have been reused).
                .filter(|pid| {
                    std::process::Command::new("ps")
                        .args(["-o", "command=", "-p", &pid.to_string()])
                        .output()
                        .is_ok_and(|o| {
                            String::from_utf8_lossy(&o.stdout).contains(&p.display().to_string())
                        })
                })
            {
                host::signal(pid, "KILL");
            }
            tracing::debug!(dir = %p.display(), "removing a stale golden recording");
            let _ = std::fs::remove_dir_all(&p);
        }
    }

    /// A QMP session to `st` (which need not be a saved entry).
    pub(crate) async fn qmp_at(&self, st: &QemuState) -> Result<QmpClient> {
        let port = st
            .qmp_port
            .ok_or_else(|| VmmError::other("no QMP port recorded"))?;
        QmpClient::connect(&format!("127.0.0.1:{port}")).await
    }

    /// Makes new instance `st` a restore of golden `snap`: its disk becomes
    /// an overlay on the golden disk, and it takes the golden UEFI
    /// variables and SSH key.
    pub(crate) async fn adopt_snapshot(
        &self,
        st: &mut QemuState,
        snap: &MemorySnapshot,
    ) -> Result<()> {
        let dir = self.dir(&st.name);
        let _ = std::fs::remove_file(&st.disk);
        img::create_overlay(&snap.disk(), &st.disk).await?;
        if let super::Firmware::UefiPflash { vars, .. } = &st.firmware {
            let golden = snap.dir.join("efivars.qcow2");
            if golden.exists() {
                copy_file(&golden, vars)?;
            }
        }
        if let Some(ssh) = self.snapshot_ssh(snap) {
            st.ssh = Some(copy_ssh_key(&ssh, &dir)?);
        }
        st.restore_from = Some(snap.clone());
        self.save(st)
    }

    fn snapshot_ssh(&self, snap: &MemorySnapshot) -> Option<SshAccess> {
        let key = snap.dir.join("id_ed25519");
        key.exists().then(|| SshAccess {
            user: "cua".into(),
            private_key: key,
            password: None,
        })
    }

    /// Restores `snap` into stopped instance `st` (whose disk already sits
    /// on the snapshot's) and gives the guest its identity: the token in
    /// `spec.env`, the host's clock, fresh entropy.
    pub(crate) async fn restore_memory(
        &self,
        st: &mut QemuState,
        spec: &StartSpec,
        snap: &MemorySnapshot,
    ) -> Result<()> {
        let identity = self.cfg.identity.clone().ok_or(VmmError::Unsupported {
            backend: "qemu",
            op: "restoring memory without a guest identity provider",
        })?;
        let current = snap.token().ok_or_else(|| {
            VmmError::other(format!(
                "the snapshot in {} has no guest token",
                snap.dir.display()
            ))
        })?;
        let started = std::time::Instant::now();
        // The snapshot's device set, with or without the bulk-lane disk.
        st.bulk = snap.bulk;
        // The snapshot's shape, whatever the caller asked for: a stream
        // only loads into the machine it was written from.
        let mut spec = spec.clone();
        spec.cpus = st.cpus;
        spec.memory_mb = st.memory_mb;
        let spec = &spec;
        let memory = if st.shared_memory {
            // A forkable VM's RAM is its own file, mapped shared: a clone of
            // the snapshot's RAM file, or one the stream's RAM loads into.
            let own = self.dir(&st.name).join(RAM);
            match &snap.ram_file {
                Some(f) => copy_file(f, &own)?,
                None => {
                    let _ = std::fs::remove_file(&own);
                }
            }
            super::Memory::File {
                path: own,
                shared: true,
            }
        } else {
            match &snap.ram_file {
                Some(f) => super::Memory::File {
                    path: f.clone(),
                    shared: false,
                },
                None => super::Memory::Ram,
            }
        };
        let boot = Boot {
            memory,
            incoming: true,
            keep_ports: false,
            free_page_reporting: Some(snap.free_page_reporting),
            headless: Some(snap.headless),
        };
        self.launch(st, spec, boot, true).await?;
        let t_launch = started.elapsed();
        let mut q = self.qmp_at(st).await?;
        q.set_migrate_capabilities(&[("x-ignore-shared", snap.ram_file.is_some())])
            .await?;
        q.migrate_from_file(&snap.state(), MIGRATE_TIMEOUT).await?;
        q.cont().await?;
        let t_load = started.elapsed();
        let ep = self.endpoints_of(st);
        let token = spec
            .env
            .get(crate::cloudinit::ENV_TOKEN_VAR)
            .filter(|t| !t.is_empty())
            .cloned();
        let id = ClaimIdentity {
            token: token.clone(),
            hostname: Some(st.name.clone()),
        };
        identity
            .renew(&ep, &current, &id, true, spec.ready_timeout)
            .await?;
        let dir = self.dir(&st.name);
        self.record_guest_token(&dir, Some(token.as_deref().unwrap_or(&current)))?;
        let t_identity = started.elapsed();
        if snap.one_shot {
            let _ = std::fs::remove_file(snap.state());
        }
        st.restore_from = None;
        st.last_start = Some(if snap.golden { "warm" } else { "restore" }.into());
        self.save(st)?;
        tracing::info!(
            name = st.name,
            how = st.last_start.as_deref().unwrap_or_default(),
            launch_ms = t_launch.as_millis() as u64,
            load_ms = (t_load - t_launch).as_millis() as u64,
            identity_ms = (t_identity - t_load).as_millis() as u64,
            "QEMU VM restored from memory"
        );
        Ok(())
    }

    /// Writes running `st`'s full state into `target` (created): its RAM
    /// and devices, a copy of its disk layer and UEFI variables, its SSH
    /// key and the token its guest accepts. The VM is paused meanwhile and
    /// resumed after unless it was paused.
    pub(crate) async fn save_memory(
        &self,
        st: &QemuState,
        target: &Path,
        one_shot: bool,
    ) -> Result<(MemorySnapshot, super::Firmware, Option<SshAccess>)> {
        // A fork's one-time snapshot keeps its disk layer; a checkpoint's
        // goes to the layer store its forks share.
        let disk = if one_shot {
            target.join("disk.qcow2")
        } else {
            let layers = self.layers_dir();
            std::fs::create_dir_all(&layers)?;
            layers.join(format!("{}-mem-{}.qcow2", st.name, super::unique_suffix()))
        };
        let src_dir = self.dir(&st.name);
        // A forkable VM's RAM is a shared file: cloned, not written out.
        let own_ram = src_dir.join(RAM);
        let clone_ram = st.shared_memory && own_ram.is_file();
        let token = self
            .guest_token(&src_dir)
            .ok_or_else(|| no_guest_token(&st.name))?;
        crate::disk::ensure_space(
            &self.cfg.root,
            if clone_ram {
                0
            } else {
                st.memory_mb.saturating_mul(1 << 20)
            } + std::fs::metadata(&st.disk).map(|m| m.len()).unwrap_or(0),
            &format!("snapshot {}", st.name),
        )?;
        std::fs::create_dir_all(target)?;
        // A fork's one-time stream is read once, right away: on tmpfs it is
        // a memory copy instead of a disk write and read.
        let state = match one_shot && !clone_ram {
            true => stage_file(&st.name, st.memory_mb << 20),
            false => None,
        };
        let state_path = state.clone().unwrap_or_else(|| target.join(STATE));
        let res: Result<_> = async {
            let mut q = self.qmp_at(st).await?;
            // A plain stream: `mapped-ram` (fixed offsets per page) writes
            // 4 GiB sparse files page by page, 7x slower on APFS (0.7 s
            // vs 4.7 s for a 4 GiB guest, QEMU 10.2), and buys nothing
            // without parallel loads.
            q.set_migrate_capabilities(&[("x-ignore-shared", clone_ram)])
                .await?;
            // Live where the accelerator tracks dirty pages (KVM): RAM is
            // copied while the guest runs and QEMU pauses it only for the
            // last dirty pages (it stays paused, `postmigrate`). HVF has no
            // dirty-page tracking (QEMU 10.2 aborts in
            // `hvf_handle_exception` on the first write-protect fault), so
            // there the guest pauses for the whole copy.
            //
            // A forkable VM is paused for its device state (milliseconds)
            // and the clone of its RAM file, which must be of that moment.
            if !st.paused && (clone_ram || !live_migration(&st.accel)) {
                q.stop().await?;
            }
            // QEMU cannot release the disks (`bdrv_inactivate_all`) while the
            // guest has requests in flight, as a guest that has only just
            // booted does: the stream is written again a moment later.
            let mut written = q.migrate_to_file(&state_path, MIGRATE_TIMEOUT).await;
            for attempt in 1..=3 {
                match &written {
                    Err(e) if e.to_string().contains("bdrv_inactivate_all") => {
                        tracing::debug!(name = st.name, attempt, "disks busy at the end of a snapshot; trying again");
                        if q.status().await? != "running" {
                            q.cont().await?;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        written = q.migrate_to_file(&state_path, MIGRATE_TIMEOUT).await;
                    }
                    _ => break,
                }
            }
            // The disk is consistent (flushed and released) while the VM
            // is stopped after a completed migration.
            let copied = match &written {
                Ok(()) => (if clone_ram {
                    copy_file(&own_ram, &target.join(RAM))
                } else {
                    Ok(())
                })
                .and_then(|()| copy_file(&st.disk, &disk))
                .and_then(|()| self.copy_vars(&st.firmware, target)),
                Err(_) => Err(VmmError::other("not copied")),
            };
            if !st.paused && q.status().await? != "running" {
                q.cont().await?;
                // A paused guest's clock stood still.
                if let Some(identity) = &self.cfg.identity
                    && let Err(e) = identity
                        .resync_clock(&self.endpoints_of(st), &token)
                        .await
                {
                    tracing::warn!(name = st.name, error = %e, "could not step the guest clock after a snapshot");
                }
            }
            written?;
            let firmware = copied?;
            if clone_ram {
                // After the VM resumed: its forks read the clone's pages.
                let ram = target.join(RAM);
                let t = std::time::Instant::now();
                let n = tokio::task::spawn_blocking(move || prefetch_ram(&ram))
                    .await
                    .map_err(|e| VmmError::other(e.to_string()))??;
                if let Ok(mut w) = WARMED.lock() {
                    w.push(target.join(RAM));
                }
                tracing::debug!(mb = n >> 20, ms = t.elapsed().as_millis() as u64, "warmed the checkpoint's RAM file");
            }
            host::write_private(&target.join(GUEST_TOKEN), token.as_bytes())?;
            let ssh = match &st.ssh {
                Some(s) => Some(copy_ssh_key(s, target)?),
                None => None,
            };
            Ok((
                MemorySnapshot {
                    dir: target.to_path_buf(),
                    ram_file: clone_ram.then(|| target.join(RAM)),
                    golden: false,
                    one_shot,
                    disk: Some(disk.clone()),
                    state_file: state.clone(),
                    free_page_reporting: st.free_page_reporting,
                    headless: st.headless,
                    bulk: st.bulk,
                },
                firmware,
                ssh,
            ))
        }
        .await;
        if res.is_err() {
            let _ = std::fs::remove_dir_all(target);
            let _ = std::fs::remove_file(&disk);
            let _ = std::fs::remove_file(&state_path);
        }
        res
    }

    /// Moves running `st` into a new QEMU process with the same disk, ports
    /// and memory: paused, its state streamed to a file, the old process
    /// ended, a new one launched (inside the namespace its egress policy
    /// needs) and loaded, and resumed unless it was paused. Its guest
    /// keeps running where it was; connections through the old process
    /// (the user-mode network's) are gone. A VM that cannot be moved is
    /// left stopped, never running outside its policy.
    pub(crate) async fn relaunch(&self, st: &mut QemuState) -> Result<()> {
        let was_paused = st.paused;
        let dir = self.dir(&st.name);
        let state =
            stage_file(&st.name, st.memory_mb << 20).unwrap_or_else(|| dir.join("relaunch.mig"));
        let saved: Result<()> = async {
            let mut q = self.qmp_at(st).await?;
            if !was_paused {
                q.stop().await?;
            }
            q.set_migrate_capabilities(&[("x-ignore-shared", false)])
                .await?;
            q.migrate_to_file(&state, MIGRATE_TIMEOUT).await
        }
        .await;
        if let Err(e) = saved {
            let _ = std::fs::remove_file(&state);
            if !was_paused && let Ok(mut q) = self.qmp_at(st).await {
                let _ = q.cont().await;
            }
            return Err(e);
        }
        self.force_stop(st, Duration::ZERO).await?;
        let mut spec = StartSpec::new(&st.name, ImageSource::Existing)
            .cpus(st.cpus)
            .memory_mb(st.memory_mb);
        spec.ports = st.ports.keys().copied().filter(|p| *p != 22).collect();
        spec.restrict_network = st.restrict_network;
        let boot = Boot {
            // A forkable VM keeps its own RAM file; others anonymous memory.
            memory: self.own_memory(st, false)?,
            incoming: true,
            keep_ports: true,
            // The same VM: the device set it runs with.
            free_page_reporting: Some(st.free_page_reporting),
            headless: Some(st.headless),
        };
        let moved: Result<()> = async {
            self.launch(st, &spec, boot, true).await?;
            let mut q = self.qmp_at(st).await?;
            q.migrate_from_file(&state, MIGRATE_TIMEOUT).await?;
            if !was_paused {
                q.cont().await?;
            }
            Ok(())
        }
        .await;
        let _ = std::fs::remove_file(&state);
        if let Err(e) = moved {
            let _ = self.force_stop(st, Duration::ZERO).await;
            st.paused = false;
            let _ = self.save(st);
            return Err(VmmError::other(format!(
                "{} could not be moved into its network namespace and is stopped (start it \
                 again): {e}",
                st.name
            )));
        }
        st.paused = was_paused;
        // The guest's clock stood still while it moved.
        if !was_paused
            && let (Some(identity), Some(token)) = (&self.cfg.identity, self.guest_token(&dir))
            && let Err(e) = identity.resync_clock(&self.endpoints_of(st), &token).await
        {
            tracing::warn!(name = st.name, error = %e, "could not step the guest clock after a move");
        }
        self.save(st)
    }

    /// Guest RAM of entry `st` when it boots: its own `mem.ram`, mapped
    /// shared, for a forkable VM (emptied first when `fresh`), else
    /// anonymous memory.
    pub(crate) fn own_memory(&self, st: &QemuState, fresh: bool) -> Result<super::Memory> {
        if !st.shared_memory {
            return Ok(super::Memory::Ram);
        }
        let path = self.dir(&st.name).join(RAM);
        if fresh {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(super::Memory::File { path, shared: true })
    }

    /// Turns checkpoint `ck`'s memory snapshot `snap`, a stream that holds
    /// the RAM, into the shape its forks share: the stream is loaded into a
    /// paused helper QEMU whose RAM is `mem.ram` next to it, mapped shared,
    /// and the helper writes its device state out with `x-ignore-shared`
    /// in place of the stream. The guest never runs in the helper. `seed`:
    /// the source VM's cloud-init seed, attached again so the helper has
    /// the source's devices.
    pub(crate) async fn share_ram(
        &self,
        ck: &QemuState,
        snap: &MemorySnapshot,
        seed: Option<&Path>,
    ) -> Result<MemorySnapshot> {
        let started = std::time::Instant::now();
        let tmp = snap.dir.join(".share");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let ram = snap.dir.join(RAM);
        let _ = std::fs::remove_file(&ram);
        let out = tmp.join(STATE);
        let mut helper = QemuState {
            kind: super::EntryKind::Instance,
            disk: tmp.join("disk.qcow2"),
            seed_iso: None,
            firmware: self.copy_vars(&ck.firmware, &tmp)?,
            ports: Default::default(),
            vnc_display: None,
            qmp_port: None,
            pid: None,
            snapshots: vec![],
            paused: false,
            restore_from: None,
            memory: None,
            last_start: None,
            shared_memory: false,
            pinned: false,
            // The checkpoint's device set (the bulk-lane disk).
            bulk: snap.bulk,
            ..ck.clone()
        };
        let res: Result<()> = async {
            img::create_overlay(&snap.disk(), &helper.disk).await?;
            if let Some(seed) = seed.filter(|p| p.is_file()) {
                let p = tmp.join("seed.iso");
                copy_file(seed, &p)?;
                helper.seed_iso = Some(p);
            }
            let mut spec = StartSpec::new(helper.name.clone(), ImageSource::Existing);
            spec.os = helper.os;
            spec.cpus = helper.cpus;
            spec.memory_mb = helper.memory_mb;
            spec.restrict_network = helper.restrict_network;
            spec.gpu = helper.gpu.clone();
            spec.extra_args = helper.extra_args.clone();
            let boot = Boot {
                memory: super::Memory::File {
                    path: ram.clone(),
                    shared: true,
                },
                incoming: true,
                // The snapshot's device set, as for any restore of it.
                free_page_reporting: Some(snap.free_page_reporting),
                headless: Some(snap.headless),
                keep_ports: false,
            };
            self.launch(&mut helper, &spec, boot, false).await?;
            let mut q = self.qmp_at(&helper).await?;
            q.set_migrate_capabilities(&[("x-ignore-shared", false)])
                .await?;
            q.migrate_from_file(&snap.state(), MIGRATE_TIMEOUT).await?;
            q.set_migrate_capabilities(&[("x-ignore-shared", true)])
                .await?;
            q.migrate_to_file(&out, MIGRATE_TIMEOUT).await?;
            let _ = q.quit().await;
            if let Some(pid) = helper.pid
                && !Self::wait_exit(pid, Duration::from_secs(10)).await
            {
                host::signal(pid, "KILL");
                Self::wait_exit(pid, Duration::from_secs(5)).await;
            }
            helper.pid = None;
            std::fs::rename(&out, snap.dir.join(STATE))?;
            Ok(())
        }
        .await;
        let _ = self.force_stop(&mut helper, Duration::ZERO).await;
        let log = std::fs::read_to_string(tmp.join("qemu.log")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = res {
            let _ = std::fs::remove_file(&ram);
            return Err(VmmError::other(format!(
                "could not load checkpoint {}'s RAM into a file its forks share: {e}{}",
                ck.name,
                match log.trim() {
                    "" => String::new(),
                    l => format!("\nQEMU: {}", crate::exec::tail(l, 2000)),
                }
            )));
        }
        tracing::info!(
            checkpoint = ck.name,
            ms = started.elapsed().as_millis() as u64,
            "loaded a checkpoint's RAM into a file its forks share"
        );
        Ok(MemorySnapshot {
            ram_file: Some(ram),
            state_file: None,
            dir: snap.dir.clone(),
            ..snap.clone()
        })
    }

    /// Removes golden snapshots no entry uses beyond the
    /// [`super::QemuConfig::golden_keep`] most recently used.
    fn gc_golden(&self) {
        let Ok(rd) = std::fs::read_dir(self.golden_root()) else {
            return;
        };
        let mut all: Vec<(std::time::SystemTime, PathBuf)> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("meta.json").is_file())
            .map(|p| {
                let t = crate::disk::last_used(&p).unwrap_or(std::time::UNIX_EPOCH);
                (t, p)
            })
            .collect();
        let used = self.golden_in_use();
        // Goldens in use neither go nor count against the budget.
        all.retain(|(_, p)| !used.iter().any(|u| u.starts_with(p)));
        all.sort_by_key(|x| std::cmp::Reverse(x.0));
        for (_, p) in all.into_iter().skip(self.cfg.golden_keep) {
            tracing::debug!(golden = %p.display(), "removing a golden QEMU snapshot");
            let _ = std::fs::remove_dir_all(&p);
        }
    }

    /// Files under the golden root that some entry uses (its disk's
    /// backing, or a pending restore).
    fn golden_in_use(&self) -> Vec<PathBuf> {
        let root = self.golden_root();
        let mut used = Vec::new();
        for st in self.all_states().unwrap_or_default() {
            if let Some(s) = &st.restore_from {
                used.push(s.dir.clone());
            }
            // Overlays name their backing by absolute path: a cheap header
            // read instead of `qemu-img info` per entry.
            for f in super::backing_files(&st.disk) {
                if f.starts_with(&root) {
                    used.push(f);
                }
            }
        }
        used
    }

    /// Drops every golden snapshot no VM uses (the next start of each
    /// configuration records it again).
    pub fn clear_golden(&self) -> Result<()> {
        let used = self.golden_in_use();
        let Ok(rd) = std::fs::read_dir(self.golden_root()) else {
            return Ok(());
        };
        for e in rd.flatten() {
            let p = e.path();
            if !used.iter().any(|u| u.starts_with(&p)) {
                std::fs::remove_dir_all(&p)?;
            }
        }
        Ok(())
    }

    /// Removes golden snapshot `snap` (one that failed to restore).
    pub(crate) fn drop_golden(&self, snap: &MemorySnapshot) {
        if snap.golden && snap.dir.starts_with(self.golden_root()) {
            let _ = std::fs::remove_dir_all(&snap.dir);
        }
    }
}

/// The error for a VM with no recorded guest token: it was created without
/// cua-spacesd, or by a Cua from before checkpoints, whose QEMU process cannot
/// save its memory in a form a copy can load (checked on a real 0.7.0 VM). A
/// QEMU VM cannot be restarted (it resumes from suspend only), so recreating
/// it is the way forward.
pub(crate) fn no_guest_token(name: &str) -> VmmError {
    VmmError::invalid(format!(
        "'{name}' has no recorded guest identity (it was created without cua-spacesd, or by a \
         Cua older than checkpoints), so its memory cannot be copied: recreate it with \
         `cua sb rm {name}` and `cua sb create`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_init_marker() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("serial.log");
        assert!(!cloud_init_finished(&p));
        std::fs::write(
            &p,
            "[  9.7] cloud-init[603]: Cloud-init v. 26.1 running 'modules:final'\n",
        )
        .unwrap();
        assert!(!cloud_init_finished(&p));
        std::fs::write(
            &p,
            "[  9.7] cloud-init[603]: Cloud-init v. 26.1 finished at Tue, 06 Oct 2026 16:26:47 +0000\n",
        )
        .unwrap();
        assert!(cloud_init_finished(&p));
    }

    #[test]
    fn missing_guest_token_says_what_to_do() {
        let msg = no_guest_token("dev").to_string();
        assert!(msg.contains("'dev'"), "{msg}");
        assert!(msg.contains("cua sb rm dev"), "{msg}");
        assert!(msg.contains("recreate"), "{msg}");
        assert!(
            !msg.contains("restart"),
            "a QEMU VM cannot be restarted: {msg}"
        );
    }

    #[test]
    fn live_snapshots_only_where_dirty_pages_are_tracked() {
        assert!(live_migration("kvm"));
        assert!(!live_migration("hvf"));
        assert!(!live_migration("whpx"));
    }

    fn rt(root: &Path, golden_keep: usize) -> QemuRuntime {
        QemuRuntime::new(super::super::QemuConfig {
            root: root.to_path_buf(),
            golden_keep,
            ..Default::default()
        })
    }

    fn entry(name: &str, disk: PathBuf) -> QemuState {
        QemuState {
            name: name.into(),
            kind: super::super::EntryKind::Instance,
            arch: Arch::X86_64,
            os: GuestOs::Linux,
            disk,
            disk_format: "qcow2".into(),
            install_iso: None,
            seed_iso: None,
            firmware: super::super::Firmware::Bios,
            cpus: 1,
            memory_mb: 256,
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
            created_at: 0,
            gpu: None,
            restore_from: None,
            memory: None,
            last_start: None,
            shared_memory: false,
            pinned: false,
            free_page_reporting: false,
            ram_file: None,
            headless: false,
            bulk: false,
            egress: None,
            netns: false,
            pasta_pid: None,
        }
    }

    #[test]
    fn a_forkable_vm_runs_on_its_own_shared_ram_file() {
        let d = tempfile::tempdir().unwrap();
        let r = rt(d.path(), 3);
        let mut st = entry("vm", d.path().join("vm").join("disk.qcow2"));
        assert_eq!(r.own_memory(&st, true).unwrap(), super::super::Memory::Ram);
        st.shared_memory = true;
        let ram = d.path().join("vm").join(RAM);
        std::fs::create_dir_all(ram.parent().unwrap()).unwrap();
        std::fs::write(&ram, b"old guest").unwrap();
        let want = super::super::Memory::File {
            path: ram.clone(),
            shared: true,
        };
        // A restart keeps nothing of the last run's RAM; a restore keeps it.
        assert_eq!(r.own_memory(&st, false).unwrap(), want);
        assert!(ram.exists());
        assert_eq!(r.own_memory(&st, true).unwrap(), want);
        assert!(!ram.exists());
    }

    #[test]
    fn golden_gc_keeps_those_in_use_and_the_newest_unused() {
        let d = tempfile::tempdir().unwrap();
        let r = rt(d.path(), 1);
        let gold = |k: &str, age: u64| {
            let p = d.path().join("_golden").join(k);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("meta.json"), b"{}").unwrap();
            std::fs::write(p.join("disk.qcow2"), b"").unwrap();
            crate::disk::mark_used_at(&p, std::time::SystemTime::now() - Duration::from_secs(age));
            p
        };
        let used_old = gold("aaa", 4000);
        let newest = gold("bbb", 10);
        let older = gold("ccc", 2000);
        // An entry about to restore the oldest golden.
        let mut vm = entry("vm", d.path().join("vm").join("disk.qcow2"));
        vm.restore_from = Some(MemorySnapshot {
            dir: used_old.clone(),
            ram_file: Some(used_old.join(RAM)),
            golden: true,
            one_shot: false,
            state_file: None,
            disk: None,
            free_page_reporting: false,
            headless: false,
            bulk: false,
        });
        std::fs::create_dir_all(d.path().join("vm")).unwrap();
        std::fs::write(
            d.path().join("vm").join("state.json"),
            serde_json::to_vec(&vm).unwrap(),
        )
        .unwrap();
        r.gc_golden();
        assert!(used_old.exists(), "in use: kept, outside the budget");
        assert!(
            newest.exists(),
            "the newest unused one is within the budget"
        );
        assert!(!older.exists(), "beyond the budget");
    }

    #[test]
    fn secrets_are_random_hex() {
        let a = random_hex(16).unwrap();
        let b = random_hex(16).unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
