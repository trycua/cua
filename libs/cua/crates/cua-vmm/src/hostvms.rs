//! The QEMU VMs running on this host, and which Cua home each belongs to.
//!
//! QEMU is started detached on purpose: it survives the process that
//! started it (a daemon restart must not kill the work in it). The price is
//! that a VM can outlive everything that knew about it (its daemon crashed,
//! its home was deleted) and keep holding CPUs and memory. This module finds
//! such VMs without trusting any one daemon's memory: the process table is
//! the source, and the *owner marker* is the pid file QEMU is started with,
//! `<home>/vmm/qemu/<name>/qemu.pid`. A process counts as a Cua VM only when
//!
//! * it is a `qemu-system-*` binary,
//! * its `-pidfile` is `<root>/<name>/qemu.pid` with `<root>` ending in
//!   `vmm/qemu`, and its `-name` is `<name>`, and
//! * that pid file holds its own pid.
//!
//! Anything else (another program's QEMU, a hand-started VM) is never
//! reported, so it is never stopped. The home a VM belongs to is read off
//! its pid file, so one home's sweep cannot reach another home's VMs.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A running Cua QEMU VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostVm {
    /// The QEMU process.
    pub pid: u32,
    /// The instance name (`-name`, and its directory under [`HostVm::root`]).
    pub name: String,
    /// `<home>/vmm/qemu`.
    pub root: PathBuf,
    /// How long the process has been running.
    pub age: Duration,
    /// A golden-snapshot recorder: its files are in
    /// `<root>/_golden/.rec-<id>`, not `<root>/<name>`, and no sandbox
    /// ever records it, so only its process and that directory exist.
    pub recorder_dir: Option<PathBuf>,
}

impl HostVm {
    /// `<home>/vmm/qemu/<name>`: its state, disk overlay and pid file (a
    /// recorder's own directory under `_golden`).
    pub fn dir(&self) -> PathBuf {
        self.recorder_dir
            .clone()
            .unwrap_or_else(|| self.root.join(&self.name))
    }

    /// The Cua home this VM belongs to.
    pub fn home(&self) -> PathBuf {
        self.root
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }
}

/// The process that started a VM (`<dir>/launcher.json`). QEMU outlives it
/// on purpose; the record lets a later sweep tell "its starter is still
/// working on it" from "its starter is gone".
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Launcher {
    /// The starting process.
    pub pid: u32,
    /// Unix seconds.
    pub at: u64,
}

impl Launcher {
    /// This process.
    pub fn here() -> Self {
        Self {
            pid: std::process::id(),
            at: crate::host::now_secs(),
        }
    }

    /// Writes the record into a VM's directory (best effort).
    pub fn write(&self, dir: &Path) {
        if let Ok(b) = serde_json::to_vec(self) {
            let _ = std::fs::write(dir.join("launcher.json"), b);
        }
    }

    /// The record of the VM in `dir`; `None` for a VM from before records.
    pub fn read(dir: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(dir.join("launcher.json")).ok()?).ok()
    }

    /// Whether the starting process is still running.
    pub fn alive(&self) -> bool {
        crate::host::pid_alive(self.pid)
    }
}

/// `[[dd-]hh:]mm:ss` as `ps -o etime` prints it.
pub fn parse_etime(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(Duration::from_secs(days * 86_400 + secs))
}

/// The value after `flag` in a command line (up to the next ` -flag` or the
/// end), for values that may hold spaces (a home under `Application
/// Support`).
fn flag_value<'a>(cmd: &'a str, flag: &str, ends_with: &str) -> Option<&'a str> {
    let key = format!(" {flag} ");
    let start = cmd.find(&key)? + key.len();
    let rest = &cmd[start..];
    let end = rest.find(ends_with)? + ends_with.len();
    // Whatever follows must end the argument.
    match rest[end..].chars().next() {
        None | Some(' ') => Some(&rest[..end]),
        _ => None,
    }
}

/// A Cua VM from one process-table line (`pid etime command...`), without
/// looking at the disk. The pid file's content is checked by [`verify`].
pub fn parse_line(line: &str) -> Option<HostVm> {
    // `ps` pads its columns: split on runs of blanks.
    fn token(s: &str) -> Option<(&str, &str)> {
        let s = s.trim_start();
        let end = s.find(char::is_whitespace)?;
        Some((&s[..end], &s[end..]))
    }
    let (pid, rest) = token(line)?;
    let (etime, rest) = token(rest)?;
    let pid: u32 = pid.parse().ok()?;
    let cmd = format!(" {}", rest.trim_start());
    let age = parse_etime(etime)?;
    // The binary: the first word, or the second behind an interpreter.
    let is_qemu = cmd.split_whitespace().take(2).any(|w| {
        Path::new(w)
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("qemu-system-"))
    });
    if !is_qemu {
        return None;
    }
    let pidfile = flag_value(&cmd, "-pidfile", "/qemu.pid")?;
    let name = cmd
        .split(" -name ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())?;
    let pidfile = Path::new(pidfile);
    let dir = pidfile.parent()?;
    if name.is_empty() {
        return None;
    }
    // A golden recorder runs from `<root>/_golden/.rec-<id>/`.
    let recorder = dir
        .file_name()
        .is_some_and(|d| d.to_string_lossy().starts_with(".rec-"))
        && dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|g| g == "_golden");
    if recorder {
        let root = dir.parent()?.parent()?;
        if !root.ends_with("vmm/qemu") {
            return None;
        }
        return Some(HostVm {
            pid,
            name: name.to_string(),
            root: root.to_path_buf(),
            age,
            recorder_dir: Some(dir.to_path_buf()),
        });
    }
    if dir.file_name()? != name {
        return None;
    }
    let root = dir.parent()?;
    if !root.ends_with("vmm/qemu") {
        return None;
    }
    Some(HostVm {
        pid,
        name: name.to_string(),
        root: root.to_path_buf(),
        age,
        recorder_dir: None,
    })
}

/// Whether `vm` really is the VM its pid file says: the pid file holds this
/// pid and the instance has its state file. A process that only looks like
/// one (copied command line, pid reuse after the file was removed) fails.
pub fn verify(vm: &HostVm) -> bool {
    let pidfile_ok = std::fs::read_to_string(vm.dir().join("qemu.pid"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        == Some(vm.pid);
    // A recorder has a launcher record instead of an instance state file.
    pidfile_ok
        && (vm.dir().join("state.json").is_file()
            || (vm.recorder_dir.is_some() && vm.dir().join("launcher.json").is_file()))
}

/// Every Cua QEMU VM in a `ps` listing (`ps -axww -o pid=,etime=,command=`),
/// verified against the disk.
pub fn parse_ps(out: &str) -> Vec<HostVm> {
    out.lines().filter_map(parse_line).filter(verify).collect()
}

/// How long a VM may run without its own instance directory before it
/// counts as left over. The directory is written before QEMU starts, so a
/// VM being created always has it; a delete removes the directory and then
/// the process, so a short grace covers a delete in progress.
pub const INSTANCE_GONE_GRACE: Duration = Duration::from_secs(30);

/// The VMs of a `ps` listing with nothing left on disk to record, stop or
/// adopt them, so only the process remains: their whole home was deleted,
/// or their own instance directory is gone (a delete or a cancelled create
/// removed the files but the process outlived it, for example when the
/// daemon was killed in between) for longer than [`INSTANCE_GONE_GRACE`].
pub fn parse_ps_homeless(out: &str) -> Vec<HostVm> {
    out.lines()
        .filter_map(parse_line)
        .filter(|v| {
            !v.home().as_os_str().is_empty()
                && (!v.home().exists() || (!v.dir().exists() && v.age >= INSTANCE_GONE_GRACE))
        })
        .collect()
}

fn ps() -> String {
    #[cfg(unix)]
    {
        std::process::Command::new("ps")
            .args(["-axww", "-o", "pid=,etime=,command="])
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        String::new()
    }
}

/// Every Cua QEMU VM running on this host, whichever home it belongs to.
pub fn host_vms() -> Vec<HostVm> {
    parse_ps(&ps())
}

/// [`parse_ps_homeless`] over the running processes.
pub fn homeless_vms() -> Vec<HostVm> {
    parse_ps_homeless(&ps())
}

/// The VMs of one home (`root` is `<home>/vmm/qemu`).
pub fn vms_of(root: &Path) -> Vec<HostVm> {
    host_vms().into_iter().filter(|v| v.root == root).collect()
}

/// Whether `vm` is still the same process (pid reuse must never make a
/// reap hit another program).
pub fn still_running(vm: &HostVm) -> bool {
    let out = ps();
    parse_ps(&out)
        .into_iter()
        .chain(parse_ps_homeless(&out))
        .any(|v| v.pid == vm.pid && v.name == vm.name && v.root == vm.root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(pid: u32, home: &str, name: &str) -> String {
        format!(
            "{pid:>6} 01:49:42 /opt/homebrew/bin/qemu-system-aarch64 -name {name} -machine virt -smp 2 \
             -drive if=pflash,file=/x/edk2.fd -pidfile {home}/vmm/qemu/{name}/qemu.pid -qmp tcp:127.0.0.1:5"
        )
    }

    #[test]
    fn padded_columns_parse() {
        let v = parse_line(
            "  1126       00:01 /bin/sh /t/bin/qemu-system-x86_64 -name vm1 -pidfile /t/h/vmm/qemu/vm1/qemu.pid",
        )
        .unwrap();
        assert_eq!((v.pid, v.name.as_str()), (1126, "vm1"));
    }

    #[test]
    fn etime_forms() {
        assert_eq!(parse_etime("00:05"), Some(Duration::from_secs(5)));
        assert_eq!(parse_etime("01:49:42"), Some(Duration::from_secs(6582)));
        assert_eq!(parse_etime("1-02:00:00"), Some(Duration::from_secs(93600)));
        assert_eq!(parse_etime("garbage"), None);
    }

    #[test]
    fn a_cua_vm_line_names_its_home() {
        let v = parse_line(&line(17287, "/Users/cua/a/home", "cua-pool-2a")).unwrap();
        assert_eq!(v.pid, 17287);
        assert_eq!(v.name, "cua-pool-2a");
        assert_eq!(v.home(), PathBuf::from("/Users/cua/a/home"));
        assert_eq!(v.age, Duration::from_secs(6582));
    }

    #[test]
    fn a_home_with_spaces_is_one_path() {
        let v = parse_line(&line(9, "/Users/x/Library/Application Support/cua", "vm1")).unwrap();
        assert_eq!(
            v.root,
            PathBuf::from("/Users/x/Library/Application Support/cua/vmm/qemu")
        );
    }

    #[test]
    fn other_programs_are_never_reported() {
        // Not QEMU.
        assert!(parse_line("12 00:10 /usr/bin/sleep 5 -pidfile /h/vmm/qemu/a/qemu.pid").is_none());
        // QEMU of someone else: no cua pid file layout.
        assert!(
            parse_line("12 00:10 /usr/bin/qemu-system-x86_64 -name a -pidfile /var/run/a.pid")
                .is_none()
        );
        // -name does not match the directory.
        assert!(
            parse_line(
                "12 00:10 /usr/bin/qemu-system-x86_64 -name b -pidfile /h/vmm/qemu/a/qemu.pid"
            )
            .is_none()
        );
        // Not under vmm/qemu.
        assert!(
            parse_line("12 00:10 /usr/bin/qemu-system-x86_64 -name a -pidfile /h/other/a/qemu.pid")
                .is_none()
        );
    }

    #[test]
    fn a_vm_whose_home_was_deleted_is_found_as_homeless() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("deleted-home");
        let l = line(7, gone.to_str().unwrap(), "vm1");
        assert!(parse_ps(&l).is_empty());
        assert_eq!(parse_ps_homeless(&l).len(), 1);
        // A home that exists, a young VM whose dir is missing (a delete in
        // progress): not yet.
        std::fs::create_dir_all(&gone).unwrap();
        assert!(parse_ps_homeless(&l.replace("01:49:42", "00:05")).is_empty());
    }

    #[test]
    fn a_vm_whose_instance_dir_is_gone_is_left_over_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let h = home.to_str().unwrap();
        // 60 s old (etime 01:00), no instance dir: left over.
        let old = format!(
            "7 01:00 /usr/bin/qemu-system-x86_64 -name vm1 -pidfile {h}/vmm/qemu/vm1/qemu.pid"
        );
        assert_eq!(parse_ps_homeless(&old).len(), 1);
        // The same VM with its dir present is a normal, verified VM.
        std::fs::create_dir_all(home.join("vmm/qemu/vm1")).unwrap();
        assert!(parse_ps_homeless(&old).is_empty());
    }

    #[test]
    fn a_golden_recorder_is_a_cua_vm_with_its_own_dir() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path().to_str().unwrap();
        let rec = dir.path().join("vmm/qemu/_golden/.rec-abc");
        std::fs::create_dir_all(&rec).unwrap();
        std::fs::write(rec.join("qemu.pid"), "9").unwrap();
        std::fs::write(rec.join("launcher.json"), "{}").unwrap();
        let l = format!(
            "9 20:00 /usr/bin/qemu-system-x86_64 -name e2b-golden-x-boot -pidfile {h}/vmm/qemu/_golden/.rec-abc/qemu.pid"
        );
        let vms = parse_ps(&l);
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].dir(), rec);
        assert_eq!(vms[0].home(), dir.path());
    }

    #[test]
    fn the_pid_file_must_hold_the_pid() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_str().unwrap();
        let vmdir = dir.path().join("vmm/qemu/vm1");
        std::fs::create_dir_all(&vmdir).unwrap();
        std::fs::write(vmdir.join("state.json"), "{}").unwrap();
        let l = line(4242, home, "vm1");
        assert!(parse_ps(&l).is_empty(), "no pid file yet");
        std::fs::write(vmdir.join("qemu.pid"), "4243\n").unwrap();
        assert!(parse_ps(&l).is_empty(), "another pid");
        std::fs::write(vmdir.join("qemu.pid"), "4242\n").unwrap();
        assert_eq!(parse_ps(&l).len(), 1);
    }
}
