//! Standard-user app launches for an elevated Windows Driver (#3607).
//!
//! A process inherits its creator's primary token. An elevated Driver (the
//! default autostart daemon runs at `RunLevel=Highest`) would therefore start
//! every `launch_app` target as an elevated administrator: the user's ordinary
//! tools could not close or automate the app, and it would behave differently
//! from the same app started from Explorer. An elevated Driver instead
//! launches apps with the verified standard-user token that already runs
//! isolated browsers (#4234): the same user and desktop, `BUILTIN\Administrators`
//! deny-only, and Medium integrity. The Driver keeps full control, because a
//! higher-integrity process may send input to and automate a lower one.
//!
//! Direct executable launches use `CreateProcessAsUserW` with that token.
//! `ShellExecuteExW` (PATH and App Paths lookup, file associations,
//! `shell:AppsFolder` registrations, URLs) always creates processes with its
//! caller's primary token, so shell launches run in a short-lived helper that
//! already holds the standard-user token: this Driver executable, started with
//! [`SHELL_LAUNCH_HELPER_FLAG`]. The helper performs one `ShellExecuteExW` call,
//! reports the launched process ID or the shell error on its standard output,
//! and exits. A process that embeds the Driver runtime without the Driver's
//! entry point (for example an SDK host) cannot run the helper, so an elevated
//! host of that kind refuses shell launches instead of starting them elevated.

use std::ffi::{c_void, OsStr, OsString};
use std::io::Read as _;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW,
};

use crate::browser_launch_token::windows_command_line;
use crate::browser_standard_user::{browser_launch_context, BrowserLaunchContext, OwnedHandle};

/// First argument that turns this Driver executable into the shell launch
/// helper. Never a public command.
pub const SHELL_LAUNCH_HELPER_FLAG: &str = "--cua-driver-standard-user-shell-launch";

/// Set once the Driver's own entry point has checked for the helper flag,
/// which proves this executable can serve as the helper.
static SHELL_LAUNCH_HELPER_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// The token that runs apps started by `launch_app`.
#[derive(Clone)]
pub(crate) enum AppLaunchToken {
    /// The Driver is not elevated; apps start with the Driver's own token.
    Driver,
    /// A verified standard-user token derived from the elevated Driver token.
    StandardUser(Arc<OwnedHandle>),
}

/// Choose the launch token. An elevated Driver that cannot derive a verified
/// standard-user token refuses rather than starting the app elevated.
pub(crate) fn app_launch_token() -> Result<AppLaunchToken, String> {
    match browser_launch_context() {
        Ok(BrowserLaunchContext::Driver) => Ok(AppLaunchToken::Driver),
        Ok(BrowserLaunchContext::StandardUser(token)) => Ok(AppLaunchToken::StandardUser(token)),
        Err(refusal) => Err(format!(
            "The Driver is elevated and could not prepare the standard-user token it uses to \
             launch apps, so no app was started: {}",
            refusal.message
        )),
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

fn nul_terminated(mut value: Vec<u16>) -> Vec<u16> {
    value.push(0);
    value
}

/// Create `application` with `command_line` under `token`, showing its first
/// window with `n_show`. The child inherits no handles.
pub(crate) fn create_process(
    token: &OwnedHandle,
    application: &str,
    command_line: &str,
    n_show: i32,
) -> Result<u32, String> {
    let application = nul_terminated(wide(OsStr::new(application)));
    let mut command_line = nul_terminated(wide(OsStr::new(command_line)));
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        wShowWindow: n_show as u16,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
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
            &mut process,
        )
    }
    .map_err(|error| format!("CreateProcessAsUserW failed: {error}"))?;
    drop(OwnedHandle::new(process.hThread));
    drop(OwnedHandle::new(process.hProcess));
    Ok(process.dwProcessId)
}

/// Run `ShellExecuteExW` for `file` and `parameters` in a helper that holds
/// `token`. Returns the launched process ID, or 0 when the shell handed the
/// request to an existing process (for example a URL opened in a running
/// browser). The helper is terminated if it does not finish within `timeout`.
pub(crate) fn shell_execute(
    token: &OwnedHandle,
    file: &str,
    parameters: &str,
    n_show: i32,
    timeout: Duration,
) -> Result<u32, String> {
    if !SHELL_LAUNCH_HELPER_AVAILABLE.load(Ordering::Relaxed) {
        return Err(
            "The Driver is elevated and this process cannot start its standard-user shell launch \
             helper, so no app was started. Launch the app by executable path with \
             start_minimized, or run this host without elevation."
                .to_owned(),
        );
    }
    let helper = std::env::current_exe()
        .map_err(|error| format!("could not locate the Driver executable: {error}"))?;
    let program = wide(helper.as_os_str());
    let args = [
        wide(OsStr::new(SHELL_LAUNCH_HELPER_FLAG)),
        wide(OsStr::new(&n_show.to_string())),
        wide(OsStr::new(file)),
        wide(OsStr::new(parameters)),
    ];
    let mut command_line = nul_terminated(windows_command_line(&program, &args)?);
    let application = nul_terminated(program);

    // Only the write end of this pipe crosses into the helper.
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe { CreatePipe(&mut read, &mut write, None, 0) }
        .map_err(|error| format!("could not create the helper result pipe: {error}"))?;
    let read = OwnedHandle::new(read);
    let write = OwnedHandle::new(write);
    unsafe { SetHandleInformation(write.raw(), HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
        .map_err(|error| format!("could not share the helper result pipe: {error}"))?;

    let mut attribute_size = 0usize;
    let _ = unsafe {
        InitializeProcThreadAttributeList(
            LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            1,
            0,
            &mut attribute_size,
        )
    };
    let mut attribute_storage = vec![0u64; attribute_size.div_ceil(8)];
    let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(attribute_storage.as_mut_ptr().cast());
    unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut attribute_size) }
        .map_err(|error| format!("could not prepare the helper handle list: {error}"))?;
    let inherited = [write.raw()];
    let updated = unsafe {
        UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(inherited.as_ptr().cast::<c_void>()),
            std::mem::size_of_val(&inherited),
            None,
            None,
        )
    };
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdOutput: write.raw(),
            hStdError: write.raw(),
            ..Default::default()
        },
        lpAttributeList: attributes,
    };
    let mut process = PROCESS_INFORMATION::default();
    let created = updated.and_then(|()| unsafe {
        CreateProcessAsUserW(
            token.raw(),
            PCWSTR(application.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            true,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut process,
        )
    });
    unsafe { DeleteProcThreadAttributeList(attributes) };
    created.map_err(|error| format!("could not start the standard-user launch helper: {error}"))?;
    drop(write);
    drop(OwnedHandle::new(process.hThread));
    let helper = OwnedHandle::new(process.hProcess);

    let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    let waited = unsafe { WaitForSingleObject(helper.raw(), timeout_ms) };
    if waited == WAIT_TIMEOUT {
        let _ = unsafe { TerminateProcess(helper.raw(), 1) };
        return Err(format!(
            "the standard-user launch helper did not finish within {}s; a blocking shell dialog \
             likely appeared on the session desktop",
            timeout.as_secs()
        ));
    }
    if waited != WAIT_OBJECT_0 {
        return Err(format!(
            "could not wait for the standard-user launch helper: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut exit_code = 0u32;
    let _ = unsafe { GetExitCodeProcess(helper.raw(), &mut exit_code) };

    // The helper has exited, so its end of the pipe is closed and the read
    // ends at the helper's last write.
    let mut output = String::new();
    let mut pipe = unsafe { std::fs::File::from_raw_handle(read.into_raw().0) };
    let _ = pipe.read_to_string(&mut output);
    parse_helper_output(&output, exit_code)
}

/// Interpret the helper's single result line.
fn parse_helper_output(output: &str, exit_code: u32) -> Result<u32, String> {
    let line = output.lines().next().unwrap_or("").trim();
    if let Some(pid) = line.strip_prefix("ok ") {
        return pid
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("the standard-user launch helper reported {line:?}"));
    }
    if let Some(message) = line.strip_prefix("error ") {
        return Err(message.trim().to_owned());
    }
    Err(format!(
        "the standard-user launch helper exited with code {exit_code:#x} without a result"
    ))
}

/// Entry point for the helper. The Driver's `main` calls this first; it
/// returns `None` for every ordinary invocation.
pub fn run_shell_launch_helper_if_requested() -> Option<i32> {
    SHELL_LAUNCH_HELPER_AVAILABLE.store(true, Ordering::Relaxed);
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(SHELL_LAUNCH_HELPER_FLAG)) {
        return None;
    }
    let rest: Vec<OsString> = args.collect();
    let result = match rest.as_slice() {
        [n_show, file, parameters] => match n_show.to_str().and_then(|v| v.parse::<i32>().ok()) {
            Some(n_show) => helper_shell_execute(file, parameters, n_show),
            None => Err("the launch helper received an invalid show state".to_owned()),
        },
        _ => Err("the launch helper received malformed arguments".to_owned()),
    };
    match result {
        Ok(pid) => {
            println!("ok {pid}");
            Some(0)
        }
        Err(message) => {
            println!("error {message}");
            Some(1)
        }
    }
}

fn helper_shell_execute(file: &OsStr, parameters: &OsStr, n_show: i32) -> Result<u32, String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Com::{
        CoInitializeEx, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::System::Threading::GetProcessId;
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    // Shell verbs may be implemented by COM handlers.
    let _ = unsafe {
        CoInitializeEx(
            None,
            COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0),
        )
    };
    let verb = nul_terminated(wide(OsStr::new("open")));
    let file = nul_terminated(wide(file));
    let parameters_wide = nul_terminated(wide(parameters));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: if parameters.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(parameters_wide.as_ptr())
        },
        nShow: n_show,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|error| error.to_string())?;
    if info.hProcess.is_invalid() {
        return Ok(0);
    }
    let pid = unsafe { GetProcessId(info.hProcess) };
    let _ = unsafe { CloseHandle(info.hProcess) };
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::parse_helper_output;

    #[test]
    fn helper_output_reports_the_process_id_or_the_shell_error() {
        assert_eq!(parse_helper_output("ok 4242\r\n", 0), Ok(4242));
        assert_eq!(parse_helper_output("ok 0\n", 0), Ok(0));
        assert_eq!(
            parse_helper_output("error The system cannot find the file specified.\n", 1),
            Err("The system cannot find the file specified.".to_owned())
        );
        assert_eq!(
            parse_helper_output("", 0xc000_0005),
            Err(
                "the standard-user launch helper exited with code 0xc0000005 without a result"
                    .to_owned()
            )
        );
    }
}
