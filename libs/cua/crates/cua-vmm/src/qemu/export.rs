//! A QEMU checkpoint as files, and a copied one made a checkpoint here
//! (snapshot sync between the hosts of a pool).
//!
//! **Export.** A checkpoint is its state entry plus every file the state
//! names: the memory snapshot directory (`state.mig`, the guest token, UEFI
//! variables, the SSH key, a RAM file), the disk layer and its whole qcow2
//! backing chain, and the UEFI code. Which files those are is read from
//! the state itself (every absolute path in it that exists), so a field a
//! later format adds (a RAM file next to the stream) is exported without
//! changing this module.
//!
//! **Import.** The copied files are laid out under
//! `<root>/_imported/<manifest digest>/` (by the caller, as hard links to
//! its verified copies), the disk layers are pointed at their backing
//! files' copies (`qemu-img rebase -u`, metadata only), and the state is
//! saved under the checkpoint's name. Forks of it then work as forks of a
//! local checkpoint. An `_imported` directory no state names any more is
//! removed with unused layers.
//!
//! **ABI.** A checkpoint restores only under the accelerator and guest
//! architecture it was taken with: `qemu/<accel>/<arch>` (`qemu/hvf/aarch64`
//! on an Apple silicon Mac, `qemu/kvm/aarch64` on an arm64 Linux host).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{EntryKind, QemuRuntime, QemuState, img, validate_name};
use crate::error::{Result, VmmError};
use crate::host;
use crate::runtime::{CheckpointExport, ExportFile};
use crate::types::Arch;

/// Where copied checkpoints' files live under the QEMU root.
pub(crate) const IMPORTED: &str = "_imported";
/// An import directory younger than this is never collected (its state may
/// not be saved yet).
const IMPORT_GRACE: Duration = Duration::from_secs(600);

fn arch_word(a: Arch) -> &'static str {
    match a {
        Arch::Aarch64 => "aarch64",
        Arch::X86_64 => "x86_64",
    }
}

/// The ABI of a checkpoint taken under `accel` for an `arch` guest.
pub fn abi(accel: &str, arch: Arch) -> String {
    format!("qemu/{accel}/{}", arch_word(arch))
}

/// Every string in `v` that is an absolute path to something that exists.
fn paths_in(v: &serde_json::Value, out: &mut BTreeSet<PathBuf>) {
    match v {
        serde_json::Value::String(s) => {
            let p = Path::new(s);
            if p.is_absolute() && p.exists() {
                out.insert(p.to_path_buf());
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| paths_in(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| paths_in(x, out)),
        _ => {}
    }
}

/// Whether `p` starts with the qcow2 magic.
fn is_qcow2(p: &Path) -> bool {
    use std::io::Read as _;
    let mut m = [0u8; 4];
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut m))
        .is_ok()
        && m == *b"QFI\xfb"
}

impl QemuRuntime {
    /// The checkpoint ABIs this host restores: the native accelerator for
    /// the host's own architecture, when memory restores work here.
    pub fn restorable_abis(&self) -> Vec<String> {
        if !self.restores_memory() {
            return vec![];
        }
        let arch = Arch::host();
        match host::qemu_accel(host::HostOs::current(), arch, &host::kvm_usable()) {
            Ok(accel) if accel != "tcg" => vec![abi(accel, arch)],
            _ => vec![],
        }
    }

    /// Where the files of a copied checkpoint of `digest` go.
    pub fn imported_dir(&self, digest: &str) -> PathBuf {
        self.cfg.root.join(IMPORTED).join(digest)
    }

    pub(crate) async fn export(&self, name: &str) -> Result<CheckpointExport> {
        let st = self.load(name)?;
        if st.kind != EntryKind::Checkpoint {
            return Err(VmmError::invalid(format!(
                "'{name}' is not a checkpoint (only checkpoints are copied to other hosts)"
            )));
        }
        let state = serde_json::to_value(&st)?;
        let mut named = BTreeSet::new();
        paths_in(&state, &mut named);
        let entry = self.dir(name);
        let mut files: Vec<ExportFile> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut dirs = Vec::new();
        for p in &named {
            if p.is_dir() {
                dirs.push(p.clone());
                let mut inside: Vec<PathBuf> =
                    std::fs::read_dir(p)?.flatten().map(|e| e.path()).collect();
                inside.sort();
                for f in inside {
                    let base = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    // The entry's own state (written again on import) and
                    // temporaries are not part of it.
                    if (*p == entry && base == "state.json")
                        || base.ends_with(".tmp")
                        || base.starts_with('.')
                    {
                        continue;
                    }
                    if f.is_dir() {
                        return Err(VmmError::invalid(format!(
                            "checkpoint '{name}' has a directory {} inside {}; it cannot be copied",
                            f.display(),
                            p.display()
                        )));
                    }
                    if seen.insert(f.clone()) {
                        files.push(ExportFile {
                            path: f,
                            dir: Some(p.clone()),
                            qcow2_backing: None,
                        });
                    }
                }
            } else if p.is_file() && seen.insert(p.clone()) {
                files.push(ExportFile {
                    path: p.clone(),
                    dir: None,
                    qcow2_backing: None,
                });
            }
        }
        // Disk layers: their whole backing chain, each linked to its
        // backing file.
        let mut i = 0;
        while i < files.len() {
            let f = files[i].path.clone();
            i += 1;
            if !is_qcow2(&f) {
                continue;
            }
            let chain = img::chain(&f).await?;
            for pair in chain.windows(2) {
                let (top, below) = (
                    PathBuf::from(&pair[0].filename),
                    PathBuf::from(&pair[1].filename),
                );
                if !below.is_absolute() {
                    return Err(VmmError::invalid(format!(
                        "{} has a relative backing file; it cannot be copied",
                        top.display()
                    )));
                }
                if let Some(e) = files.iter_mut().find(|e| e.path == top) {
                    e.qcow2_backing = Some(below.clone());
                }
                if seen.insert(below.clone()) {
                    files.push(ExportFile {
                        path: below,
                        dir: None,
                        qcow2_backing: None,
                    });
                }
            }
        }
        Ok(CheckpointExport {
            state,
            files,
            dirs,
            abi: abi(&st.accel, st.arch),
        })
    }

    pub(crate) async fn import(
        &self,
        name: &str,
        state: serde_json::Value,
        rebase: Vec<(PathBuf, PathBuf)>,
    ) -> Result<()> {
        validate_name(name)?;
        let _g = self.lock.lock().await;
        if self.state_path(name).exists() {
            return Err(VmmError::AlreadyExists(name.to_string()));
        }
        let mut st: QemuState = serde_json::from_value(state)?;
        if st.kind != EntryKind::Checkpoint {
            return Err(VmmError::invalid("only checkpoints are imported"));
        }
        let here = self.restorable_abis();
        let theirs = abi(&st.accel, st.arch);
        if !here.contains(&theirs) {
            return Err(VmmError::invalid(format!(
                "checkpoint '{name}' is {theirs}; this host restores {}",
                if here.is_empty() {
                    "no QEMU checkpoints".to_string()
                } else {
                    here.join(", ")
                }
            )));
        }
        for (overlay, backing) in &rebase {
            let fmt = img::info(backing, true).await?.format;
            let o = overlay.display().to_string();
            let b = backing.display().to_string();
            host::run(
                img::qemu_img()?,
                &["rebase", "-u", "-f", "qcow2", "-b", &b, "-F", &fmt, &o],
            )
            .await?;
        }
        st.name = name.to_string();
        st.pid = None;
        st.qmp_port = None;
        st.vnc_display = None;
        st.ports.clear();
        st.paused = false;
        st.restore_from = None;
        st.last_start = None;
        self.save(&st)
    }

    /// Removes `_imported/<digest>` directories no state names any more.
    pub(crate) fn gc_imported(&self, states: &[QemuState]) {
        let root = self.cfg.root.join(IMPORTED);
        let Ok(rd) = std::fs::read_dir(&root) else {
            return;
        };
        let named: Vec<String> = states
            .iter()
            .filter_map(|s| serde_json::to_string(s).ok())
            .collect();
        for e in rd.flatten() {
            let p = e.path();
            let young = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age < IMPORT_GRACE);
            let needle = p.display().to_string();
            if young || named.iter().any(|s| s.contains(&needle)) {
                continue;
            }
            tracing::debug!(dir = %p.display(), "removing an unused imported checkpoint");
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abis_name_the_accelerator_and_arch() {
        assert_eq!(abi("hvf", Arch::Aarch64), "qemu/hvf/aarch64");
        assert_eq!(abi("kvm", Arch::X86_64), "qemu/kvm/x86_64");
    }

    #[test]
    fn only_existing_absolute_paths_are_files() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x");
        std::fs::write(&f, b"1").unwrap();
        let v = serde_json::json!({
            "a": f,
            "b": "relative/path",
            "c": "/definitely/not/here",
            "d": ["hvf", {"e": dir.path()}],
        });
        let mut out = BTreeSet::new();
        paths_in(&v, &mut out);
        assert_eq!(out, BTreeSet::from([f.clone(), dir.path().to_path_buf()]));
        assert!(!is_qcow2(&f));
        std::fs::write(&f, b"QFI\xfb\0\0\0\x03").unwrap();
        assert!(is_qcow2(&f));
    }

    struct Ident;
    #[async_trait::async_trait]
    impl crate::runtime::GuestIdentity for Ident {
        async fn renew(
            &self,
            _: &crate::types::Endpoints,
            _: &str,
            _: &crate::types::ClaimIdentity,
            _: bool,
            _: std::time::Duration,
        ) -> Result<()> {
            Ok(())
        }
    }

    /// Real qemu-img, nothing booted: a checkpoint's disk chain is
    /// exported, its files copied to another root (as another host would
    /// lay them out), imported there with its layers rebased onto the
    /// copies, forked, and collected once nothing names it.
    #[tokio::test]
    async fn a_checkpoint_round_trips_to_another_root() {
        use crate::runtime::Runtime as _;
        use crate::types::ImageSource;
        if img::qemu_img().is_err() {
            eprintln!("qemu-img missing; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let cfg = |root: &Path| super::super::QemuConfig {
            root: root.to_path_buf(),
            identity: Some(std::sync::Arc::new(Ident)),
            ..Default::default()
        };
        let a = QemuRuntime::new(cfg(&tmp.path().join("a")));
        let image = tmp.path().join("image.qcow2");
        img::create_blank(&image, 1).await.unwrap();
        a.ensure_base(&ImageSource::disk(&image), "base")
            .await
            .unwrap();
        a.fork("base", "vm").await.unwrap();
        a.checkpoint("vm", "ck").await.unwrap();
        let ex = a.export_checkpoint("ck").await.unwrap();
        let st = a.load("ck").unwrap();
        assert_eq!(ex.abi, abi(&st.accel, st.arch));
        // The disk layer is there, linked to its backing file, which is
        // there too (down to the image).
        let top = ex
            .files
            .iter()
            .find(|f| f.path == st.disk)
            .expect("disk layer");
        let below = top.qcow2_backing.clone().expect("backing");
        assert!(ex.files.iter().any(|f| f.path == below));
        let chain = img::chain_files(&st.disk).await.unwrap();
        for c in &chain {
            assert!(
                ex.files.iter().any(|f| &f.path == c),
                "{} missing",
                c.display()
            );
        }
        // Instances are not exported.
        assert!(a.export_checkpoint("vm").await.is_err());

        // "Another host": copy each file under its import directory.
        let b = QemuRuntime::new(cfg(&tmp.path().join("b")));
        let dest = b.import_dir("d1g3st").unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        let mut map = std::collections::BTreeMap::new();
        for (i, f) in ex.files.iter().enumerate() {
            let to = dest.join(format!(
                "{i}-{}",
                f.path.file_name().unwrap().to_string_lossy()
            ));
            std::fs::copy(&f.path, &to).unwrap();
            map.insert(f.path.clone(), to);
        }
        let rebase: Vec<(PathBuf, PathBuf)> = ex
            .files
            .iter()
            .filter_map(|f| Some((map[&f.path].clone(), map[f.qcow2_backing.as_ref()?].clone())))
            .collect();
        let mut state = ex.state.clone();
        fn swap(v: &mut serde_json::Value, m: &std::collections::BTreeMap<PathBuf, PathBuf>) {
            match v {
                serde_json::Value::String(s) => {
                    if let Some(to) = m.get(Path::new(s.as_str())) {
                        *s = to.display().to_string();
                    }
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(|x| swap(x, m)),
                serde_json::Value::Object(o) => o.values_mut().for_each(|x| swap(x, m)),
                _ => {}
            }
        }
        swap(&mut state, &map);
        let here = b.restorable_abis();
        if !here.contains(&ex.abi) {
            // This machine has no hardware accelerator for the guest (CI
            // without KVM): the import is refused with why.
            let e = b.import_checkpoint("ck", state, rebase).await.unwrap_err();
            assert!(e.to_string().contains("this host restores"), "{e}");
            return;
        }
        b.import_checkpoint("ck2", state.clone(), rebase.clone())
            .await
            .unwrap();
        assert!(matches!(
            b.import_checkpoint("ck2", state, rebase).await,
            Err(VmmError::AlreadyExists(_))
        ));
        let imported = b.load("ck2").unwrap();
        assert_eq!(imported.kind, EntryKind::Checkpoint);
        assert!(imported.disk.starts_with(&dest));
        // Its chain is entirely the copies: nothing on "host a".
        for f in img::chain_files(&imported.disk).await.unwrap() {
            assert!(f.starts_with(&dest), "{} not rebased", f.display());
        }
        // A fork of it is an overlay on the copy.
        b.fork("ck2", "child").await.unwrap();
        let child = b.load("child").unwrap();
        let cchain = img::chain_files(&child.disk).await.unwrap();
        assert!(cchain[1..].iter().all(|f| f.starts_with(&dest)));
        // Collected only once nothing names it (and it is old enough).
        let ten_min_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let f = std::fs::File::open(&dest).unwrap();
        f.set_modified(ten_min_ago).unwrap();
        b.gc_imported(&b.all_states().unwrap());
        assert!(dest.exists());
        b.delete("child").await.unwrap();
        b.delete_checkpoint("ck2").await.unwrap();
        let f = std::fs::File::open(&dest).unwrap();
        f.set_modified(ten_min_ago).unwrap();
        b.gc_imported(&b.all_states().unwrap());
        assert!(!dest.exists());
    }
}
