//! Windows worker containment: a private Job Object that bounds the worker
//! tree and kills every descendant when the job handle closes.
//!
//! The child is created suspended and assigned to the job before its first
//! instruction runs, which removes the spawn-to-assignment race: a worker that
//! started running first could otherwise fork a descendant outside the job and
//! survive cleanup. The job is unnamed, so no other process can open it by
//! name, and `JOB_OBJECT_LIMIT_BREAKAWAY_OK` is deliberately not set, so a
//! descendant cannot leave the job on its own.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::Path;

use cua_driver_contract::VisualParseError;
use tokio::process::Child;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_TIME,
};
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, TerminateProcess, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    THREAD_SUSPEND_RESUME,
};

use super::{base_command, containment_error, spawn_error, ContainmentLimits};

/// Owns the job handle. Closing it terminates every process still in the job,
/// which is what makes cancellation, timeout, runtime shutdown, client drop and
/// idle cleanup reach the worker's descendants.
///
/// The handle is stored as an `isize` so the guard stays `Send` across the
/// awaits that hold a warm worker.
pub(super) struct Guard {
    job: isize,
}

impl Guard {
    fn new(job: HANDLE) -> Self {
        Self {
            job: job.0 as isize,
        }
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.job as *mut c_void)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}

pub(super) fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    limits: &ContainmentLimits,
) -> Result<(Child, Guard), VisualParseError> {
    // The job exists and is fully limited before anything is spawned, so a
    // configuration failure never leaves a running unconstrained worker.
    let guard = create_job(limits)?;

    let mut command = base_command(executable);
    command.args(args).current_dir(working_directory);
    command
        .as_std_mut()
        .creation_flags((CREATE_SUSPENDED.0 | CREATE_NO_WINDOW.0) as u32);

    let child = command.spawn().map_err(spawn_error)?;
    let Some(process) = child.raw_handle() else {
        return Err(containment_error(
            "the perception worker process handle was unavailable for job assignment",
            None,
        ));
    };
    let process = HANDLE(process as *mut c_void);
    let Some(pid) = child.id() else {
        terminate(process);
        return Err(containment_error(
            "the perception worker process id was unavailable for job assignment",
            None,
        ));
    };

    if let Err(cause) = unsafe { AssignProcessToJobObject(guard.handle(), process) } {
        terminate(process);
        return Err(containment_error(
            "failed to assign the perception worker to its containment job object",
            Some(format!("hresult {:#010x}", cause.code().0)),
        ));
    }
    if let Err(failure) = resume(pid) {
        terminate(process);
        return Err(failure);
    }
    Ok((child, guard))
}

fn create_job(limits: &ContainmentLimits) -> Result<Guard, VisualParseError> {
    // An unnamed job object is private to this process.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(|cause| {
        containment_error(
            "failed to create the perception worker containment job object",
            Some(format!("hresult {:#010x}", cause.code().0)),
        )
    })?;
    let guard = Guard::new(job);

    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_JOB_MEMORY
        | JOB_OBJECT_LIMIT_PROCESS_TIME;
    information.BasicLimitInformation.ActiveProcessLimit = limits.max_processes;
    // `PerProcessUserTimeLimit` counts in 100-nanosecond units.
    information.BasicLimitInformation.PerProcessUserTimeLimit =
        i64::try_from(limits.max_cpu_seconds().saturating_mul(10_000_000)).unwrap_or(i64::MAX);
    information.JobMemoryLimit = usize::try_from(limits.max_memory_bytes).unwrap_or(usize::MAX);

    unsafe {
        SetInformationJobObject(
            guard.handle(),
            JobObjectExtendedLimitInformation,
            &information as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(|cause| {
        containment_error(
            "failed to apply the perception worker containment job object limits",
            Some(format!("hresult {:#010x}", cause.code().0)),
        )
    })?;
    Ok(guard)
}

/// Resume the single thread a `CREATE_SUSPENDED` process owns. The toolhelp
/// snapshot is the supported way to reach it: `std::process::Child` does not
/// expose the primary thread handle.
fn resume(pid: u32) -> Result<(), VisualParseError> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(|cause| {
        containment_error(
            "failed to enumerate the perception worker threads after job assignment",
            Some(format!("hresult {:#010x}", cause.code().0)),
        )
    })?;
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut resumed = false;
    let mut more = unsafe { Thread32First(snapshot, &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == pid {
            if let Ok(thread) =
                unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
            {
                let previous = unsafe { ResumeThread(thread) };
                unsafe {
                    let _ = CloseHandle(thread);
                }
                resumed |= previous != u32::MAX;
            }
        }
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        more = unsafe { Thread32Next(snapshot, &mut entry) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    if !resumed {
        return Err(containment_error(
            "failed to resume the perception worker after job assignment",
            None,
        ));
    }
    Ok(())
}

fn terminate(process: HANDLE) {
    unsafe {
        let _ = TerminateProcess(process, 1);
    }
}
