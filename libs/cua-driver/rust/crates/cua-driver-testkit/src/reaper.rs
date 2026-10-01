//! Cross-platform child reaping. Kills every spawned child tree on drop:
//! Windows uses a kill-on-close Job Object, while Unix gives each test-owned
//! child its own process group and terminates that group explicitly.

use std::process::{Child, Command};
use std::time::Duration;

/// Owns spawned children and externally-discovered pids (e.g. a packaged app's
/// broker-launched window process), killing them all on drop. Prompt cleanup
/// between tests; the Windows Job Object is the hard-kill backstop.
pub struct ChildReaper {
    children: Vec<Child>,
    pids: Vec<u32>,
}

impl ChildReaper {
    pub fn new() -> Self {
        ChildReaper {
            children: Vec::new(),
            pids: Vec::new(),
        }
    }

    /// Spawn `cmd` into the kill-on-close job (Windows) and own the child.
    pub fn spawn(&mut self, cmd: &mut Command) -> std::io::Result<()> {
        let child = spawn_in_job(cmd)?;
        self.children.push(child);
        Ok(())
    }

    /// Take ownership of an already-spawned child (assigning it to the job on
    /// Windows). Use when you spawned via [`spawn_in_job`] to grab its pipes
    /// first, then hand the remainder here.
    pub fn push(&mut self, child: Child) {
        #[cfg(target_os = "windows")]
        win::assign_child(&child);
        self.children.push(child);
    }

    /// Track an external pid (and its whole tree) for teardown — packaged /
    /// broker-launched window processes that aren't our direct child.
    pub fn track_pid(&mut self, pid: u32) {
        #[cfg(target_os = "windows")]
        win::assign_pid(pid);
        self.pids.push(pid);
    }
}

impl Default for ChildReaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ChildReaper {
    fn drop(&mut self) {
        for &pid in &self.pids {
            tree_kill(pid);
        }
        for c in &mut self.children {
            #[cfg(unix)]
            process_group_kill(c.id());
            tree_kill(c.id());
            let _ = c.kill();
            let _ = c.wait();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Spawn a command, assigning it to the kill-on-close job on Windows so it can
/// never outlive the test process. On other platforms a plain spawn (the
/// [`ChildReaper`] still kills it on drop).
pub fn spawn_in_job(cmd: &mut Command) -> std::io::Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn()?;
    #[cfg(target_os = "windows")]
    win::assign_child(&child);
    Ok(child)
}

#[cfg(unix)]
fn process_group_kill(pid: u32) {
    let Some(group) = process_group_target(pid) else {
        return;
    };
    // A negative pid targets one process group. Do this directly: some `kill`
    // utilities parse `kill -9 -<pgid>` as `kill(-1, SIGKILL)` unless the
    // negative operand is protected with an implementation-specific `--`.
    unsafe {
        libc::kill(group, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn process_group_target(pid: u32) -> Option<i32> {
    let pid = i32::try_from(pid).ok()?;
    (pid > 1).then_some(-pid)
}

#[cfg(target_os = "windows")]
fn tree_kill(pid: u32) {
    use std::process::Stdio;
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(target_os = "windows"))]
fn tree_kill(pid: u32) {
    use std::process::Stdio;
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "windows")]
mod win {
    use core::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// Process-global job handle (stored as usize so it's Send/Sync in the
    /// OnceLock), created lazily with KILL_ON_JOB_CLOSE.
    static JOB: OnceLock<usize> = OnceLock::new();

    fn job() -> HANDLE {
        let raw = *JOB.get_or_init(|| unsafe {
            let h =
                CreateJobObjectW(None, windows::core::PCWSTR::null()).expect("CreateJobObjectW");
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let _ = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            h.0 as usize
        });
        HANDLE(raw as *mut c_void)
    }

    /// Assign one of our spawned children to the kill-on-close job.
    pub(super) fn assign_child(child: &Child) {
        unsafe {
            let h = HANDLE(child.as_raw_handle() as *mut c_void);
            if let Err(error) = AssignProcessToJobObject(job(), h) {
                eprintln!(
                    "[testkit] could not assign child {} to job: {error}",
                    child.id()
                );
            }
        }
    }

    /// Assign an already-running pid (broker-spawned window process) to the job.
    pub(super) fn assign_pid(pid: u32) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) {
                if !h.is_invalid() {
                    if let Err(error) = AssignProcessToJobObject(job(), h) {
                        eprintln!("[testkit] could not assign pid {pid} to job: {error}");
                    }
                    let _ = CloseHandle(h);
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::process_group_target;

    #[test]
    fn process_group_target_is_exact_and_never_all_processes() {
        assert_eq!(process_group_target(42), Some(-42));
        assert_eq!(process_group_target(0), None);
        assert_eq!(process_group_target(1), None);
        assert_eq!(process_group_target(u32::MAX), None);
    }
}
