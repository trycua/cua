// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Child processes the driver owns.
//!
//! [`ManagedChild::builder`] collects how a child relates to its owner and
//! applies the matching OS mechanism at spawn:
//!
//! - [`detached`](ManagedChildBuilder::detached): the child outlives the
//!   owner. Unix puts it in its own process group; Windows starts it with
//!   `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` and no inherited handles.
//! - [`no_window`](ManagedChildBuilder::no_window): Windows `CREATE_NO_WINDOW`,
//!   so a console child does not open a console window. No effect elsewhere.
//! - [`kill_with_owner`](ManagedChildBuilder::kill_with_owner): the child and
//!   every process it starts die with the [`ManagedChild`]. Windows starts the
//!   child suspended, assigns it to a `KILL_ON_JOB_CLOSE` job and then resumes
//!   it, so nothing escapes the job; the job also dies with the owner process.
//!   Unix puts the child in its own process group and kills that group when
//!   the `ManagedChild` is dropped or killed; an owner that dies without
//!   unwinding leaves the group running.
//! - [`reap_on_drop`](ManagedChildBuilder::reap_on_drop): dropping a child
//!   that is still running kills and waits for it, so no zombie remains.
//!
//! This is process ownership, not a security boundary. Sandboxed children
//! (`perception_client::containment`) build their own spawn.

use std::io;
use std::ops::{Deref, DerefMut};
use std::process::{Child, Command, Output};

/// Options for an owned child. See the [module docs](self).
#[derive(Debug, Clone, Copy, Default)]
#[must_use]
pub struct ManagedChildBuilder {
    no_window: bool,
    kill_with_owner: bool,
    reap_on_drop: bool,
}

/// Options for a child that outlives its owner.
#[derive(Debug, Clone, Copy, Default)]
#[must_use]
pub struct DetachedBuilder {
    no_window: bool,
}

/// A spawned child and whatever ties it to its owner. Dereferences to the
/// [`Child`]; [`kill`](Self::kill) and [`wait_with_output`](Self::wait_with_output)
/// replace the `Child` methods of the same name so they keep that tie.
pub struct ManagedChild {
    child: Option<Child>,
    owner: Option<Owner>,
    reap_on_drop: bool,
}

impl ManagedChild {
    pub fn builder() -> ManagedChildBuilder {
        ManagedChildBuilder::default()
    }

    /// Kill the child, and with [`kill_with_owner`](ManagedChildBuilder::kill_with_owner)
    /// every process it started.
    pub fn kill(&mut self) -> io::Result<()> {
        if let Some(owner) = &self.owner {
            owner.kill();
        }
        self.child_mut().kill()
    }

    /// [`Child::wait_with_output`]. Processes the child started stay under
    /// the owner tie until the wait returns, then are killed with it.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        self.child
            .take()
            .expect("child is present until consumed")
            .wait_with_output()
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("child is present until consumed")
    }
}

impl Deref for ManagedChild {
    type Target = Child;

    fn deref(&self) -> &Child {
        self.child
            .as_ref()
            .expect("child is present until consumed")
    }
}

impl DerefMut for ManagedChild {
    fn deref_mut(&mut self) -> &mut Child {
        self.child_mut()
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            owner.kill();
        }
        if let Some(mut child) = self.child.take() {
            if self.reap_on_drop && !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl ManagedChildBuilder {
    /// Spawn a child that outlives its owner. See the [module docs](self).
    pub fn detached(self) -> DetachedBuilder {
        DetachedBuilder {
            no_window: self.no_window,
        }
    }

    pub fn no_window(mut self) -> Self {
        self.no_window = true;
        self
    }

    pub fn kill_with_owner(mut self) -> Self {
        self.kill_with_owner = true;
        self
    }

    pub fn reap_on_drop(mut self) -> Self {
        self.reap_on_drop = true;
        self
    }

    /// Apply the options that take effect before spawn to a command the
    /// caller spawns through another API, such as `tokio::process::Command`.
    ///
    /// # Panics
    ///
    /// With [`kill_with_owner`](Self::kill_with_owner) or
    /// [`reap_on_drop`](Self::reap_on_drop), which need [`spawn`](Self::spawn).
    pub fn configure(self, command: &mut Command) {
        assert!(
            !self.kill_with_owner && !self.reap_on_drop,
            "kill_with_owner and reap_on_drop need ManagedChildBuilder::spawn"
        );
        self.prepare(command);
    }

    pub fn spawn(self, command: &mut Command) -> io::Result<ManagedChild> {
        self.prepare(command);
        #[cfg(windows)]
        let (child, owner) = if self.kill_with_owner {
            let (child, job) = win::spawn_in_job(command)?;
            (child, Some(Owner::Job(job)))
        } else {
            (command.spawn()?, None)
        };
        #[cfg(unix)]
        let (child, owner) = {
            let child = command.spawn()?;
            let owner = if self.kill_with_owner {
                process_group_target(child.id()).map(Owner::Group)
            } else {
                None
            };
            (child, owner)
        };
        Ok(ManagedChild {
            child: Some(child),
            owner,
            reap_on_drop: self.reap_on_drop,
        })
    }

    fn prepare(self, command: &mut Command) {
        #[cfg(unix)]
        if self.kill_with_owner {
            std::os::unix::process::CommandExt::process_group(command, 0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            let mut flags = 0;
            if self.no_window {
                flags |= win::CREATE_NO_WINDOW;
            }
            if self.kill_with_owner {
                flags |= win::CREATE_SUSPENDED;
            }
            if flags != 0 {
                command.creation_flags(flags);
            }
        }
        #[cfg(not(windows))]
        let _ = self.no_window;
    }
}

impl DetachedBuilder {
    pub fn no_window(mut self) -> Self {
        self.no_window = true;
        self
    }

    /// Spawn the child and return its pid. The child is never waited for.
    ///
    /// On Windows the child inherits no handles, so `command`'s stdio
    /// settings do not apply, and it runs in the owner's environment and
    /// working directory: a command that sets either is refused with
    /// [`io::ErrorKind::InvalidInput`].
    pub fn spawn(self, command: &mut Command) -> io::Result<u32> {
        #[cfg(unix)]
        {
            let _ = self.no_window;
            std::os::unix::process::CommandExt::process_group(command, 0);
            Ok(command.spawn()?.id())
        }
        #[cfg(windows)]
        {
            win::spawn_detached(command, self.no_window)
        }
    }
}

enum Owner {
    #[cfg(unix)]
    Group(i32),
    #[cfg(windows)]
    Job(win::Job),
}

impl Owner {
    fn kill(&self) {
        match self {
            #[cfg(unix)]
            Owner::Group(group) => {
                // SAFETY: kill(2) with a negative pid signals exactly that
                // process group; `process_group_target` never yields 0 or -1.
                let _ = unsafe { libc::kill(*group, libc::SIGKILL) };
            }
            #[cfg(windows)]
            Owner::Job(job) => job.terminate(),
        }
    }
}

/// The kill(2) target for the process group led by `pid`. Never 0 (the
/// caller's own group) or -1 (every process the caller may signal).
#[cfg(unix)]
fn process_group_target(pid: u32) -> Option<i32> {
    let pid = i32::try_from(pid).ok()?;
    (pid > 1).then_some(-pid)
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        CreateProcessW, OpenThread, ResumeThread, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW, THREAD_SUSPEND_RESUME,
    };

    pub(super) const CREATE_NO_WINDOW: u32 = windows::Win32::System::Threading::CREATE_NO_WINDOW.0;
    pub(super) const CREATE_SUSPENDED: u32 = windows::Win32::System::Threading::CREATE_SUSPENDED.0;

    /// A kill-on-close job. The handle is kept as an integer so the child
    /// stays `Send`.
    pub(super) struct Job(isize);

    impl Job {
        pub(super) fn handle(&self) -> HANDLE {
            HANDLE(self.0 as _)
        }

        pub(super) fn terminate(&self) {
            let _ = unsafe { TerminateJobObject(self.handle(), 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.handle()) };
        }
    }

    /// Spawn `command` (already flagged `CREATE_SUSPENDED`), place it in a new
    /// job, then resume it, so a shim cannot start a process outside the job.
    pub(super) fn spawn_in_job(command: &mut Command) -> io::Result<(Child, Job)> {
        let job = unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null())?;
            let job = Job(handle.0 as isize);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            job
        };
        let mut child = command.spawn()?;
        let started = unsafe {
            AssignProcessToJobObject(job.handle(), HANDLE(child.as_raw_handle() as _))
                .map_err(io::Error::from)
                .and_then(|()| resume(child.id()))
        };
        if let Err(error) = started {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, job))
    }

    /// Resume the only thread of a process created suspended. `Child` does
    /// not expose its primary thread handle, so find it by owner pid.
    unsafe fn resume(pid: u32) -> io::Result<()> {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut thread_id = None;
        let mut more = Thread32First(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                thread_id = Some(entry.th32ThreadID);
                break;
            }
            more = Thread32Next(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        let thread_id = thread_id
            .ok_or_else(|| io::Error::other(format!("no thread found for suspended {pid}")))?;
        let thread = OpenThread(THREAD_SUSPEND_RESUME, false, thread_id)?;
        let resumed = ResumeThread(thread);
        let _ = CloseHandle(thread);
        if resumed == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn spawn_detached(command: &Command, no_window: bool) -> io::Result<u32> {
        if command.get_envs().next().is_some() || command.get_current_dir().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a detached Windows child runs in the owner's environment and working directory",
            ));
        }
        let program = command
            .get_program()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut command_line = command_line(command);
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut flags = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS;
        if no_window {
            flags |= PROCESS_CREATION_FLAGS(CREATE_NO_WINDOW);
        }
        let mut process = PROCESS_INFORMATION::default();
        // Rust's Command::output gives a short-lived owner inheritable capture
        // handles. Command::spawn can leak those unrelated handles into the
        // long-lived child, so the owner's caller never observes EOF.
        // CreateProcess with handle inheritance disabled is the boundary here.
        unsafe {
            CreateProcessW(
                PCWSTR(program.as_ptr()),
                PWSTR(command_line.as_mut_ptr()),
                None,
                None,
                BOOL::from(false),
                flags,
                None,
                PCWSTR::null(),
                &startup,
                &mut process,
            )?;
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        Ok(process.dwProcessId)
    }

    fn command_line(command: &Command) -> Vec<u16> {
        let mut line = Vec::new();
        for argument in std::iter::once(command.get_program()).chain(command.get_args()) {
            if !line.is_empty() {
                line.push(' ' as u16);
            }
            append_argument(&mut line, argument.encode_wide());
        }
        line.push(0);
        line
    }

    fn append_argument(line: &mut Vec<u16>, argument: impl IntoIterator<Item = u16>) {
        let argument = argument.into_iter().collect::<Vec<_>>();
        let quote = argument.is_empty()
            || argument
                .iter()
                .any(|value| matches!(*value, 0x20 | 0x09 | 0x22));
        if !quote {
            line.extend(argument);
            return;
        }
        line.push('"' as u16);
        let mut backslashes = 0;
        for value in argument {
            if value == '\\' as u16 {
                backslashes += 1;
            } else if value == '"' as u16 {
                line.extend(std::iter::repeat_n('\\' as u16, backslashes * 2 + 1));
                line.push(value);
                backslashes = 0;
            } else {
                line.extend(std::iter::repeat_n('\\' as u16, backslashes));
                line.push(value);
                backslashes = 0;
            }
        }
        line.extend(std::iter::repeat_n('\\' as u16, backslashes * 2));
        line.push('"' as u16);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{Duration, Instant};
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::JobObjects::{
            JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
        };
        use windows::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };

        #[repr(C)]
        struct ProcessIds {
            list: JOBOBJECT_BASIC_PROCESS_ID_LIST,
            more: [usize; 15],
        }

        fn job_pids(job: &Job) -> Vec<u32> {
            let mut ids: ProcessIds = unsafe { std::mem::zeroed() };
            unsafe {
                QueryInformationJobObject(
                    job.handle(),
                    JobObjectBasicProcessIdList,
                    &mut ids as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<ProcessIds>() as u32,
                    None,
                )
            }
            .expect("query job process ids");
            let count = ids.list.NumberOfProcessIdsInList as usize;
            let base = std::ptr::addr_of!(ids.list.ProcessIdList) as *const usize;
            (0..count)
                .map(|index| unsafe { *base.add(index) } as u32)
                .collect()
        }

        #[test]
        fn dropping_a_kill_with_owner_child_ends_the_processes_it_started() {
            let mut command = Command::new("cmd.exe");
            command.args(["/d", "/c", "ping -n 60 127.0.0.1 >nul"]);
            let child = super::super::ManagedChild::builder()
                .no_window()
                .kill_with_owner()
                .reap_on_drop()
                .spawn(&mut command)
                .expect("spawn cmd in a job");
            let Some(super::super::Owner::Job(job)) = &child.owner else {
                panic!("kill_with_owner must create a job");
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            let pids = loop {
                let pids = job_pids(job);
                if pids.len() >= 2 {
                    break pids;
                }
                assert!(
                    Instant::now() < deadline,
                    "ping never joined the job: {pids:?}"
                );
                std::thread::sleep(Duration::from_millis(50));
            };
            let handles = pids
                .iter()
                .map(|pid| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, *pid) }.unwrap())
                .collect::<Vec<_>>();
            drop(child);
            for (pid, handle) in pids.iter().zip(handles) {
                let waited = unsafe { WaitForSingleObject(handle, 10_000) };
                let _ = unsafe { CloseHandle(handle) };
                assert_eq!(waited, WAIT_OBJECT_0, "job process {pid} outlived the drop");
            }
        }

        #[test]
        fn command_line_quotes_like_the_msvc_runtime_parses() {
            let mut command = Command::new(r"C:\Program Files\cua\cua-driver.exe");
            command.args(["serve", "", r#"a "b""#, r"c d\", r"e\f", "plain"]);
            let line = String::from_utf16(&command_line(&command)).unwrap();
            assert_eq!(
                line,
                "\"C:\\Program Files\\cua\\cua-driver.exe\" serve \"\" \"a \\\"b\\\"\" \"c d\\\\\" e\\f plain\0"
            );
        }

        #[test]
        fn detached_spawn_refuses_an_environment_it_cannot_carry() {
            let mut command = Command::new("cmd.exe");
            command.env("CUA_PROCESS_TEST", "1");
            let error = super::super::ManagedChild::builder()
                .detached()
                .spawn(&mut command)
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::BufRead as _;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that `pid` exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Spawn `sh` that starts a background `sleep` and reports its pid, so the
    /// test owns a child that has a child of its own.
    fn spawn_shell_with_grandchild(builder: ManagedChildBuilder) -> (ManagedChild, i32) {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 60 & echo $!; wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = builder.spawn(&mut command).expect("spawn sh");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        (child, line.trim().parse().expect("grandchild pid"))
    }

    fn wait_gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn dropping_a_kill_with_owner_child_kills_its_process_group() {
        let (child, grandchild) =
            spawn_shell_with_grandchild(ManagedChild::builder().kill_with_owner().reap_on_drop());
        let leader = child.id() as i32;
        assert!(alive(grandchild));
        drop(child);
        assert!(!alive(leader), "reap_on_drop must leave no zombie leader");
        assert!(
            wait_gone(grandchild),
            "grandchild {grandchild} outlived the drop"
        );
    }

    #[test]
    fn kill_reaches_the_whole_group() {
        let (mut child, grandchild) =
            spawn_shell_with_grandchild(ManagedChild::builder().kill_with_owner());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            wait_gone(grandchild),
            "grandchild {grandchild} outlived kill"
        );
    }

    #[test]
    fn reap_on_drop_alone_kills_only_the_child() {
        let (child, grandchild) =
            spawn_shell_with_grandchild(ManagedChild::builder().reap_on_drop());
        let leader = child.id() as i32;
        drop(child);
        assert!(!alive(leader), "reap_on_drop must reap the child");
        assert!(
            alive(grandchild),
            "without kill_with_owner the group is left alone"
        );
        // SAFETY: the test started this sleep and still owns it.
        unsafe { libc::kill(grandchild, libc::SIGKILL) };
    }

    #[test]
    fn process_group_target_is_exact_and_never_all_processes() {
        assert_eq!(process_group_target(42), Some(-42));
        assert_eq!(process_group_target(0), None);
        assert_eq!(process_group_target(1), None);
        assert_eq!(process_group_target(u32::MAX), None);
    }

    #[test]
    #[should_panic(expected = "need ManagedChildBuilder::spawn")]
    fn configure_refuses_options_it_cannot_honor() {
        ManagedChild::builder()
            .kill_with_owner()
            .configure(&mut Command::new("true"));
    }
}
