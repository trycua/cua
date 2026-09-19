//! Windows worker containment.
//!
//! The worker runs as an AppContainer (LowBox) identity with no capabilities,
//! inside a private Job Object, on a private desktop, holding exactly three
//! inherited handles.
//!
//! Each piece closes a specific hole:
//!
//! - An AppContainer with an empty capability set is the documented Windows
//!   primitive that denies network access: without `internetClient` or
//!   `privateNetworkClientServer` the token cannot reach the transport stack at
//!   all. It also denies the user profile, the Credential Manager and DPAPI
//!   user secrets, because none of those objects grant the container's SID.
//!   The worker's own bundle, runtime, model and working-directory paths are
//!   granted to that exact SID and nothing else is.
//! - `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` replaces `bInheritHandles = TRUE`'s
//!   "every inheritable handle in the Driver" with exactly the two protocol
//!   pipes and the null device. The protocol pipes are private named pipes so
//!   the parent side can stay asynchronous while the child receives ordinary
//!   synchronous stdio handles.
//! - The Job Object carries kill-on-close, an active-process ceiling, a job
//!   memory ceiling, a per-process CPU-time ceiling and the full set of UI
//!   restrictions, so the worker cannot read or write the clipboard, use a
//!   USER handle it did not create, switch desktops, or change system
//!   parameters. `JOB_OBJECT_LIMIT_BREAKAWAY_OK` is deliberately not set.
//! - A private desktop means a screen capture or synthetic input attempt
//!   reaches an empty desktop rather than the user's session.
//!
//! The child is created suspended and is resumed only after the job, the
//! limits, the UI restrictions and every handle boundary are installed, so no
//! worker instruction runs before its containment exists. Any failure
//! terminates the suspended process and returns a fail-closed error.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::Path;

use cua_driver_contract::VisualParseError;
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};

use super::{
    containment_error, os_detail, spawn_error, ContainedChild, ContainmentLimits,
    FilesystemBoundary, RawExit,
};

pub(super) type WorkerStdin = NamedPipeServer;
pub(super) type WorkerStdout = NamedPipeServer;

type Handle = *mut c_void;
type Bool = i32;

const FALSE: Bool = 0;
const TRUE: Bool = 1;
const INFINITE: u32 = 0xFFFF_FFFF;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_EXECUTE: u32 = 0x2000_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_TAG_INFO_CLASS: u32 = 9;
const FILE_NAME_NORMALIZED: u32 = 0;

const CREATE_SUSPENDED: u32 = 0x0000_0004;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const STARTF_USESHOWWINDOW: u32 = 0x0000_0001;
const SW_HIDE: u16 = 0;

const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
const PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 0x0002_0009;

const JOB_OBJECT_LIMIT_PROCESS_TIME: u32 = 0x0000_0002;
const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x0000_0008;
const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x0000_0200;
const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x0000_0400;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

const JOB_OBJECT_BASIC_UI_RESTRICTIONS: u32 = 4;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

const JOB_OBJECT_UILIMIT_HANDLES: u32 = 0x0000_0001;
const JOB_OBJECT_UILIMIT_READCLIPBOARD: u32 = 0x0000_0002;
const JOB_OBJECT_UILIMIT_WRITECLIPBOARD: u32 = 0x0000_0004;
const JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS: u32 = 0x0000_0008;
const JOB_OBJECT_UILIMIT_DISPLAYSETTINGS: u32 = 0x0000_0010;
const JOB_OBJECT_UILIMIT_GLOBALATOMS: u32 = 0x0000_0020;
const JOB_OBJECT_UILIMIT_DESKTOP: u32 = 0x0000_0040;
const JOB_OBJECT_UILIMIT_EXITWINDOWS: u32 = 0x0000_0080;

/// Every UI restriction a Job Object can impose.
const JOB_OBJECT_UILIMIT_ALL: u32 = JOB_OBJECT_UILIMIT_HANDLES
    | JOB_OBJECT_UILIMIT_READCLIPBOARD
    | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
    | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
    | JOB_OBJECT_UILIMIT_GLOBALATOMS
    | JOB_OBJECT_UILIMIT_DESKTOP
    | JOB_OBJECT_UILIMIT_EXITWINDOWS;

const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
const SE_FILE_OBJECT: u32 = 1;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_USER_CLASS: u32 = 1;

const TRUSTEE_IS_SID: u32 = 0;
const TRUSTEE_IS_UNKNOWN: u32 = 0;
const NO_MULTIPLE_TRUSTEE: u32 = 0;
const GRANT_ACCESS: u32 = 1;
const NO_INHERITANCE: u32 = 0x0;
const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 0x3;

const ERROR_SUCCESS: u32 = 0;
const ERROR_INSUFFICIENT_BUFFER: i32 = 122;
/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const HRESULT_ALREADY_EXISTS: i32 = -2_147_024_713;

/// The rights a worker needs on its private desktop: it may create its own
/// windows and read its own desktop, but never record or play back journal
/// input, hook another thread, or switch the session to another desktop.
const DESKTOP_WORKER_ACCESS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0080;

/// Exit statuses that mean a containment ceiling ended the worker rather than a
/// fault in its own code.
const RESOURCE_LIMIT_EXITS: &[u32] = &[
    0xC000_0017, // STATUS_NO_MEMORY
    0xC000_0044, // STATUS_QUOTA_EXCEEDED
    0xC000_009A, // STATUS_INSUFFICIENT_RESOURCES
    0xC000_012D, // STATUS_COMMITMENT_LIMIT
];

/// A stable AppContainer identity for the perception worker. Every process the
/// local user deliberately launches with this identity receives the same ACL
/// grants, so the SID is an isolation principal rather than authentication.
/// Stability prevents one stale grant per Driver launch. Windows launch remains
/// fail-closed in `capabilities` until those grants are applied through verified
/// no-follow handles instead of mutable path names.
const APP_CONTAINER_NAME: &str = "com.trycua.cua-driver.perception.worker";
const APP_CONTAINER_DISPLAY_NAME: &str = "Cua Driver perception worker";
const APP_CONTAINER_DESCRIPTION: &str =
    "Contained inference worker for the optional Cua Driver perception extension.";

// Every type below mirrors a Win32 layout. Fields the operating system fills in
// are declared for correct size and offset even where this module never reads
// them, so the layouts are allowed to carry unread members.
#[allow(dead_code)]
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    security_descriptor: *mut c_void,
    inherit_handle: Bool,
}

/// A `SECURITY_DESCRIPTOR` in absolute form. The Win32 minimum is 40 bytes on a
/// 64-bit target; 64 pointer-aligned bytes is comfortably above it.
#[repr(C, align(8))]
struct SecurityDescriptorStorage([u8; 64]);

#[allow(dead_code)]
#[repr(C)]
struct StartupInfoW {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_count_chars: u32,
    y_count_chars: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
    reserved2_size: u16,
    reserved2: *mut u8,
    std_input: Handle,
    std_output: Handle,
    std_error: Handle,
}

#[allow(dead_code)]
#[repr(C)]
struct StartupInfoExW {
    startup_info: StartupInfoW,
    attribute_list: *mut c_void,
}

#[allow(dead_code)]
#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[allow(dead_code)]
#[repr(C)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[allow(dead_code)]
#[repr(C)]
struct JobBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[allow(dead_code)]
#[repr(C)]
struct JobExtendedLimitInformation {
    basic_limit_information: JobBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[allow(dead_code)]
#[repr(C)]
struct JobBasicUiRestrictions {
    ui_restrictions_class: u32,
}

#[allow(dead_code)]
#[repr(C)]
struct SecurityCapabilities {
    app_container_sid: *mut c_void,
    capabilities: *mut c_void,
    capability_count: u32,
    reserved: u32,
}

#[allow(dead_code)]
#[repr(C)]
struct SidAndAttributes {
    sid: *mut c_void,
    attributes: u32,
}

#[allow(dead_code)]
#[repr(C)]
struct TokenUserInformation {
    user: SidAndAttributes,
}

#[allow(dead_code)]
#[repr(C)]
struct TrusteeW {
    multiple_trustee: *mut c_void,
    multiple_trustee_operation: u32,
    trustee_form: u32,
    trustee_type: u32,
    name: *mut u16,
}

#[allow(dead_code)]
#[repr(C)]
struct ExplicitAccessW {
    access_permissions: u32,
    access_mode: u32,
    inheritance: u32,
    trustee: TrusteeW,
}

#[repr(C)]
struct FileAttributeTagInfo {
    file_attributes: u32,
    reparse_tag: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(object: Handle) -> Bool;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const SecurityAttributes,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;
    fn CreateProcessW(
        application_name: *const u16,
        command_line: *mut u16,
        process_attributes: *const SecurityAttributes,
        thread_attributes: *const SecurityAttributes,
        inherit_handles: Bool,
        creation_flags: u32,
        environment: *const c_void,
        current_directory: *const u16,
        startup_info: *const StartupInfoW,
        process_information: *mut ProcessInformation,
    ) -> Bool;
    fn ResumeThread(thread: Handle) -> u32;
    fn TerminateProcess(process: Handle, exit_code: u32) -> Bool;
    fn WaitForSingleObject(object: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetSystemWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn GetFinalPathNameByHandleW(file: Handle, path: *mut u16, path_size: u32, flags: u32) -> u32;
    fn GetFileInformationByHandleEx(
        file: Handle,
        information_class: u32,
        information: *mut c_void,
        size: u32,
    ) -> Bool;
    fn CreateJobObjectW(attributes: *const SecurityAttributes, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        class: u32,
        information: *const c_void,
        length: u32,
    ) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn InitializeProcThreadAttributeList(
        list: *mut c_void,
        attribute_count: u32,
        flags: u32,
        size: *mut usize,
    ) -> Bool;
    fn UpdateProcThreadAttribute(
        list: *mut c_void,
        flags: u32,
        attribute: usize,
        value: *mut c_void,
        size: usize,
        previous_value: *mut c_void,
        return_size: *mut usize,
    ) -> Bool;
    fn DeleteProcThreadAttributeList(list: *mut c_void);
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: Handle, desired_access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        information: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> Bool;
    fn InitializeSecurityDescriptor(descriptor: *mut c_void, revision: u32) -> Bool;
    fn SetSecurityDescriptorDacl(
        descriptor: *mut c_void,
        dacl_present: Bool,
        dacl: *mut c_void,
        dacl_defaulted: Bool,
    ) -> Bool;
    fn SetEntriesInAclW(
        count: u32,
        entries: *mut ExplicitAccessW,
        old_acl: *mut c_void,
        new_acl: *mut *mut c_void,
    ) -> u32;
    fn GetSecurityInfo(
        object: Handle,
        object_type: u32,
        security_information: u32,
        owner: *mut *mut c_void,
        group: *mut *mut c_void,
        dacl: *mut *mut c_void,
        sacl: *mut *mut c_void,
        security_descriptor: *mut *mut c_void,
    ) -> u32;
    fn SetSecurityInfo(
        object: Handle,
        object_type: u32,
        security_information: u32,
        owner: *mut c_void,
        group: *mut c_void,
        dacl: *mut c_void,
        sacl: *mut c_void,
    ) -> u32;
    fn FreeSid(sid: *mut c_void) -> *mut c_void;
}

#[link(name = "user32")]
extern "system" {
    fn CreateDesktopW(
        desktop: *const u16,
        device: *const u16,
        devmode: *mut c_void,
        flags: u32,
        desired_access: u32,
        attributes: *const SecurityAttributes,
    ) -> Handle;
    fn CloseDesktop(desktop: Handle) -> Bool;
}

#[link(name = "userenv")]
extern "system" {
    fn CreateAppContainerProfile(
        name: *const u16,
        display_name: *const u16,
        description: *const u16,
        capabilities: *mut c_void,
        capability_count: u32,
        sid: *mut *mut c_void,
    ) -> i32;
    fn DeriveAppContainerSidFromAppContainerName(name: *const u16, sid: *mut *mut c_void) -> i32;
}

/// Owns the job and the private desktop. Closing the job handle terminates
/// every process still inside it, which is what makes cancellation, timeout,
/// runtime shutdown, Driver exit, client drop and idle cleanup reach the
/// worker. Both handles are stored as `isize` so the guard stays `Send` across
/// the awaits that hold a warm worker.
pub(super) struct Guard {
    job: isize,
    desktop: isize,
}

impl Guard {
    pub(super) fn note_reaped(&mut self) {}

    /// The Job Object's memory ceiling is enforced by the kernel and surfaces
    /// as an exit status, so no supervisor state is folded in here.
    pub(super) fn memory_ceiling_exceeded(&self) -> bool {
        false
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            // Kill the tree first, then release the desktop it was running on.
            CloseHandle(self.job as Handle);
            CloseDesktop(self.desktop as Handle);
        }
    }
}

pub(super) struct Process {
    process: isize,
}

impl Process {
    pub(super) async fn wait(&mut self) -> std::io::Result<RawExit> {
        let process = self.process;
        let code = tokio::task::spawn_blocking(move || unsafe {
            WaitForSingleObject(process as Handle, INFINITE);
            let mut code = 0_u32;
            if GetExitCodeProcess(process as Handle, &mut code) == FALSE {
                return Err(std::io::Error::last_os_error());
            }
            Ok(code)
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(RawExit {
            success: code == 0,
            resource_limited: RESOURCE_LIMIT_EXITS.contains(&code),
            description: format!("exit code {code:#010x}"),
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.process as Handle);
        }
    }
}

pub(super) async fn spawn(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    boundary: &FilesystemBoundary,
    limits: &ContainmentLimits,
) -> Result<ContainedChild, VisualParseError> {
    if !executable.is_file() {
        return Err(spawn_error(std::io::Error::from_raw_os_error(2)));
    }
    let user = current_user_sid()?;
    let container = AppContainerSid::resolve()?;
    grant_worker_paths(working_directory, boundary, user.sid(), container.sid())?;

    let token = uuid::Uuid::new_v4().simple().to_string();
    let protocol_security =
        PrivateSecurity::new(&[(user.sid(), GENERIC_ALL), (container.sid(), GENERIC_ALL)])?;
    let job_security = PrivateSecurity::new(&[(user.sid(), GENERIC_ALL)])?;
    let desktop_security = PrivateSecurity::new(&[
        (user.sid(), GENERIC_ALL),
        (container.sid(), DESKTOP_WORKER_ACCESS),
    ])?;

    let mut inbound =
        ProtocolPipe::create(&token, "out", Direction::FromWorker, &protocol_security)?;
    let mut outbound = ProtocolPipe::create(&token, "in", Direction::ToWorker, &protocol_security)?;
    // Both ends already exist, so the connections complete before any worker
    // code is created, let alone resumed.
    outbound.connect().await?;
    inbound.connect().await?;
    let null_device = OwnedHandle::open_null()?;
    let job = OwnedHandle::create_job(limits, &job_security)?;
    let mut desktop = PrivateDesktop::create(&token, &desktop_security)?;

    let information = create_suspended(
        executable,
        args,
        working_directory,
        &desktop,
        [outbound.client(), inbound.client(), null_device.get()],
        container.sid(),
        &protocol_security,
    )?;

    // The suspended process has not executed one instruction, so any failure
    // from here on can still be resolved by terminating it.
    if unsafe { AssignProcessToJobObject(job.get(), information.process) } == FALSE {
        let failure =
            last_error("failed to assign the perception worker to its containment job object");
        information.terminate();
        return Err(failure);
    }
    // The child now owns its copies; releasing the parent's copies is what
    // makes the worker observe end of file when the Driver goes away.
    drop(null_device);
    let inbound = inbound.into_server();
    let outbound = outbound.into_server();

    if unsafe { ResumeThread(information.thread) } == u32::MAX {
        let failure = last_error("failed to resume the contained perception worker");
        information.terminate();
        return Err(failure);
    }
    let guard = Guard {
        job: job.release() as isize,
        desktop: desktop.release() as isize,
    };
    let process = Process {
        process: information.into_process() as isize,
    };
    Ok(ContainedChild {
        guard,
        process,
        stdin: Some(outbound),
        stdout: Some(inbound),
    })
}

/// A handle that is closed when it is dropped unless ownership is released to a
/// longer-lived guard.
struct OwnedHandle(Handle);

// Win32 kernel handles are process-wide values. Ownership stays unique in this
// wrapper, and moving it between executor threads does not change validity.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn get(&self) -> Handle {
        self.0
    }

    fn release(mut self) -> Handle {
        let handle = self.0;
        self.0 = std::ptr::null_mut();
        std::mem::forget(self);
        handle
    }

    /// Open the null device as the worker's stderr. Diagnostics travel over the
    /// framed protocol, so the worker never writes to an inherited console.
    fn open_null() -> Result<Self, VisualParseError> {
        let name = wide("NUL");
        let attributes = inheritable_attributes();
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                0,
                &attributes,
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle as isize == -1 {
            return Err(last_error(
                "failed to open the null device for the perception worker",
            ));
        }
        Ok(Self(handle))
    }

    fn create_job(
        limits: &ContainmentLimits,
        security: &PrivateSecurity,
    ) -> Result<Self, VisualParseError> {
        // An unnamed job object with a private, protected security descriptor
        // cannot be opened by name or by an unrelated process.
        let attributes = security.attributes();
        let job = unsafe { CreateJobObjectW(&attributes, std::ptr::null()) };
        if job.is_null() {
            return Err(last_error(
                "failed to create the perception worker containment job object",
            ));
        }
        let job = Self(job);

        let mut information: JobExtendedLimitInformation = unsafe { std::mem::zeroed() };
        information.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_PROCESS_TIME;
        information.basic_limit_information.active_process_limit = limits.max_processes;
        // `PerProcessUserTimeLimit` counts in 100-nanosecond units.
        information
            .basic_limit_information
            .per_process_user_time_limit =
            i64::try_from(limits.max_cpu_seconds().saturating_mul(10_000_000)).unwrap_or(i64::MAX);
        information.job_memory_limit =
            usize::try_from(limits.max_memory_bytes).unwrap_or(usize::MAX);
        if unsafe {
            SetInformationJobObject(
                job.get(),
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                std::ptr::addr_of!(information).cast(),
                std::mem::size_of::<JobExtendedLimitInformation>() as u32,
            )
        } == FALSE
        {
            return Err(last_error(
                "failed to apply the perception worker containment job object limits",
            ));
        }

        let restrictions = JobBasicUiRestrictions {
            ui_restrictions_class: JOB_OBJECT_UILIMIT_ALL,
        };
        if unsafe {
            SetInformationJobObject(
                job.get(),
                JOB_OBJECT_BASIC_UI_RESTRICTIONS,
                std::ptr::addr_of!(restrictions).cast(),
                std::mem::size_of::<JobBasicUiRestrictions>() as u32,
            )
        } == FALSE
        {
            return Err(last_error(
                "failed to apply the perception worker containment job object UI restrictions",
            ));
        }
        Ok(job)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// The worker's own desktop. Nothing else runs on it, so a capture or input
/// attempt from inside the worker reaches an empty desktop rather than the
/// user's session.
struct PrivateDesktop {
    handle: Handle,
    name: Vec<u16>,
    released: bool,
}

impl PrivateDesktop {
    fn create(token: &str, security: &PrivateSecurity) -> Result<Self, VisualParseError> {
        let name = wide(&format!("cua-perception-{token}"));
        let attributes = security.attributes();
        let handle = unsafe {
            CreateDesktopW(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
                GENERIC_ALL,
                &attributes,
            )
        };
        if handle.is_null() {
            return Err(last_error(
                "failed to create the private desktop for the perception worker",
            ));
        }
        Ok(Self {
            handle,
            name,
            released: false,
        })
    }

    fn name(&self) -> *mut u16 {
        self.name.as_ptr().cast_mut()
    }

    /// Hand the desktop handle to the guard that outlives this value. The
    /// desktop exists only while a handle to it is open, so the guard must hold
    /// it for as long as the worker runs.
    fn release(&mut self) -> Handle {
        self.released = true;
        self.handle
    }
}

impl Drop for PrivateDesktop {
    fn drop(&mut self) {
        if !self.released && !self.handle.is_null() {
            unsafe {
                CloseDesktop(self.handle);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Direction {
    ToWorker,
    FromWorker,
}

/// One half of the framed protocol: an asynchronous private named pipe on the
/// Driver side and a plain inheritable stdio handle on the worker side.
struct ProtocolPipe {
    server: NamedPipeServer,
    client: OwnedHandle,
}

impl ProtocolPipe {
    fn create(
        token: &str,
        suffix: &str,
        direction: Direction,
        security: &PrivateSecurity,
    ) -> Result<Self, VisualParseError> {
        let name = format!(r"\\.\pipe\cua-perception-{token}-{suffix}");
        let mut options = ServerOptions::new();
        options
            .pipe_mode(PipeMode::Byte)
            // One instance, claimed first, so no other process can pre-create
            // or later attach to this name.
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true);
        match direction {
            Direction::ToWorker => options.access_inbound(false).access_outbound(true),
            Direction::FromWorker => options.access_inbound(true).access_outbound(false),
        };
        let mut attributes = security.attributes();
        let server = unsafe {
            options.create_with_security_attributes_raw(
                &name,
                std::ptr::addr_of_mut!(attributes).cast(),
            )
        }
        .map_err(|cause| {
            containment_error(
                "failed to create a private perception worker protocol pipe",
                os_detail(&cause),
            )
        })?;

        let wide_name = wide(&name);
        let client_attributes = inheritable_attributes();
        let access = match direction {
            Direction::ToWorker => GENERIC_READ,
            Direction::FromWorker => GENERIC_WRITE,
        };
        let client = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                access,
                0,
                &client_attributes,
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if client as isize == -1 {
            return Err(last_error(
                "failed to open the worker end of a perception protocol pipe",
            ));
        }
        Ok(Self {
            server,
            client: OwnedHandle(client),
        })
    }

    fn client(&self) -> Handle {
        self.client.get()
    }

    async fn connect(&mut self) -> Result<(), VisualParseError> {
        self.server.connect().await.map_err(|cause| {
            containment_error(
                "failed to connect a private perception worker protocol pipe",
                os_detail(&cause),
            )
        })
    }

    /// Release the worker's end, which the child now owns, and keep the
    /// Driver's asynchronous end.
    fn into_server(self) -> NamedPipeServer {
        self.server
    }
}

/// The suspended worker process and its primary thread.
struct SuspendedProcess {
    process: Handle,
    thread: Handle,
}

impl SuspendedProcess {
    fn terminate(self) {
        unsafe {
            TerminateProcess(self.process, 1);
            CloseHandle(self.thread);
            CloseHandle(self.process);
        }
        std::mem::forget(self);
    }

    fn into_process(self) -> Handle {
        let process = self.process;
        unsafe {
            CloseHandle(self.thread);
        }
        std::mem::forget(self);
        process
    }
}

impl Drop for SuspendedProcess {
    fn drop(&mut self) {
        unsafe {
            TerminateProcess(self.process, 1);
            CloseHandle(self.thread);
            CloseHandle(self.process);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_suspended(
    executable: &Path,
    args: &[String],
    working_directory: &Path,
    desktop: &PrivateDesktop,
    inherited: [Handle; 3],
    container: *mut c_void,
    security: &PrivateSecurity,
) -> Result<SuspendedProcess, VisualParseError> {
    let application = wide(executable.as_os_str());
    let mut command_line = command_line(executable, args);
    let current_directory = wide(working_directory.as_os_str());
    let environment = environment_block(working_directory)?;

    let mut attributes = AttributeList::new(2)?;
    let mut handles = inherited;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        handles.as_mut_ptr().cast(),
        std::mem::size_of_val(&handles),
        "failed to restrict the perception worker's inherited handles",
    )?;
    let mut capabilities = SecurityCapabilities {
        app_container_sid: container,
        capabilities: std::ptr::null_mut(),
        capability_count: 0,
        reserved: 0,
    };
    attributes.set(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        std::ptr::addr_of_mut!(capabilities).cast(),
        std::mem::size_of::<SecurityCapabilities>(),
        "failed to place the perception worker in its AppContainer",
    )?;

    let mut startup: StartupInfoExW = unsafe { std::mem::zeroed() };
    startup.startup_info.cb = std::mem::size_of::<StartupInfoExW>() as u32;
    startup.startup_info.desktop = desktop.name();
    startup.startup_info.flags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    startup.startup_info.show_window = SW_HIDE;
    startup.startup_info.std_input = inherited[0];
    startup.startup_info.std_output = inherited[1];
    startup.startup_info.std_error = inherited[2];
    startup.attribute_list = attributes.pointer();

    let process_attributes = security.attributes();
    let mut information: ProcessInformation = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            &process_attributes,
            &process_attributes,
            // Required by the handle list, which is exactly what narrows this
            // from "every inheritable handle" to the three above.
            TRUE,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            current_directory.as_ptr(),
            std::ptr::addr_of!(startup.startup_info),
            &mut information,
        )
    };
    if created == FALSE {
        return Err(last_error(
            "failed to launch the contained perception worker",
        ));
    }
    Ok(SuspendedProcess {
        process: information.process,
        thread: information.thread,
    })
}

/// A `PROC_THREAD_ATTRIBUTE_LIST` in a pointer-aligned allocation.
struct AttributeList {
    storage: Vec<usize>,
}

impl AttributeList {
    fn new(count: u32) -> Result<Self, VisualParseError> {
        let mut size: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut size);
        }
        if size == 0 {
            return Err(last_error(
                "failed to size the perception worker process attribute list",
            ));
        }
        // `Vec<usize>` guarantees the pointer alignment the attribute list
        // requires; a byte vector does not. The list is only wrapped in the
        // guard once it initializes, so `Drop` never deletes an unformed list.
        let mut storage = vec![0_usize; size.div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), count, 0, &mut size)
        } == FALSE
        {
            return Err(last_error(
                "failed to create the perception worker process attribute list",
            ));
        }
        Ok(Self { storage })
    }

    fn pointer(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }

    fn set(
        &mut self,
        attribute: usize,
        value: *mut c_void,
        size: usize,
        message: &'static str,
    ) -> Result<(), VisualParseError> {
        let list = self.pointer();
        if unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                attribute,
                value,
                size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == FALSE
        {
            return Err(last_error(message));
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.storage.as_mut_ptr().cast());
        }
    }
}

/// The worker's AppContainer identity.
struct AppContainerSid(*mut c_void);

// The SID is an owned allocation released by `FreeSid`; no thread-local state
// is associated with the pointer.
unsafe impl Send for AppContainerSid {}

impl AppContainerSid {
    fn resolve() -> Result<Self, VisualParseError> {
        let name = wide(APP_CONTAINER_NAME);
        let display = wide(APP_CONTAINER_DISPLAY_NAME);
        let description = wide(APP_CONTAINER_DESCRIPTION);
        let mut sid: *mut c_void = std::ptr::null_mut();
        let created = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                display.as_ptr(),
                description.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut sid,
            )
        };
        if created == HRESULT_ALREADY_EXISTS {
            let derived =
                unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            if derived < 0 {
                return Err(containment_error(
                    "failed to resolve the perception worker AppContainer identity",
                    Some(format!("hresult {derived:#010x}")),
                ));
            }
        } else if created < 0 {
            return Err(containment_error(
                "failed to create the perception worker AppContainer identity",
                Some(format!("hresult {created:#010x}")),
            ));
        }
        if sid.is_null() {
            return Err(containment_error(
                "the perception worker AppContainer identity was empty",
                None,
            ));
        }
        Ok(Self(sid))
    }

    fn sid(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for AppContainerSid {
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.0);
        }
    }
}

/// The Driver's own user identity, read from its process token.
struct UserSid {
    buffer: Vec<u8>,
}

impl UserSid {
    fn sid(&self) -> *mut c_void {
        // `TOKEN_USER` starts with the `SID_AND_ATTRIBUTES` whose first field
        // points into this same buffer.
        let information = self.buffer.as_ptr().cast::<TokenUserInformation>();
        unsafe { (*information).user.sid }
    }
}

fn current_user_sid() -> Result<UserSid, VisualParseError> {
    let mut token: Handle = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == FALSE {
        return Err(last_error(
            "failed to read the Driver identity for the perception worker sandbox",
        ));
    }
    let token = OwnedHandle(token);
    let mut required = 0_u32;
    unsafe {
        GetTokenInformation(
            token.get(),
            TOKEN_USER_CLASS,
            std::ptr::null_mut(),
            0,
            &mut required,
        );
    }
    if required == 0
        || std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER)
    {
        return Err(last_error(
            "failed to size the Driver identity for the perception worker sandbox",
        ));
    }
    let mut buffer = vec![0_u8; required as usize];
    if unsafe {
        GetTokenInformation(
            token.get(),
            TOKEN_USER_CLASS,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == FALSE
    {
        return Err(last_error(
            "failed to read the Driver identity for the perception worker sandbox",
        ));
    }
    Ok(UserSid { buffer })
}

/// A private, protected discretionary access control list plus the security
/// descriptor that carries it.
struct PrivateSecurity {
    descriptor: Box<SecurityDescriptorStorage>,
    _acl: LocalAcl,
}

impl PrivateSecurity {
    fn new(grants: &[(*mut c_void, u32)]) -> Result<Self, VisualParseError> {
        let acl = build_acl(grants, NO_INHERITANCE, std::ptr::null_mut())?;
        let mut descriptor = Box::new(SecurityDescriptorStorage([0_u8; 64]));
        let pointer: *mut c_void = std::ptr::addr_of_mut!(descriptor.0).cast();
        if unsafe { InitializeSecurityDescriptor(pointer, SECURITY_DESCRIPTOR_REVISION) } == FALSE {
            return Err(last_error(
                "failed to create the private security descriptor for the perception worker",
            ));
        }
        if unsafe { SetSecurityDescriptorDacl(pointer, TRUE, acl.0, FALSE) } == FALSE {
            return Err(last_error(
                "failed to apply the private access control list for the perception worker",
            ));
        }
        Ok(Self {
            descriptor,
            _acl: acl,
        })
    }

    fn attributes(&self) -> SecurityAttributes {
        SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            security_descriptor: std::ptr::addr_of!(self.descriptor.0).cast_mut().cast(),
            inherit_handle: FALSE,
        }
    }
}

/// An access control list allocated by `SetEntriesInAclW`.
struct LocalAcl(*mut c_void);

// The ACL is an owned `LocalAlloc` allocation and is only read while installed
// in a descriptor or passed to the security APIs.
unsafe impl Send for LocalAcl {}

impl Drop for LocalAcl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

fn build_acl(
    grants: &[(*mut c_void, u32)],
    inheritance: u32,
    existing: *mut c_void,
) -> Result<LocalAcl, VisualParseError> {
    let mut entries: Vec<ExplicitAccessW> = grants
        .iter()
        .map(|(sid, access)| ExplicitAccessW {
            access_permissions: *access,
            access_mode: GRANT_ACCESS,
            inheritance,
            trustee: TrusteeW {
                multiple_trustee: std::ptr::null_mut(),
                multiple_trustee_operation: NO_MULTIPLE_TRUSTEE,
                trustee_form: TRUSTEE_IS_SID,
                trustee_type: TRUSTEE_IS_UNKNOWN,
                name: (*sid).cast(),
            },
        })
        .collect();
    let mut acl: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        SetEntriesInAclW(
            entries.len() as u32,
            entries.as_mut_ptr(),
            existing,
            &mut acl,
        )
    };
    if status != ERROR_SUCCESS || acl.is_null() {
        return Err(containment_error(
            "failed to build the access control list for the perception worker",
            Some(format!("os error {status}")),
        ));
    }
    Ok(LocalAcl(acl))
}

/// Give the worker's AppContainer identity exactly the access its bundle,
/// runtime, model and working-directory paths require, and nothing else.
///
/// The working directory receives a protected list naming only the Driver's
/// user and the worker, so no other account on the machine can read the
/// screenshot the worker is parsing. The remaining roots are merged rather than
/// replaced, because they are shared installation paths whose existing
/// permissions belong to the installer.
fn grant_worker_paths(
    working_directory: &Path,
    boundary: &FilesystemBoundary,
    user: *mut c_void,
    container: *mut c_void,
) -> Result<(), VisualParseError> {
    let working = std::fs::canonicalize(working_directory).map_err(|cause| {
        containment_error(
            "failed to resolve the private perception worker directory",
            os_detail(&cause),
        )
    })?;
    set_protected_dacl(&working, &[(user, GENERIC_ALL), (container, GENERIC_ALL)])?;
    for root in &boundary.writable {
        if *root == working {
            continue;
        }
        merge_grant(root, container, GENERIC_ALL)?;
    }
    for root in &boundary.readable {
        if *root == working {
            continue;
        }
        merge_grant(root, container, GENERIC_READ | GENERIC_EXECUTE)?;
    }
    Ok(())
}

fn set_protected_dacl(path: &Path, grants: &[(*mut c_void, u32)]) -> Result<(), VisualParseError> {
    let target = AclTarget::open(path)?;
    let acl = build_acl(
        grants,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        std::ptr::null_mut(),
    )?;
    apply_dacl(
        target.handle.get(),
        acl.0,
        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
        "failed to apply the private access control list to the perception worker directory",
    )
}

fn merge_grant(path: &Path, sid: *mut c_void, access: u32) -> Result<(), VisualParseError> {
    let target = AclTarget::open(path)?;
    let mut existing: *mut c_void = std::ptr::null_mut();
    let mut descriptor: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            target.handle.get(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut existing,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(containment_error(
            "failed to read the existing permissions of a perception worker path",
            Some(format!("os error {status}")),
        ));
    }
    let descriptor = LocalAcl(descriptor);
    let acl = build_acl(
        &[(sid, access)],
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        existing,
    )?;
    let result = apply_dacl(
        target.handle.get(),
        acl.0,
        DACL_SECURITY_INFORMATION,
        "failed to grant the perception worker access to its own bundle",
    );
    drop(descriptor);
    result
}

fn apply_dacl(
    target: Handle,
    acl: *mut c_void,
    information: u32,
    message: &'static str,
) -> Result<(), VisualParseError> {
    let status = unsafe {
        SetSecurityInfo(
            target,
            SE_FILE_OBJECT,
            information,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(containment_error(
            message,
            Some(format!("os error {status}")),
        ));
    }
    Ok(())
}

/// An ACL target opened without following a final reparse point and held
/// without delete sharing until the descriptor update finishes. The final
/// kernel-resolved name is compared with the already canonical policy path
/// before any grant is applied. A concurrent ancestor swap that redirects the
/// open resolves to a different name and is refused; after the open, withholding
/// delete sharing leaves this exact directory pinned through the ACL update.
struct AclTarget {
    handle: OwnedHandle,
}

impl AclTarget {
    fn open(path: &Path) -> Result<Self, VisualParseError> {
        let name = wide(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                READ_CONTROL | WRITE_DAC,
                // Allow ordinary readers and writers to keep using shared
                // installation roots. Deliberately withhold delete sharing so
                // this object cannot be renamed out from under the ACL update.
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if handle as isize == -1 {
            return Err(last_error(
                "failed to open a perception worker path for an ACL update",
            ));
        }
        let handle = OwnedHandle(handle);

        let mut tag = FileAttributeTagInfo {
            file_attributes: 0,
            reparse_tag: 0,
        };
        if unsafe {
            GetFileInformationByHandleEx(
                handle.get(),
                FILE_ATTRIBUTE_TAG_INFO_CLASS,
                std::ptr::addr_of_mut!(tag).cast(),
                std::mem::size_of::<FileAttributeTagInfo>() as u32,
            )
        } == FALSE
        {
            return Err(last_error(
                "failed to validate a perception worker ACL target",
            ));
        }
        if tag.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(containment_error(
                "a perception worker ACL target is a reparse point",
                None,
            ));
        }

        let resolved = final_path(&handle)?;
        if normalize_windows_path(&resolved) != normalize_windows_path(path) {
            return Err(containment_error(
                "a perception worker ACL target changed while it was opened",
                None,
            ));
        }
        Ok(Self { handle })
    }
}

fn final_path(handle: &OwnedHandle) -> Result<std::path::PathBuf, VisualParseError> {
    let required = unsafe {
        GetFinalPathNameByHandleW(handle.get(), std::ptr::null_mut(), 0, FILE_NAME_NORMALIZED)
    };
    if required == 0 {
        return Err(last_error(
            "failed to size a perception worker ACL target path",
        ));
    }
    let mut path = vec![0_u16; required as usize];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            handle.get(),
            path.as_mut_ptr(),
            path.len() as u32,
            FILE_NAME_NORMALIZED,
        )
    };
    if written == 0 || written as usize >= path.len() {
        return Err(last_error(
            "failed to read a perception worker ACL target path",
        ));
    }
    path.truncate(written as usize);
    Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(
        &path,
    )))
}

fn normalize_windows_path(path: &Path) -> String {
    let mut text = path.as_os_str().to_string_lossy().replace('/', "\\");
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        text = format!(r"\\{stripped}");
    } else if let Some(stripped) = text.strip_prefix(r"\\?\") {
        text = stripped.to_owned();
    }
    text.trim_end_matches('\\').to_ascii_lowercase()
}

/// A minimal environment block. The worker inherits nothing from the Driver, so
/// no proxy setting, token, or caller path reaches it; only the loader's
/// `SystemRoot` and private temporary and profile directories inside the
/// working directory are provided.
fn environment_block(working_directory: &Path) -> Result<Vec<u16>, VisualParseError> {
    let profile = working_directory.join("profile");
    let roaming_app_data = profile.join("AppData").join("Roaming");
    let local_app_data = profile.join("AppData").join("Local");
    std::fs::create_dir_all(&profile).map_err(|cause| {
        containment_error(
            "failed to create the perception worker's private profile directory",
            os_detail(&cause),
        )
    })?;
    std::fs::create_dir_all(&roaming_app_data).map_err(|cause| {
        containment_error(
            "failed to create the perception worker's private roaming application data directory",
            os_detail(&cause),
        )
    })?;
    std::fs::create_dir_all(&local_app_data).map_err(|cause| {
        containment_error(
            "failed to create the perception worker's private local application data directory",
            os_detail(&cause),
        )
    })?;

    let mut system_root = vec![0_u16; 260];
    let written =
        unsafe { GetSystemWindowsDirectoryW(system_root.as_mut_ptr(), system_root.len() as u32) };
    if written == 0 || written as usize >= system_root.len() {
        return Err(last_error(
            "failed to read the system directory for the perception worker environment",
        ));
    }
    system_root.truncate(written as usize);
    let system_root = String::from_utf16_lossy(&system_root);
    let temporary = working_directory.display().to_string();
    let profile = profile.display().to_string();
    let roaming_app_data = roaming_app_data.display().to_string();
    let local_app_data = local_app_data.display().to_string();

    let mut block = Vec::new();
    let mut entries = [
        format!("APPDATA={roaming_app_data}"),
        format!("LOCALAPPDATA={local_app_data}"),
        format!("SystemRoot={system_root}"),
        format!("PATH={system_root}\\System32"),
        format!("TEMP={temporary}"),
        format!("TMP={temporary}"),
        format!("USERPROFILE={profile}"),
        format!("windir={system_root}"),
    ];
    entries.sort_by_key(|entry| entry.to_ascii_lowercase());
    for entry in entries {
        block.extend(entry.encode_utf16());
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn command_line(executable: &Path, args: &[String]) -> Vec<u16> {
    let mut command_line = Vec::new();
    for argument in
        std::iter::once(executable.as_os_str()).chain(args.iter().map(std::ffi::OsStr::new))
    {
        if !command_line.is_empty() {
            command_line.push(u16::from(b' '));
        }
        append_argument(&mut command_line, argument.encode_wide());
    }
    command_line.push(0);
    command_line
}

/// Quote one argument the way `CommandLineToArgvW` parses it, so an argument
/// containing a space, tab or quote cannot be split or escape into another.
fn append_argument(command_line: &mut Vec<u16>, argument: impl IntoIterator<Item = u16>) {
    let argument = argument.into_iter().collect::<Vec<_>>();
    let quote = argument.is_empty()
        || argument
            .iter()
            .any(|value| matches!(*value, 0x20 | 0x09 | 0x22));
    if !quote {
        command_line.extend(argument);
        return;
    }
    command_line.push(u16::from(b'"'));
    let mut backslashes = 0;
    for value in argument {
        if value == u16::from(b'\\') {
            backslashes += 1;
        } else if value == u16::from(b'"') {
            command_line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            command_line.push(value);
            backslashes = 0;
        } else {
            command_line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
            command_line.push(value);
            backslashes = 0;
        }
    }
    command_line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    command_line.push(u16::from(b'"'));
}

fn inheritable_attributes() -> SecurityAttributes {
    SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        security_descriptor: std::ptr::null_mut(),
        inherit_handle: TRUE,
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_error(message: &'static str) -> VisualParseError {
    containment_error(message, os_detail(&std::io::Error::last_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_job_ui_restriction_is_applied() {
        for restriction in [
            JOB_OBJECT_UILIMIT_HANDLES,
            JOB_OBJECT_UILIMIT_READCLIPBOARD,
            JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
            JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
            JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
            JOB_OBJECT_UILIMIT_GLOBALATOMS,
            JOB_OBJECT_UILIMIT_DESKTOP,
            JOB_OBJECT_UILIMIT_EXITWINDOWS,
        ] {
            assert_eq!(JOB_OBJECT_UILIMIT_ALL & restriction, restriction);
        }
    }

    #[test]
    fn the_desktop_grant_withholds_hooks_and_desktop_switching() {
        const DESKTOP_HOOKCONTROL: u32 = 0x0008;
        const DESKTOP_JOURNALRECORD: u32 = 0x0010;
        const DESKTOP_JOURNALPLAYBACK: u32 = 0x0020;
        const DESKTOP_SWITCHDESKTOP: u32 = 0x0100;
        for withheld in [
            DESKTOP_HOOKCONTROL,
            DESKTOP_JOURNALRECORD,
            DESKTOP_JOURNALPLAYBACK,
            DESKTOP_SWITCHDESKTOP,
        ] {
            assert_eq!(DESKTOP_WORKER_ACCESS & withheld, 0);
        }
    }

    #[test]
    fn the_command_line_quotes_arguments_that_could_otherwise_split() {
        let rendered = command_line(
            Path::new(r"C:\Program Files\cua\worker.exe"),
            &["--manifest".to_owned(), r"C:\a b\m.json".to_owned()],
        );
        let rendered = String::from_utf16(&rendered[..rendered.len() - 1]).unwrap();
        assert_eq!(
            rendered,
            "\"C:\\Program Files\\cua\\worker.exe\" --manifest \"C:\\a b\\m.json\""
        );
    }

    #[test]
    fn the_environment_block_carries_only_loader_and_private_worker_paths() {
        let worker_root = tempfile::tempdir().unwrap();
        let block = environment_block(worker_root.path()).unwrap();
        let rendered = String::from_utf16(&block).unwrap();
        let entries = rendered
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.split_once('=').unwrap())
            .collect::<std::collections::HashMap<_, _>>();

        let expected_profile = worker_root.path().join("profile");
        let expected_roaming = expected_profile.join("AppData").join("Roaming");
        let expected_local = expected_profile.join("AppData").join("Local");
        assert_eq!(entries["TEMP"], worker_root.path().display().to_string());
        assert_eq!(entries["TMP"], worker_root.path().display().to_string());
        assert_eq!(
            entries["USERPROFILE"],
            expected_profile.display().to_string()
        );
        assert_eq!(entries["APPDATA"], expected_roaming.display().to_string());
        assert_eq!(
            entries["LOCALAPPDATA"],
            expected_local.display().to_string()
        );
        assert!(entries.contains_key("SystemRoot"));
        assert!(entries.contains_key("windir"));
        assert!(entries.contains_key("PATH"));
        for unrelated in ["HTTP_PROXY", "PATHEXT", "USERNAME", "HOMEDRIVE"] {
            assert!(
                !entries.contains_key(unrelated),
                "{unrelated} reached the worker"
            );
        }
        for path in [&expected_profile, &expected_roaming, &expected_local] {
            assert!(path.is_dir(), "{} was not created", path.display());
            assert!(path.starts_with(worker_root.path()));
        }
        for name in ["USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
            assert!(Path::new(entries[name]).starts_with(worker_root.path()));
            if let Some(host) = std::env::var_os(name) {
                assert_ne!(
                    normalize_windows_path(Path::new(entries[name])),
                    normalize_windows_path(Path::new(&host)),
                    "{name} reused the host profile path"
                );
            }
        }
        assert!(rendered.ends_with("\0\0"));
    }

    #[test]
    fn final_handle_paths_compare_without_win32_namespace_prefixes() {
        assert_eq!(
            normalize_windows_path(Path::new(r"\\?\C:\Cua\Worker")),
            normalize_windows_path(Path::new(r"c:\cua\worker\"))
        );
        assert_eq!(
            normalize_windows_path(Path::new(r"\\?\UNC\server\share\worker")),
            normalize_windows_path(Path::new(r"\\server\share\worker"))
        );
    }
}
