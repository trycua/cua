//! Linux process enumeration via the /proc filesystem.
//!
//! Each /proc/<pid>/status file contains Name:, Pid:, PPid: etc.
//! /proc/<pid>/cmdline is the full command line (NUL-separated).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
}

fn is_process_live_state(status: &str) -> bool {
    status
        .lines()
        .find_map(|line| {
            line.strip_prefix("State:")
                .and_then(|state| state.trim().chars().next())
        })
        .is_some_and(|state| !matches!(state, 'Z' | 'X'))
}

/// Return whether a PID still represents a live process.
///
/// A zombie remains visible under `/proc` until its parent reaps it, so an
/// existence check alone is not enough for callers that may query AT-SPI or
/// X11 state for the process. Treat zombies and dead processes as gone.
pub fn is_process_live(pid: u32) -> bool {
    let status = match fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => status,
        Err(_) => return false,
    };

    is_process_live_state(&status)
}

/// Parse the kernel start time (field 22, in clock ticks) from the contents
/// of `/proc/<pid>/stat`.
pub(crate) fn process_start_time_from_stat(stat: &str) -> Option<u64> {
    // `comm` is parenthesized and may contain spaces and closing parentheses.
    // Fields after its final `)` begin with state (field 3); starttime is
    // field 22.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Kernel start-time token for one PID. Unlike the numeric PID alone, this
/// distinguishes a live process from a later process that reused its PID.
pub fn process_instance_id(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    process_start_time_from_stat(&stat)
}

/// One process instance as `/proc/<pid>/stat` describes it: its scheduler
/// state and its kernel start time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessStat {
    pub state: char,
    pub start_time: u64,
}

/// Parse state (field 3) and start time (field 22) from `/proc/<pid>/stat`.
pub(crate) fn process_stat_from_stat(stat: &str) -> Option<ProcessStat> {
    let state = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .next()?
        .chars()
        .next()?;
    Some(ProcessStat {
        state,
        start_time: process_start_time_from_stat(stat)?,
    })
}

/// Read one pid's [`ProcessStat`]; `Ok(None)` when no such process exists.
pub(crate) fn read_process_stat(pid: u32) -> std::io::Result<Option<ProcessStat>> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => process_stat_from_stat(&stat).map(Some).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed /proc stat")
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// How the process first read as `before` ended, judged from a later read of
/// the same pid, or `None` while that same instance is still alive. A zombie
/// has exited and only waits for its parent to reap it.
pub(crate) fn process_exit_observation(
    before: ProcessStat,
    now: Option<ProcessStat>,
) -> Option<&'static str> {
    match now {
        None => Some("process_absent"),
        Some(now) if now.start_time != before.start_time => Some("pid_reused"),
        Some(now) if matches!(now.state, 'Z' | 'X') => Some("zombie"),
        Some(_) => None,
    }
}

#[derive(Debug)]
pub(crate) struct RunningExecutable {
    pub path: PathBuf,
    pub comm: Option<String>,
    pub metadata: Option<fs::Metadata>,
}

impl RunningExecutable {
    pub(crate) fn is_system_program(&self, name: &str) -> bool {
        self.comm.as_deref() == Some(name)
            && self.path.file_name().and_then(|file| file.to_str()) == Some(name)
            && self
                .metadata
                .as_ref()
                .is_some_and(|meta| is_system_owned(meta.uid(), meta.mode()))
    }
}

pub(crate) fn running_executable(pid: u32) -> Option<RunningExecutable> {
    let link = format!("/proc/{pid}/exe");
    let reported = fs::read_link(&link).ok()?;
    Some(judge(pid, &link).unwrap_or(RunningExecutable {
        path: reported,
        comm: None,
        metadata: None,
    }))
}

fn judge(pid: u32, link: &str) -> Option<RunningExecutable> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let instance = process_instance_id(pid)?;
    let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|comm| comm.trim().to_owned());
    let image = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(link)
        .ok()?;
    let running = image.metadata().ok()?;
    let reported = fs::read_link(format!("/proc/self/fd/{}", image.as_raw_fd())).ok()?;
    if process_instance_id(pid)? != instance {
        return None;
    }
    let (path, metadata) = match path_before_replacement(&reported, running.nlink() == 0) {
        Some(original) => {
            if !same_mount_namespace(pid) || !mounted_in_our_namespace(image.as_raw_fd()) {
                return None;
            }
            (original, running)
        }
        None => {
            let at_path = fs::metadata(&reported).ok()?;
            if (at_path.dev(), at_path.ino()) != (running.dev(), running.ino()) {
                return None;
            }
            (reported, at_path)
        }
    };
    Some(RunningExecutable {
        path,
        comm,
        metadata: Some(metadata),
    })
}

fn path_before_replacement(reported: &Path, unlinked: bool) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let original = unlinked
        .then_some(reported.as_os_str().as_bytes())?
        .strip_suffix(b" (deleted)")?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(original)))
}

fn mounted_in_our_namespace(fd: std::os::fd::RawFd) -> bool {
    let Some(id) = fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
        .ok()
        .and_then(|info| {
            info.lines()
                .find_map(|line| line.strip_prefix("mnt_id:"))
                .and_then(|id| id.trim().parse::<u64>().ok())
        })
    else {
        return false;
    };
    fs::read_to_string("/proc/self/mountinfo").is_ok_and(|table| {
        table.lines().any(|line| {
            line.split(' ')
                .next()
                .and_then(|first| first.parse::<u64>().ok())
                == Some(id)
        })
    })
}

fn same_mount_namespace(pid: u32) -> bool {
    match (
        fs::read_link(format!("/proc/{pid}/ns/mnt")),
        fs::read_link("/proc/self/ns/mnt"),
    ) {
        (Ok(theirs), Ok(ours)) => theirs == ours,
        _ => false,
    }
}

fn is_system_owned(uid: u32, mode: u32) -> bool {
    uid == 0 && mode & 0o022 == 0
}

/// Return all running processes by reading /proc/<pid>/status.
pub fn list_processes() -> Vec<ProcessInfo> {
    let mut result = Vec::new();
    let proc_dir = Path::new("/proc");
    let entries = match fs::read_dir(proc_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid_str = name.to_string_lossy();
        let pid: u32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue, // Skip non-numeric entries.
        };

        let status_path = proc_dir.join(&*pid_str).join("status");
        let status = match fs::read_to_string(&status_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if !is_process_live_state(&status) {
            continue;
        }

        let proc_name = status
            .lines()
            .find(|l| l.starts_with("Name:"))
            .map(|l| l[5..].trim().to_owned())
            .unwrap_or_default();

        let cmdline_path = proc_dir.join(&*pid_str).join("cmdline");
        let cmdline = fs::read(cmdline_path)
            .ok()
            .map(|b| {
                // cmdline is NUL-separated; first entry is argv[0].
                let s = String::from_utf8_lossy(&b);
                s.split('\0').next().unwrap_or("").trim().to_owned()
            })
            .unwrap_or_default();

        result.push(ProcessInfo {
            pid,
            name: proc_name,
            cmdline,
        });
    }

    result.sort_by_key(|p| p.pid);
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    #[test]
    fn a_replaced_programs_path_loses_the_kernels_deleted_suffix_only_when_unlinked() {
        use std::os::unix::ffi::OsStrExt;
        let replaced = PathBuf::from("/usr/bin/gnome-shell (deleted)");
        assert_eq!(
            super::path_before_replacement(&replaced, true),
            Some(PathBuf::from("/usr/bin/gnome-shell"))
        );
        assert_eq!(super::path_before_replacement(&replaced, false), None);
        assert_eq!(
            super::path_before_replacement(std::path::Path::new("/usr/bin/gnome-shell"), true),
            None
        );
        let raw = std::ffi::OsStr::from_bytes(b"/opt/\xff/gnome-shell (deleted)");
        assert_eq!(
            super::path_before_replacement(std::path::Path::new(raw), true),
            Some(PathBuf::from(std::ffi::OsStr::from_bytes(
                b"/opt/\xff/gnome-shell"
            )))
        );
    }

    #[test]
    fn only_root_owned_images_writable_by_no_one_else_are_system_programs() {
        assert!(super::is_system_owned(0, 0o100755));
        assert!(super::is_system_owned(0, 0o100555));
        assert!(!super::is_system_owned(1000, 0o100755));
        assert!(!super::is_system_owned(0, 0o100775));
        assert!(!super::is_system_owned(0, 0o100757));
    }

    pub(crate) struct ReplacedProgram {
        pub child: std::process::Child,
        pub path: PathBuf,
        _dir: tempfile::TempDir,
    }

    impl Drop for ReplacedProgram {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct Reaped(std::process::Child);

    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }


    fn wait_for_exec(pid: u32) {
        let ours = std::fs::read_link("/proc/self/exe").expect("our image");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == ours) {
            assert!(
                std::time::Instant::now() < deadline,
                "child {pid} never exec'd"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            std::fs::read_link(format!("/proc/{pid}/exe")).is_ok(),
            "child {pid} exited right after exec"
        );
    }

    pub(crate) fn replaced_program(
        name: &str,
        own_mount_namespace: bool,
    ) -> Option<(
        ReplacedProgram,
        super::RunningExecutable,
        super::RunningExecutable,
    )> {
        use std::os::unix::process::CommandExt;
        let sleep = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|dir| dir.join("sleep"))
            .find(|candidate| candidate.is_file())
            .expect("sleep on PATH");
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        std::fs::copy(&sleep, &path).expect("copy sleep");
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod copy");
        let mut attempts = 0;
        let child = loop {
            let mut command = std::process::Command::new(&path);
            command.arg0("sleep");
            command.arg("30").stdin(std::process::Stdio::null());
            if own_mount_namespace {
                unsafe {
                    command.pre_exec(|| {
                        if libc::unshare(libc::CLONE_NEWNS) == 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    });
                }
            }
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) if own_mount_namespace && error.raw_os_error() == Some(libc::EPERM) => {
                    return None;
                }
                Err(error) => panic!("spawn copy: {error}"),
            }
        };
        let program = ReplacedProgram {
            child,
            path,
            _dir: dir,
        };
        let pid = program.child.id();
        wait_for_exec(pid);
        let before = super::running_executable(pid).expect("running executable");
        assert_eq!(before.path, program.path);
        std::fs::remove_file(&program.path).expect("unlink the running program");
        let after = super::running_executable(pid).expect("running executable after unlink");
        Some((program, before, after))
    }

    fn root_owned_copy() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn a_running_program_whose_file_was_replaced_keeps_its_path_and_image() {
        use std::os::unix::fs::MetadataExt;
        let (program, before, after) = replaced_program("gnome-shell", false).expect("spawn");
        assert_eq!(before.comm.as_deref(), Some("gnome-shell"));
        assert_eq!(before.is_system_program("gnome-shell"), root_owned_copy());
        assert_eq!(
            after.path, program.path,
            "the deleted suffix must be dropped"
        );
        let meta = after.metadata.as_ref().expect("running image metadata");
        assert_eq!(meta.nlink(), 0);
        assert_eq!(after.is_system_program("gnome-shell"), root_owned_copy());
        assert!(!after.is_system_program("kwin_wayland"));
    }

    #[test]
    fn a_replaced_program_in_another_mount_namespace_is_never_trusted() {
        let Some((_program, before, after)) = replaced_program("gnome-shell", true) else {
            eprintln!("skipped: creating a mount namespace needs root or `unshare -r`");
            return;
        };
        assert_eq!(before.is_system_program("gnome-shell"), root_owned_copy());
        assert!(after.metadata.is_none());
        assert!(!after.is_system_program("gnome-shell"));
    }

    #[test]
    fn a_replaced_program_run_from_another_mount_namespace_is_never_trusted() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::process::CommandExt;
        let sleep = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|dir| dir.join("sleep"))
            .find(|candidate| candidate.is_file())
            .expect("sleep on PATH");
        let dir = tempfile::tempdir().expect("temp dir");
        let program = dir.path().join("program");
        std::fs::copy(&sleep, &program).expect("copy sleep");
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("chmod copy");
        let image = std::fs::File::open(&program).expect("open the program in our namespace");
        let fd = image.as_raw_fd();
        let alias = dir.path().join("gnome-shell");
        std::os::unix::fs::symlink(format!("/proc/self/fd/{fd}"), &alias).expect("symlink");
        let mut attempts = 0;
        let child = Reaped(loop {
            let mut command = std::process::Command::new(&alias);
            command.arg0("sleep");
            command.arg("30").stdin(std::process::Stdio::null());
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) != 0
                        || libc::unshare(libc::CLONE_NEWNS) != 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                    eprintln!("skipped: creating a mount namespace needs root or `unshare -r`");
                    return;
                }
                Err(error) => panic!("spawn: {error}"),
            }
        });
        let pid = child.0.id();
        wait_for_exec(pid);
        drop(image);
        std::fs::remove_file(&program).expect("delete the program");
        let judged = super::running_executable(pid).expect("running executable");
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).expect("child comm");
        let theirs =
            std::fs::read_link(format!("/proc/{pid}/ns/mnt")).expect("child mount namespace");
        let ours = std::fs::read_link("/proc/self/ns/mnt").expect("our mount namespace");
        let exe = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH)
            .open(format!("/proc/{pid}/exe"))
            .expect("the child's image");
        let image_mount_is_ours = super::mounted_in_our_namespace(exe.as_raw_fd());
        drop(child);
        assert_eq!(comm.trim(), "gnome-shell");
        assert_ne!(
            theirs, ours,
            "the child must run in its own mount namespace"
        );
        assert!(
            image_mount_is_ours,
            "the image must come from one of our mounts"
        );
        assert!(judged.metadata.is_none());
        assert!(!judged.is_system_program("gnome-shell"));
    }

    #[test]
    fn a_program_run_from_behind_a_private_mount_is_judged_by_what_it_runs() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::process::CommandExt;
        let sleep = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|dir| dir.join("sleep"))
            .find(|candidate| candidate.is_file())
            .expect("sleep on PATH");
        let dir = tempfile::tempdir().expect("temp dir");
        let trusted = dir.path().join("gnome-shell");
        let impostor = dir.path().join("impostor");
        for path in [&trusted, &impostor] {
            std::fs::copy(&sleep, path).expect("copy sleep");
            std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .expect("chmod copy");
        }
        let target = CString::new(trusted.as_os_str().as_bytes()).unwrap();
        let source = CString::new(impostor.as_os_str().as_bytes()).unwrap();
        let mut attempts = 0;
        let child = Reaped(loop {
            let (target, source) = (target.clone(), source.clone());
            let mut command = std::process::Command::new(&trusted);
            command.arg0("sleep");
            command.arg("30").stdin(std::process::Stdio::null());
            unsafe {
                command.pre_exec(move || {
                    let root = b"/\0";
                    if libc::unshare(libc::CLONE_NEWNS) != 0
                        || libc::mount(
                            std::ptr::null(),
                            root.as_ptr().cast(),
                            std::ptr::null(),
                            libc::MS_REC | libc::MS_PRIVATE,
                            std::ptr::null(),
                        ) != 0
                        || libc::mount(
                            source.as_ptr(),
                            target.as_ptr(),
                            std::ptr::null(),
                            libc::MS_BIND,
                            std::ptr::null(),
                        ) != 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                    eprintln!("skipped: private mounts need root or `unshare -r`");
                    return;
                }
                Err(error) => panic!("spawn behind a private mount: {error}"),
            }
        });
        wait_for_exec(child.0.id());
        let judged = super::running_executable(child.0.id()).expect("running executable");
        drop(child);
        assert_eq!(
            judged.path, trusted,
            "the kernel reports the mounted-over path"
        );
        assert!(judged.metadata.is_none());
        assert!(!judged.is_system_program("gnome-shell"));
    }

    #[test]
    fn a_replaced_program_run_through_another_namespaces_mount_is_never_trusted() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::process::CommandExt;
        let sleep = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|dir| dir.join("sleep"))
            .find(|candidate| candidate.is_file())
            .expect("sleep on PATH");
        let dir = tempfile::tempdir().expect("temp dir");
        let named = dir.path().join("gnome-shell");
        let program = dir.path().join("program");
        for path in [&named, &program] {
            std::fs::copy(&sleep, path).expect("copy sleep");
            std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .expect("chmod copy");
        }
        let spawn = |command: &mut std::process::Command| {
            let mut attempts = 0;
            loop {
                match command.spawn() {
                    Ok(child) => return Some(child),
                    Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 100 => {
                        attempts += 1;
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) if error.raw_os_error() == Some(libc::EPERM) => return None,
                    Err(error) => panic!("spawn: {error}"),
                }
            }
        };
        let (target, source) = (
            CString::new(named.as_os_str().as_bytes()).unwrap(),
            CString::new(program.as_os_str().as_bytes()).unwrap(),
        );
        let mut helper = std::process::Command::new(&named);
        helper.arg0("sleep");
        helper.arg("30").stdin(std::process::Stdio::null());
        unsafe {
            helper.pre_exec(move || {
                let root = b"/\0";
                if libc::unshare(libc::CLONE_NEWNS) != 0
                    || libc::mount(
                        std::ptr::null(),
                        root.as_ptr().cast(),
                        std::ptr::null(),
                        libc::MS_REC | libc::MS_PRIVATE,
                        std::ptr::null(),
                    ) != 0
                    || libc::mount(
                        source.as_ptr(),
                        target.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let Some(helper) = spawn(&mut helper).map(Reaped) else {
            eprintln!("skipped: private mounts need root or `unshare -r`");
            return;
        };
        wait_for_exec(helper.0.id());
        let alias_dir = tempfile::tempdir().expect("temp dir");
        let alias = alias_dir.path().join("gnome-shell");
        std::os::unix::fs::symlink(format!("/proc/{}/exe", helper.0.id()), &alias)
            .expect("symlink");
        let mut runner = std::process::Command::new(&alias);
        runner.arg0("sleep");
        runner.arg("30").stdin(std::process::Stdio::null());
        let runner = Reaped(spawn(&mut runner).expect("run through the helper's mount"));
        let pid = runner.0.id();
        wait_for_exec(pid);
        std::fs::remove_file(&program).expect("delete the program");
        let judged = super::running_executable(pid).expect("running executable");
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).expect("runner comm");
        let raw = std::fs::read_link(format!("/proc/{pid}/exe")).expect("runner image");
        let theirs =
            std::fs::read_link(format!("/proc/{pid}/ns/mnt")).expect("runner mount namespace");
        let ours = std::fs::read_link("/proc/self/ns/mnt").expect("our mount namespace");
        let runner_in_our_namespace = theirs == ours;
        drop((runner, helper));
        assert_eq!(comm.trim(), "gnome-shell");
        assert!(
            runner_in_our_namespace,
            "the runner must share the driver's namespace"
        );
        assert!(
            raw.as_os_str()
                .as_bytes()
                .ends_with(b"gnome-shell (deleted)"),
            "the kernel reports the private mount's name: {raw:?}"
        );
        assert!(judged.metadata.is_none());
        assert!(!judged.is_system_program("gnome-shell"));
    }

    #[test]
    fn process_start_time_handles_spaces_in_comm() {
        let mut fields = vec!["S".to_owned()];
        fields.extend((4..=21).map(|field| field.to_string()));
        fields.push("987654".to_owned());
        let fields = fields.join(" ");
        for comm in [
            "fixture with spaces",
            "name with ) parentheses",
            "ends with )",
        ] {
            let stat = format!("42 ({comm}) {fields} 23");
            assert_eq!(
                super::process_start_time_from_stat(&stat),
                Some(987654),
                "{comm}"
            );
        }
        assert_eq!(super::process_start_time_from_stat("bad"), None);
        assert_eq!(
            super::process_start_time_from_stat("42 (short) S 1 2"),
            None
        );
    }

    #[test]
    fn exit_observation_tells_the_same_live_instance_from_its_end() {
        let stat = |state| super::ProcessStat {
            state,
            start_time: 7,
        };
        let parsed = super::process_stat_from_stat(&format!(
            "42 (a) b) S {} 7 23",
            (4..=21)
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        assert_eq!(parsed, Some(stat('S')));
        assert_eq!(
            super::process_exit_observation(stat('S'), Some(stat('R'))),
            None
        );
        assert_eq!(
            super::process_exit_observation(stat('S'), Some(stat('Z'))),
            Some("zombie")
        );
        assert_eq!(
            super::process_exit_observation(stat('S'), None),
            Some("process_absent")
        );
        let reused = super::ProcessStat {
            state: 'S',
            start_time: 8,
        };
        assert_eq!(
            super::process_exit_observation(stat('S'), Some(reused)),
            Some("pid_reused")
        );
    }

    #[test]
    fn process_states_fail_closed() {
        assert!(super::is_process_live_state(
            "Name:\ttest\nState:\tR (running)\n"
        ));
        assert!(super::is_process_live_state("State:\tS (sleeping)"));
        assert!(!super::is_process_live_state("State:\tZ (zombie)"));
        assert!(!super::is_process_live_state("State:\tX (dead)"));
        assert!(!super::is_process_live_state("Name:\ttest\n"));
        assert!(!super::is_process_live_state("State:\t"));
    }

    #[test]
    fn real_zombie_is_not_live_and_disappears_after_reaping() {
        let child = unsafe { libc::fork() };
        assert!(
            child >= 0,
            "fork failed: {}",
            std::io::Error::last_os_error()
        );
        if child == 0 {
            unsafe { libc::_exit(0) };
        }

        let pid = child as u32;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut observed_zombie = false;
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .is_some_and(|status| status.contains("State:\tZ"))
            {
                observed_zombie = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let live_while_zombie = super::is_process_live(pid);
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(child, &mut status, 0) };

        assert!(observed_zombie, "child never entered zombie state");
        assert!(!live_while_zombie, "zombie pid was reported live");
        assert_eq!(reaped, child, "failed to reap child");
        assert!(!super::is_process_live(pid), "reaped pid was reported live");
    }
}
