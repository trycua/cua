//! What a local Space costs this computer's disk, as opposed to what the
//! guest sees, and what it was given (its allocation).
//!
//! A macOS Space is an APFS clone of its base image: its files share blocks
//! with other copies, so `st_blocks` counts the shared blocks again for each
//! copy. [`private_size`] asks APFS for the bytes only that file holds
//! (`ATTR_CMNEXT_PRIVATESIZE`), which is what deleting it frees. Where that
//! cannot be measured the number is left out and [`DiskOnHost::shares_space`]
//! says the Space shares with other copies.
//!
//! A container Space costs what its own layer holds: the writable layer the
//! engine keeps for it (and its sidecars) plus the anonymous volumes that
//! go with it ([`container_disk`]); the image it runs is shared and stays.
//! A microVM's instance directory holds its own scratch disk and state; its
//! base image lives apart.

use super::{DiskOnHost, MIB};
use cua_sandbox_core::state::{SandboxState, StateStore};
use cua_vmm::disk::allocated_size;
use std::path::{Path, PathBuf};

/// Allocated blocks beyond the private bytes that still count as "shares
/// space with other copies" (a few metadata blocks do not).
pub const SHARED_SLACK: u64 = 16 * MIB;

/// The bytes only one file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Private {
    /// Measured: the bytes only this file holds.
    Bytes(u64),
    /// The volume cannot share blocks between files (not APFS), so the
    /// file's allocated blocks are its own.
    NotShared,
    /// It could not be measured (permissions, a vanished file).
    Unknown,
}

/// The bytes only `path` (a file) holds: `ATTR_CMNEXT_PRIVATESIZE` on APFS.
/// Off macOS no file system here shares blocks between files, so the answer
/// is [`Private::NotShared`].
pub fn private_size(path: &Path) -> Private {
    #[cfg(target_os = "macos")]
    {
        macos_private_size(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Private::NotShared
    }
}

#[cfg(target_os = "macos")]
fn macos_private_size(path: &Path) -> Private {
    use std::os::unix::ffi::OsStrExt;
    // sys/attr.h: ATTR_BIT_MAP_COUNT, FSOPT_ATTR_CMN_EXTENDED and
    // ATTR_CMNEXT_PRIVATESIZE (a `forkattr`-word extended common attribute).
    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const FSOPT_ATTR_CMN_EXTENDED: u32 = 0x20;
    const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x8;
    #[repr(C, packed(4))]
    struct Reply {
        length: u32,
        private: i64,
    }
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return Private::Unknown;
    };
    // SAFETY: `attrlist` is plain data (zeroed is valid); `reply` is a
    // writable buffer of the size passed; `c` is NUL-terminated.
    let (rc, reply) = unsafe {
        let mut list: libc::attrlist = std::mem::zeroed();
        list.bitmapcount = ATTR_BIT_MAP_COUNT;
        list.forkattr = ATTR_CMNEXT_PRIVATESIZE;
        let mut reply = Reply {
            length: 0,
            private: 0,
        };
        let rc = libc::getattrlist(
            c.as_ptr(),
            (&mut list as *mut libc::attrlist).cast(),
            (&mut reply as *mut Reply).cast(),
            std::mem::size_of::<Reply>(),
            FSOPT_ATTR_CMN_EXTENDED,
        );
        (rc, reply)
    };
    if rc == 0 {
        return Private::Bytes(u64::try_from(reply.private).unwrap_or(0));
    }
    match std::io::Error::last_os_error().raw_os_error() {
        // A volume that does not know the attribute cannot share blocks.
        Some(libc::ENOTSUP | libc::EINVAL) => Private::NotShared,
        _ => Private::Unknown,
    }
}

fn allocated_file(path: &Path) -> u64 {
    allocated_size(path)
}

/// The private bytes of every file under `dir` (symlinks are not followed).
/// `Unknown` when any file could not be measured.
pub fn private_tree(dir: &Path) -> Private {
    let mut total = 0u64;
    let mut shared_capable = false;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let meta = match std::fs::symlink_metadata(&next) {
            Ok(m) => m,
            // Gone while the tree was walked (a log rotated, a temp file):
            // it holds nothing now.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Private::Unknown,
        };
        if meta.is_dir() {
            let rd = match std::fs::read_dir(&next) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Private::Unknown,
            };
            stack.extend(rd.flatten().map(|e| e.path()));
        } else if meta.is_file() {
            match private_size(&next) {
                Private::Bytes(b) => {
                    shared_capable = true;
                    total = total.saturating_add(b);
                }
                Private::NotShared => total = total.saturating_add(allocated_file(&next)),
                Private::Unknown if !next.exists() => {}
                Private::Unknown => return Private::Unknown,
            }
        }
    }
    if shared_capable {
        Private::Bytes(total)
    } else {
        Private::NotShared
    }
}

/// What the Space whose files are under `dir` costs this computer. `None`
/// when there is nothing there to measure.
pub fn measure(dir: &Path) -> Option<DiskOnHost> {
    if !dir.exists() {
        return None;
    }
    let allocated = allocated_size(dir);
    Some(match private_tree(dir) {
        Private::Bytes(b) => DiskOnHost {
            bytes: Some(b),
            allocated,
            shares_space: allocated.saturating_sub(b) > SHARED_SLACK,
        },
        Private::NotShared => DiskOnHost {
            bytes: Some(allocated),
            allocated,
            shares_space: false,
        },
        // Could not measure: say only that a copy may share, never a number.
        Private::Unknown => DiskOnHost {
            bytes: None,
            allocated,
            shares_space: true,
        },
    })
}

/// A local Space's allocation and where its disk lives on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    /// The sandbox name (`local:<name>`).
    pub name: String,
    /// The engine word of its state file (`lume`, `qemu`, `gvisor`, ...).
    pub runtime: String,
    /// Cores given to it (0: unknown).
    pub cpus: u32,
    /// Memory given to it, bytes (0: unknown).
    pub memory: u64,
    /// Its disk's size limit, bytes (0: unknown).
    pub disk: u64,
    /// The directory holding its own files on this computer: a VM's disk,
    /// a microVM's instance (scratch disk and state).
    pub dir: Option<PathBuf>,
    /// A container a container engine runs: its own bytes are the engine's
    /// writable layer for it ([`container_disk`]).
    pub engine: bool,
    /// Bytes `lume serve` reports its disk occupies, clones counted in
    /// full (0: not reported). The fallback when [`Footprint::dir`] cannot
    /// be read.
    pub reported: u64,
}

/// What `lume serve` says about one VM.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LumeFacts {
    /// VM name.
    pub name: String,
    /// Cores.
    pub cpus: u32,
    /// Memory, bytes.
    pub memory: u64,
    /// Disk size limit, bytes.
    pub disk_total: u64,
    /// Bytes its disk occupies, clones counted in full.
    pub reported: u64,
}

/// A qcow2 file's virtual size (what the guest can fill), from its header.
pub fn qcow2_virtual_size(path: &Path) -> Option<u64> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut hdr = [0u8; 32];
    f.read_exact(&mut hdr).ok()?;
    (&hdr[..4] == b"QFI\xfb").then(|| u64::from_be_bytes(hdr[24..32].try_into().expect("8 bytes")))
}

/// The local Spaces' allocations, from their state files, `lume serve` and
/// each QEMU instance's disk. Nothing is started or connected.
pub fn footprints(
    store: &StateStore,
    layout: &cua_disk::Layout,
    lume: &[LumeFacts],
    lume_home: Option<&Path>,
) -> Vec<Footprint> {
    store
        .list_all()
        .into_iter()
        .filter_map(|s| match s {
            SandboxState::Local(l) if l.runtime_type != "direct" => Some(l),
            _ => None,
        })
        .map(|l| {
            let facts = lume.iter().find(|v| v.name == l.name);
            let mut f = Footprint {
                cpus: l.cpu_count.or(facts.map(|v| v.cpus)).unwrap_or(0),
                memory: l
                    .memory_mb
                    .map(|m| m.saturating_mul(MIB))
                    .or(facts.map(|v| v.memory))
                    .unwrap_or(0),
                disk: facts.map(|v| v.disk_total).unwrap_or(0),
                reported: facts.map(|v| v.reported).unwrap_or(0),
                name: l.name.clone(),
                runtime: l.runtime_type.clone(),
                dir: None,
                engine: false,
            };
            match l.runtime_type.as_str() {
                "gvisor" | "runsc" | "container" | "docker" | "runc" | "managed"
                | "qemu-docker" | "qemu_docker" => f.engine = true,
                "firecracker" | "microvm" | "vz" => {
                    f.dir = Some(
                        layout
                            .home()
                            .join("vmm")
                            .join("microvm")
                            .join("instances")
                            .join(&l.name),
                    );
                }
                "containerization" => {
                    let rt = cua_vmm::containerization::ContainerizationRuntime::new(
                        cua_vmm::containerization::ContainerizationConfig {
                            root: layout.home().join("vmm").join("containerization"),
                            ..Default::default()
                        },
                    );
                    f.dir = Some(rt.container_dir(&l.name));
                }
                "lume" => f.dir = lume_home.map(|h| h.join(&l.name)),
                "qemu" => {
                    let dir = layout.qemu().join(&l.name);
                    if f.disk == 0
                        && let Some(state) =
                            std::fs::read(dir.join("state.json")).ok().and_then(|b| {
                                serde_json::from_slice::<cua_vmm::qemu::QemuState>(&b).ok()
                            })
                    {
                        f.disk = qcow2_virtual_size(&state.disk).unwrap_or(0);
                    }
                    f.dir = Some(dir);
                }
                _ => {}
            }
            f
        })
        .collect()
}

/// Whether a volume name is an anonymous one (64 hex digits): the engine
/// made it for one container, and it goes with that container.
fn is_anonymous_volume(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The containers of Space `name` among `containers`: its own and its
/// sidecars' (`<name>-sc-<sidecar>`), of this home (`home_tag`) when the
/// container says which home made it.
fn space_containers<'a>(
    name: &'a str,
    home_tag: &'a str,
    containers: &'a [cua_disk::docker::DockerContainer],
) -> impl Iterator<Item = &'a cua_disk::docker::DockerContainer> + 'a {
    let sidecar = format!("{name}-sc-");
    containers.iter().filter(move |c| {
        (c.name == name || c.name.starts_with(&sidecar))
            && c.labels
                .get(cua_disk::docker::LABEL_HOME)
                .is_none_or(|h| h == home_tag)
    })
}

/// The anonymous volumes Space `name`'s containers mount (sized with the
/// engine's disk-usage report, only when there are some).
pub fn container_volumes(
    name: &str,
    home_tag: &str,
    containers: &[cua_disk::docker::DockerContainer],
) -> Vec<String> {
    let mut out: Vec<String> = space_containers(name, home_tag, containers)
        .flat_map(|c| c.volumes.iter().filter(|v| is_anonymous_volume(v)).cloned())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What container Space `name` costs: the writable layers of its
/// containers plus the anonymous volumes they mount (what deleting it
/// frees). Its image's layers are shared and not counted, nor are named
/// volumes (Cua Volumes outlive the Space). `None` when the engine has no
/// container of it.
pub fn container_disk(
    name: &str,
    home_tag: &str,
    containers: &[cua_disk::docker::DockerContainer],
    volume_sizes: &std::collections::HashMap<String, u64>,
) -> Option<DiskOnHost> {
    let mut found = false;
    let mut bytes = 0u64;
    for c in space_containers(name, home_tag, containers) {
        found = true;
        bytes = bytes.saturating_add(c.size_rw);
    }
    if !found {
        return None;
    }
    for v in container_volumes(name, home_tag, containers) {
        bytes = bytes.saturating_add(volume_sizes.get(&v).copied().unwrap_or(0));
    }
    Some(DiskOnHost {
        bytes: Some(bytes),
        allocated: bytes,
        shares_space: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_sandbox_core::state::LocalState;

    fn write(path: &Path, mib: usize, byte: u8) {
        use std::io::Write;
        let mut f = std::fs::File::create(path).unwrap();
        let chunk = vec![byte; MIB as usize];
        for _ in 0..mib {
            f.write_all(&chunk).unwrap();
        }
    }

    #[test]
    fn a_plain_copy_counts_in_full() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("disk.img"), 4, 7);
        let d = measure(dir.path()).unwrap();
        assert!(d.allocated >= 4 * MIB);
        // Nothing else holds those blocks.
        assert_eq!(d.bytes, Some(d.allocated));
        assert!(!d.shares_space);
    }

    #[test]
    fn nothing_there_is_not_measured() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(measure(&dir.path().join("gone")), None);
    }

    /// A clone shares its blocks with the original: only what the clone
    /// changed is its own (macOS, APFS).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_clone_counts_only_what_it_changed() {
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        let copy = dir.path().join("copy");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&copy).unwrap();
        write(&base.join("disk.img"), 48, 9);
        let src = std::ffi::CString::new(base.join("disk.img").as_os_str().as_bytes()).unwrap();
        let dst = std::ffi::CString::new(copy.join("disk.img").as_os_str().as_bytes()).unwrap();
        // SAFETY: both are NUL-terminated paths.
        let rc = unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) };
        if rc != 0 {
            eprintln!("skipped: this volume cannot clone files");
            return;
        }
        if private_size(&copy.join("disk.img")) == Private::NotShared {
            eprintln!("skipped: not an APFS volume");
            return;
        }
        // 8 MiB of its own.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(copy.join("disk.img"))
            .unwrap();
        f.write_all(&vec![1u8; (8 * MIB) as usize]).unwrap();
        f.sync_all().unwrap();
        let d = measure(&copy).unwrap();
        assert!(d.allocated >= 56 * MIB, "{d:?}");
        let own = d.bytes.expect("APFS reports private bytes");
        assert!((8 * MIB..24 * MIB).contains(&own), "own {own} of {d:?}");
        assert!(d.shares_space, "{d:?}");
    }

    #[test]
    fn a_qcow2_header_gives_the_virtual_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("disk.qcow2");
        let mut hdr = vec![0u8; 64];
        hdr[..4].copy_from_slice(b"QFI\xfb");
        hdr[24..32].copy_from_slice(&(80u64 << 30).to_be_bytes());
        std::fs::write(&p, hdr).unwrap();
        assert_eq!(qcow2_virtual_size(&p), Some(80 << 30));
        std::fs::write(&p, b"not a qcow2 file at all, just text........").unwrap();
        assert_eq!(qcow2_virtual_size(&p), None);
    }

    #[test]
    fn allocations_come_from_state_files_and_lume() {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(home.path().join("sandboxes"));
        for (name, runtime, cpus, mem) in [
            ("mac", "lume", None, None),
            ("box", "gvisor", Some(2), Some(2048)),
        ] {
            store
                .save(&SandboxState::Local(LocalState {
                    name: name.into(),
                    runtime_type: runtime.into(),
                    status: "running".into(),
                    cpu_count: cpus,
                    memory_mb: mem,
                    ..Default::default()
                }))
                .unwrap();
        }
        let lume = [LumeFacts {
            name: "mac".into(),
            cpus: 4,
            memory: 8 << 30,
            disk_total: 80 << 30,
            reported: 31 << 30,
        }];
        let layout = cua_disk::Layout::new(home.path());
        let lume_home = home.path().join("lume");
        let mut list = footprints(&store, &layout, &lume, Some(&lume_home));
        list.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(list.len(), 2);
        let b = &list[0];
        assert_eq!(
            (b.name.as_str(), b.cpus, b.memory, b.disk),
            ("box", 2, 2 << 30, 0)
        );
        assert_eq!(b.dir, None);
        let m = &list[1];
        assert_eq!(
            (m.cpus, m.memory, m.disk, m.reported),
            (4, 8 << 30, 80 << 30, 31 << 30)
        );
        assert_eq!(m.dir, Some(lume_home.join("mac")));
        assert!(!m.engine);
        assert!(b.engine, "a gVisor Space is the engine's container");
    }

    #[test]
    fn microvms_and_apple_containers_are_measured_in_their_own_directories() {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(home.path().join("sandboxes"));
        for (name, runtime) in [
            ("fc", "firecracker"),
            ("vz1", "vz"),
            ("apple", "containerization"),
        ] {
            store
                .save(&SandboxState::Local(LocalState {
                    name: name.into(),
                    runtime_type: runtime.into(),
                    status: "stopped".into(),
                    ..Default::default()
                }))
                .unwrap();
        }
        let layout = cua_disk::Layout::new(home.path());
        let mut list = footprints(&store, &layout, &[], None);
        list.sort_by(|a, b| a.name.cmp(&b.name));
        let vmm = home.path().join("vmm");
        let dirs: Vec<_> = list
            .iter()
            .map(|f| (f.name.as_str(), f.dir.clone(), f.engine))
            .collect();
        assert_eq!(
            dirs,
            [
                (
                    "apple",
                    Some(vmm.join("containerization/data/containers/apple")),
                    false
                ),
                ("fc", Some(vmm.join("microvm/instances/fc")), false),
                ("vz1", Some(vmm.join("microvm/instances/vz1")), false),
            ]
        );
        // A microVM's instance directory: its scratch disk is its own.
        let dir = vmm.join("microvm/instances/fc");
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("scratch.ext4"), 3, 1);
        let d = measure(&dir).unwrap();
        assert!(d.bytes.unwrap() >= 3 * MIB, "{d:?}");
    }

    fn container(
        name: &str,
        home: Option<&str>,
        size_rw: u64,
        volumes: &[&str],
    ) -> cua_disk::docker::DockerContainer {
        cua_disk::docker::DockerContainer {
            name: name.into(),
            size_rw,
            labels: home
                .map(|h| [(cua_disk::docker::LABEL_HOME.to_string(), h.to_string())].into())
                .unwrap_or_default(),
            volumes: volumes.iter().map(|v| v.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_container_costs_its_own_layer_its_sidecars_and_its_anonymous_volumes() {
        let anon = "a".repeat(64);
        let other_anon = "b".repeat(64);
        let containers = [
            container("box", Some("h1"), 300 * MIB, &[&anon, "cua-vol-notes"]),
            container("box-sc-db", Some("h1"), 50 * MIB, &[]),
            // Another home's container of the same name, another Space's,
            // and one from before homes were labelled.
            container("box", Some("h2"), 9000 * MIB, &[]),
            container("boxer", Some("h1"), 7000 * MIB, &[&other_anon]),
            container("old", None, 20 * MIB, &[]),
        ];
        assert_eq!(container_volumes("box", "h1", &containers), [anon.as_str()]);
        let sizes = std::collections::HashMap::from([
            (anon.clone(), 100 * MIB),
            ("cua-vol-notes".to_string(), 5000 * MIB),
            (other_anon, 1000 * MIB),
        ]);
        let d = container_disk("box", "h1", &containers, &sizes).unwrap();
        // 300 + 50 + 100: not the image, not the named volume (it outlives
        // the Space), not another home's or Space's.
        assert_eq!(d.bytes, Some(450 * MIB));
        assert_eq!(d.allocated, 450 * MIB);
        assert!(!d.shares_space);
        assert_eq!(
            container_disk("old", "h1", &containers, &sizes)
                .unwrap()
                .bytes,
            Some(20 * MIB)
        );
        // An unsized volume adds nothing; no container: nothing to say.
        let d = container_disk("box", "h1", &containers, &Default::default()).unwrap();
        assert_eq!(d.bytes, Some(350 * MIB));
        assert_eq!(container_disk("gone", "h1", &containers, &sizes), None);
    }

    /// Only what truly cannot be read leaves the number out: a folder this
    /// user may not list.
    #[cfg(unix)]
    #[test]
    fn only_what_cannot_be_read_leaves_the_size_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("disk.img"), 2, 3);
        assert!(measure(dir.path()).unwrap().bytes.is_some());
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&locked).is_ok(); // root reads anything
        let d = measure(dir.path()).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert_eq!(d.bytes, None, "{d:?}");
            assert!(d.shares_space);
        }
    }
}
