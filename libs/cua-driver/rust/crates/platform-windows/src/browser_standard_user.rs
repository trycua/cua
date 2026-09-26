//! Win32 side of the standard-user launch for isolated browsers.
//!
//! An elevated Driver derives a standard-user token from its own process token
//! with the SAFER "Normal User" level (the level `runas /trustlevel:0x20000`
//! uses): `BUILTIN\Administrators` and the other administrative groups become
//! deny-only and administrative privileges are removed. The Driver then lowers
//! the token to Medium integrity, removes any remaining privilege a standard
//! user does not hold, and verifies the result before use. The browser is
//! created with that token, the installation protection probes run while
//! impersonating it, and file work inside the browser-writable profile runs
//! while impersonating it. The Driver never falls back to its own elevated
//! token. Decision rules live in `browser_launch_token`.
//!
//! A restricted token from `CreateRestrictedToken` (with or without
//! `LUA_TOKEN`/`DISABLE_MAX_PRIVILEGE`) is deliberately not used: Chrome's
//! sandbox cannot launch its GPU and renderer processes under one.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::{Arc, Mutex};

use cua_driver_core::browser::refusal::{BrowserRefusal, BrowserRefusalCode};
use cua_driver_core::browser::IsolatedBrowserProcess;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, E_ACCESSDENIED, HANDLE, LUID, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::Security::AppLocker::{
    SaferCloseLevel, SaferComputeTokenFromLevel, SaferCreateLevel,
    SAFER_COMPUTE_TOKEN_FROM_LEVEL_FLAGS, SAFER_LEVELID_NORMALUSER, SAFER_LEVEL_OPEN,
    SAFER_SCOPEID_USER,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, CreateWellKnownSid, EqualSid, GetLengthSid, GetSidSubAuthority,
    GetSidSubAuthorityCount, GetTokenInformation, ImpersonateLoggedOnUser, LookupPrivilegeNameW,
    RevertToSelf, SecurityImpersonation, SetTokenInformation, TokenElevation, TokenGroups,
    TokenImpersonationLevel, TokenIntegrityLevel, TokenPrivileges, WinBuiltinAdministratorsSid,
    WinMediumLabelSid, LUID_AND_ATTRIBUTES, PSID, SAFER_LEVEL_HANDLE, SECURITY_IMPERSONATION_LEVEL,
    SE_PRIVILEGE_REMOVED, SID_AND_ATTRIBUTES, TOKEN_ACCESS_MASK, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_DELETE_CHILD,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetCurrentThread, GetExitCodeProcess,
    OpenProcessToken, OpenThreadToken, TerminateProcess, WaitForSingleObject, INFINITE,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::browser_launch_token::{
    browser_launch_token, privileges_beyond_standard_user, standard_user_token_unavailable_message,
    validate_standard_user_token, windows_command_line, AdministratorsMembership,
    BrowserLaunchToken, TokenFacts, MEDIUM_INTEGRITY_RID,
};

// `SE_GROUP_*` attribute bits from winnt.h.
const SE_GROUP_ENABLED: u32 = 0x0000_0004;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x0000_0010;
const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;

/// An owned kernel handle closed on drop.
pub(crate) struct OwnedHandle(HANDLE);

// SAFETY: a kernel handle value is process-wide and may be used and closed
// from any thread. The wrapper never hands out the handle for closing.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// The token that will run the isolated browser.
#[derive(Clone)]
pub(crate) enum BrowserLaunchContext {
    /// The Driver is not elevated; the browser runs with the Driver's token.
    Driver,
    /// A verified standard-user token derived from the elevated Driver token.
    StandardUser(Arc<OwnedHandle>),
}

impl BrowserLaunchContext {
    pub(crate) fn kind(&self) -> BrowserLaunchToken {
        match self {
            Self::Driver => BrowserLaunchToken::Driver,
            Self::StandardUser(_) => BrowserLaunchToken::StandardUser,
        }
    }
}

/// A process token cannot change after process start, so one verified
/// standard-user token serves every isolated launch. Failures are not cached.
static STANDARD_USER_TOKEN: Mutex<Option<Arc<OwnedHandle>>> = Mutex::new(None);

fn refusal(message: impl Into<String>) -> BrowserRefusal {
    BrowserRefusal::new(BrowserRefusalCode::BrowserRouteUnavailable, message)
}

/// Decide which token runs the isolated browser and, for an elevated Driver,
/// produce the verified standard-user token. Never returns the Driver's
/// elevated token for the browser.
pub(crate) fn browser_launch_context() -> Result<BrowserLaunchContext, BrowserRefusal> {
    let driver_token = current_process_token(TOKEN_QUERY)
        .map_err(|error| refusal(format!("could not open the Driver process token: {error}")))?;
    let driver = token_facts(driver_token.raw()).map_err(|error| {
        refusal(format!(
            "could not inspect the Driver process token: {error}"
        ))
    })?;
    if browser_launch_token(&driver) == BrowserLaunchToken::Driver {
        return Ok(BrowserLaunchContext::Driver);
    }
    let mut cached = STANDARD_USER_TOKEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(token) = cached.as_ref() {
        return Ok(BrowserLaunchContext::StandardUser(token.clone()));
    }
    let token = derive_verified_standard_user_token(&driver)
        .map(Arc::new)
        .map_err(|reason| refusal(standard_user_token_unavailable_message(&reason)))?;
    *cached = Some(token.clone());
    Ok(BrowserLaunchContext::StandardUser(token))
}

fn current_process_token(access: TOKEN_ACCESS_MASK) -> Result<OwnedHandle, String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) }
        .map_err(|error| format!("OpenProcessToken failed: {error}"))?;
    Ok(OwnedHandle(token))
}

/// Derive, harden, and verify the standard-user token for `driver`.
fn derive_verified_standard_user_token(driver: &TokenFacts) -> Result<OwnedHandle, String> {
    let driver_token = current_process_token(TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY)?;
    let derived = safer_normal_user_token(driver_token.raw())?;
    let facts = token_facts(derived.raw())?;
    if facts.integrity_rid > MEDIUM_INTEGRITY_RID {
        set_medium_integrity(derived.raw())?;
    }
    remove_privileges(derived.raw(), &facts)?;
    let verified = token_facts(derived.raw())?;
    validate_standard_user_token(driver, &verified).map_err(|reason| {
        format!(
            "{reason}; derived token: integrity=0x{:04x}, administrators={:?}, privileges=[{}]",
            verified.integrity_rid,
            verified.administrators,
            verified.privileges.join(", ")
        )
    })?;
    Ok(derived)
}

fn safer_normal_user_token(driver_token: HANDLE) -> Result<OwnedHandle, String> {
    let mut level = SAFER_LEVEL_HANDLE::default();
    unsafe {
        SaferCreateLevel(
            SAFER_SCOPEID_USER,
            SAFER_LEVELID_NORMALUSER,
            SAFER_LEVEL_OPEN,
            &mut level,
            None,
        )
    }
    .map_err(|error| format!("SaferCreateLevel failed: {error}"))?;
    let mut derived = HANDLE::default();
    let computed = unsafe {
        SaferComputeTokenFromLevel(
            level,
            driver_token,
            &mut derived,
            SAFER_COMPUTE_TOKEN_FROM_LEVEL_FLAGS(0),
            None,
        )
    };
    let _ = unsafe { SaferCloseLevel(level) };
    computed.map_err(|error| format!("SaferComputeTokenFromLevel failed: {error}"))?;
    Ok(OwnedHandle(derived))
}

fn set_medium_integrity(token: HANDLE) -> Result<(), String> {
    // SECURITY_MAX_SID_SIZE is 68 bytes; use u64 storage for alignment.
    let mut sid_buffer = [0u64; 12];
    let mut sid_size = std::mem::size_of_val(&sid_buffer) as u32;
    let sid = PSID(sid_buffer.as_mut_ptr().cast());
    unsafe { CreateWellKnownSid(WinMediumLabelSid, PSID::default(), sid, &mut sid_size) }
        .map_err(|error| format!("could not build the Medium integrity SID: {error}"))?;
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: SE_GROUP_INTEGRITY,
        },
    };
    let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + unsafe { GetLengthSid(sid) };
    unsafe {
        SetTokenInformation(
            token,
            TokenIntegrityLevel,
            std::ptr::from_ref(&label).cast(),
            size,
        )
    }
    .map_err(|error| format!("could not lower the token to Medium integrity: {error}"))
}

fn remove_privileges(token: HANDLE, facts: &TokenFacts) -> Result<(), String> {
    let extra = privileges_beyond_standard_user(&facts.privileges);
    if extra.is_empty() {
        return Ok(());
    }
    for (name, luid) in token_privileges(token)? {
        if !extra.contains(&name.as_str()) {
            continue;
        }
        let state = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_REMOVED,
            }],
        };
        unsafe { AdjustTokenPrivileges(token, false, Some(&state), 0, None, None) }
            .map_err(|error| format!("could not remove {name}: {error}"))?;
        if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
            return Err(format!(
                "could not remove {name}: not all privileges assigned"
            ));
        }
    }
    Ok(())
}

/// Read one variable-length token information class into aligned storage.
fn token_information(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>, String> {
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token, class, None, 0, &mut needed) };
    if needed == 0 {
        return Err(format!("GetTokenInformation({}) reported no data", class.0));
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    unsafe {
        GetTokenInformation(
            token,
            class,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|error| format!("GetTokenInformation({}) failed: {error}", class.0))?;
    Ok(buffer)
}

fn sid_last_rid(sid: PSID) -> Option<u32> {
    unsafe {
        let count = GetSidSubAuthorityCount(sid);
        if count.is_null() || *count == 0 {
            return None;
        }
        let rid = GetSidSubAuthority(sid, u32::from(*count - 1));
        (!rid.is_null()).then(|| *rid)
    }
}

fn token_privileges(token: HANDLE) -> Result<Vec<(String, LUID)>, String> {
    let buffer = token_information(token, TokenPrivileges)?;
    let header = buffer.as_ptr().cast::<TOKEN_PRIVILEGES>();
    let count = unsafe { (*header).PrivilegeCount } as usize;
    let first = unsafe { std::ptr::addr_of!((*header).Privileges).cast::<LUID_AND_ATTRIBUTES>() };
    let mut privileges = Vec::with_capacity(count);
    for index in 0..count {
        let luid = unsafe { (*first.add(index)).Luid };
        let mut name = [0u16; 128];
        let mut length = name.len() as u32;
        let resolved = unsafe {
            LookupPrivilegeNameW(PCWSTR::null(), &luid, PWSTR(name.as_mut_ptr()), &mut length)
        };
        // An unresolvable privilege keeps a synthetic name, which is never on
        // the standard-user allow list, so validation fails closed.
        let name = match resolved {
            Ok(()) => String::from_utf16_lossy(&name[..length as usize]),
            Err(_) => format!("LUID({:#x}:{:#x})", luid.HighPart, luid.LowPart),
        };
        privileges.push((name, luid));
    }
    Ok(privileges)
}

fn administrators_membership(token: HANDLE) -> Result<AdministratorsMembership, String> {
    let mut sid_buffer = [0u64; 12];
    let mut sid_size = std::mem::size_of_val(&sid_buffer) as u32;
    let administrators = PSID(sid_buffer.as_mut_ptr().cast());
    unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            PSID::default(),
            administrators,
            &mut sid_size,
        )
    }
    .map_err(|error| format!("could not build the Administrators SID: {error}"))?;
    let buffer = token_information(token, TokenGroups)?;
    let header = buffer.as_ptr().cast::<TOKEN_GROUPS>();
    let count = unsafe { (*header).GroupCount } as usize;
    let first = unsafe { std::ptr::addr_of!((*header).Groups).cast::<SID_AND_ATTRIBUTES>() };
    for index in 0..count {
        let group = unsafe { *first.add(index) };
        if unsafe { EqualSid(group.Sid, administrators) }.is_ok() {
            let attributes = group.Attributes;
            return Ok(if attributes & SE_GROUP_USE_FOR_DENY_ONLY != 0 {
                AdministratorsMembership::DenyOnly
            } else if attributes & SE_GROUP_ENABLED != 0 {
                AdministratorsMembership::Enabled
            } else {
                // A present, disabled, allow-capable group can be re-enabled
                // by the token holder, so treat it as enabled.
                AdministratorsMembership::Enabled
            });
        }
    }
    Ok(AdministratorsMembership::Absent)
}

/// Gather the security-relevant facts of `token`.
pub(crate) fn token_facts(token: HANDLE) -> Result<TokenFacts, String> {
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    }
    .map_err(|error| format!("GetTokenInformation(TokenElevation) failed: {error}"))?;
    let label = token_information(token, TokenIntegrityLevel)?;
    let label = label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
    let integrity_rid = sid_last_rid(unsafe { (*label).Label.Sid })
        .ok_or_else(|| "the token integrity label has no RID".to_owned())?;
    Ok(TokenFacts {
        elevated: elevation.TokenIsElevated != 0,
        integrity_rid,
        administrators: administrators_membership(token)?,
        privileges: token_privileges(token)?
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
    })
}

/// Reverts the calling thread's impersonation when dropped, including on
/// unwind, so a thread never keeps a borrowed token.
struct ImpersonationGuard;

impl Drop for ImpersonationGuard {
    fn drop(&mut self) {
        if unsafe { RevertToSelf() }.is_err() {
            // Continuing on a thread with an unknown security context would
            // make later Driver work unpredictable.
            std::process::abort();
        }
    }
}

/// Run `work` on the current thread while impersonating `token`. Windows
/// silently downgrades a disallowed impersonation to Identification level,
/// which would make file access fail, so the level is verified first.
pub(crate) fn with_impersonation<R>(
    token: &OwnedHandle,
    work: impl FnOnce() -> R,
) -> std::io::Result<R> {
    unsafe { ImpersonateLoggedOnUser(token.raw()) }.map_err(std::io::Error::other)?;
    let guard = ImpersonationGuard;
    let mut thread_token = HANDLE::default();
    unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut thread_token) }
        .map_err(std::io::Error::other)?;
    let thread_token = OwnedHandle(thread_token);
    let mut level = SECURITY_IMPERSONATION_LEVEL::default();
    let mut returned = 0u32;
    unsafe {
        GetTokenInformation(
            thread_token.raw(),
            TokenImpersonationLevel,
            Some(std::ptr::from_mut(&mut level).cast::<c_void>()),
            std::mem::size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32,
            &mut returned,
        )
    }
    .map_err(std::io::Error::other)?;
    if level.0 < SecurityImpersonation.0 {
        return Err(std::io::Error::other(format!(
            "Windows granted impersonation level {} instead of SecurityImpersonation",
            level.0
        )));
    }
    let result = work();
    drop(guard);
    Ok(result)
}

/// Require the standard-user token to be able to create, list, and delete
/// entries in the isolated profile directory the Driver prepared.
pub(crate) fn require_profile_writable(
    token: &OwnedHandle,
    profile: &Path,
) -> Result<(), BrowserRefusal> {
    let wide = profile
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let access = FILE_LIST_DIRECTORY.0
        | FILE_TRAVERSE.0
        | FILE_ADD_FILE.0
        | FILE_ADD_SUBDIRECTORY.0
        | FILE_DELETE_CHILD.0;
    let opened = with_impersonation(token, || unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    })
    .map_err(|error| {
        refusal(format!(
            "could not assume the standard-user browser token to check the isolated profile: {error}"
        ))
    })?;
    match opened {
        Ok(handle) => {
            drop(OwnedHandle(handle));
            Ok(())
        }
        Err(error) if error.code() == E_ACCESSDENIED => Err(refusal(
            "the standard-user token that runs the isolated browser cannot write its driver-owned \
             profile directory; remove the custom permissions from the Cua Driver browser profile \
             root",
        )),
        Err(error) => Err(refusal(format!(
            "could not verify the isolated profile for the standard-user browser token: {error}"
        ))),
    }
}

/// The root process of an isolated browser created with a standard-user
/// token. File work in its profile runs while impersonating that token.
pub(crate) struct StandardUserBrowserProcess {
    process: OwnedHandle,
    pid: u32,
    token: Arc<OwnedHandle>,
}

impl StandardUserBrowserProcess {
    fn exit_status(&self) -> std::io::Result<ExitStatus> {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.process.raw(), &mut code) }
            .map_err(std::io::Error::other)?;
        Ok(ExitStatus::from_raw(code))
    }

    #[cfg(test)]
    pub(crate) fn process_handle(&self) -> HANDLE {
        self.process.raw()
    }
}

impl IsolatedBrowserProcess for StandardUserBrowserProcess {
    fn id(&self) -> u32 {
        self.pid
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let waited = unsafe { WaitForSingleObject(self.process.raw(), 0) };
        if waited == WAIT_OBJECT_0 {
            self.exit_status().map(Some)
        } else if waited == WAIT_TIMEOUT {
            Ok(None)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn kill(&mut self) -> std::io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        unsafe { TerminateProcess(self.process.raw(), 1) }.map_err(std::io::Error::other)
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if unsafe { WaitForSingleObject(self.process.raw(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(std::io::Error::last_os_error());
        }
        self.exit_status()
    }

    fn with_browser_file_authority(&self, work: &mut dyn FnMut()) -> std::io::Result<()> {
        with_impersonation(&self.token, work)
    }
}

/// Create the process described by `command` with `token`. Only the program
/// and arguments are used: the child inherits no handles (so no elevated
/// handle crosses the boundary), the Driver's environment, and the Driver's
/// window station and desktop.
pub(crate) fn spawn_with_token(
    token: Arc<OwnedHandle>,
    command: &Command,
) -> Result<StandardUserBrowserProcess, String> {
    if command.get_envs().next().is_some() || command.get_current_dir().is_some() {
        return Err(
            "the isolated browser command sets an environment or working directory that the \
             standard-user launch does not forward"
                .to_owned(),
        );
    }
    let program = command.get_program().encode_wide().collect::<Vec<_>>();
    let args = command
        .get_args()
        .map(|arg| arg.encode_wide().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let mut command_line = windows_command_line(&program, &args)?;
    command_line.push(0);
    let application = program
        .into_iter()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut information = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessAsUserW(
            token.raw(),
            PCWSTR(application.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            PCWSTR::null(),
            &startup,
            &mut information,
        )
    }
    .map_err(|error| format!("CreateProcessAsUserW failed: {error}"))?;
    drop(OwnedHandle(information.hThread));
    Ok(StandardUserBrowserProcess {
        process: OwnedHandle(information.hProcess),
        pid: information.dwProcessId,
        token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_launch_token::HIGH_INTEGRITY_RID;

    fn process_token_facts(process: HANDLE) -> TokenFacts {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
            .expect("open child process token");
        let token = OwnedHandle(token);
        token_facts(token.raw()).expect("child token facts")
    }

    fn system32() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32")
    }

    #[test]
    fn driver_token_facts_are_readable_and_consistent() {
        let token = current_process_token(TOKEN_QUERY).expect("process token");
        let facts = token_facts(token.raw()).expect("token facts");
        assert!(facts.integrity_rid >= MEDIUM_INTEGRITY_RID, "{facts:?}");
        assert!(
            facts
                .privileges
                .iter()
                .any(|name| name == "SeChangeNotifyPrivilege"),
            "{facts:?}"
        );
        if facts.integrity_rid >= HIGH_INTEGRITY_RID {
            assert_eq!(
                browser_launch_token(&facts),
                BrowserLaunchToken::StandardUser
            );
        }
    }

    /// On an elevated host (GitHub-hosted Windows runs tests elevated), derive
    /// the real standard-user token, prove its posture, prove impersonation
    /// cannot write Program Files, and prove the created process runs with it.
    #[test]
    fn elevated_driver_launches_children_with_a_verified_standard_user_token() {
        let driver_token = current_process_token(TOKEN_QUERY).expect("process token");
        let driver = token_facts(driver_token.raw()).expect("driver facts");
        let context = browser_launch_context().expect("launch context");
        let BrowserLaunchContext::StandardUser(token) = context else {
            assert_eq!(browser_launch_token(&driver), BrowserLaunchToken::Driver);
            eprintln!("host token is not elevated; standard-user derivation is not exercised");
            return;
        };
        let derived = token_facts(token.raw()).expect("derived facts");
        assert_eq!(validate_standard_user_token(&driver, &derived), Ok(()));
        assert_ne!(
            derived.administrators,
            AdministratorsMembership::Enabled,
            "{derived:?}"
        );
        assert!(derived.integrity_rid <= MEDIUM_INTEGRITY_RID, "{derived:?}");

        // Impersonation reaches file access checks: the derived token cannot
        // add files to System32 even though the elevated Driver can.
        let system = system32()
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let denied = with_impersonation(&token, || unsafe {
            CreateFileW(
                PCWSTR(system.as_ptr()),
                FILE_ADD_FILE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        })
        .expect("impersonate the standard-user token");
        match denied {
            Ok(handle) => {
                drop(OwnedHandle(handle));
                panic!("the standard-user token must not be able to write System32");
            }
            Err(error) => assert_eq!(error.code(), E_ACCESSDENIED, "{error}"),
        }

        // The derived token cannot open the elevated Driver process to write
        // its memory or inject a thread.
        let driver_pid = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
        let opened = with_impersonation(&token, || unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_VM_WRITE
                    | windows::Win32::System::Threading::PROCESS_CREATE_THREAD,
                false,
                driver_pid,
            )
        })
        .expect("impersonate the standard-user token");
        match opened {
            Ok(handle) => {
                drop(OwnedHandle(handle));
                panic!("the standard-user token must not be able to write the Driver process");
            }
            Err(error) => assert_eq!(error.code(), E_ACCESSDENIED, "{error}"),
        }

        // The thread is back on the Driver's token afterwards.
        let mut thread_token = HANDLE::default();
        assert!(
            unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut thread_token) }
                .is_err(),
            "impersonation must be reverted"
        );

        let mut command = Command::new(system32().join("PING.EXE"));
        command.args(["-n", "30", "127.0.0.1"]);
        let mut child = spawn_with_token(token.clone(), &command).expect("spawn with token");
        let child_facts = process_token_facts(child.process_handle());
        assert_eq!(
            validate_standard_user_token(&driver, &child_facts),
            Ok(()),
            "{child_facts:?}"
        );
        assert!(child.try_wait().expect("try_wait").is_none());
        child.kill().expect("terminate child");
        let status = child.wait().expect("wait child");
        assert_eq!(status.code(), Some(1));
        child.kill().expect("killing an exited child is a no-op");

        let mut command = Command::new(system32().join("cmd.exe"));
        command.args(["/d", "/c", "exit 7"]);
        let mut child = spawn_with_token(token, &command).expect("spawn with token");
        assert_eq!(child.wait().expect("wait").code(), Some(7));
    }

    #[test]
    fn standard_user_launch_refuses_unforwarded_command_state() {
        let token = Arc::new(current_process_token(TOKEN_QUERY).expect("token"));
        let mut command = Command::new(system32().join("cmd.exe"));
        command.env("CUA_TEST", "1");
        assert!(spawn_with_token(token.clone(), &command).is_err());
        let mut command = Command::new(system32().join("cmd.exe"));
        command.current_dir(system32());
        assert!(spawn_with_token(token, &command).is_err());
    }
}
