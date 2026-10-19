// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! OS biometric-authorization gate for **sensitive** session exports.
//!
//! The export runs on the end user's own machine, so any code path that reads a
//! `sensitive: true` [`crate::ManifestItem`] and packs it into a
//! [`cua_teleport_bundle::bundle::SessionBundle`] must first obtain interactive OS user
//! authorization. This prevents a hijacked or misused SDK from silently
//! exfiltrating a user's credentials (for example a Claude Code OAuth token)
//! off their computer.
//!
//! The gate is enforced inside this library — at the export choke point in the
//! [`crate::ExportProvider`] trait — so the CLI and any embedder are both
//! covered, not just a consent UI.
//!
//! ## Platform behavior
//!
//! - **macOS**: prompt via the LocalAuthentication framework
//!   (`LAContext` / `evaluatePolicy:localizedReason:reply:`) using
//!   `LAPolicyDeviceOwnerAuthentication` — Touch ID with a device-passcode
//!   fallback, the robust choice for an unsigned CLI.
//! - **Windows**: Windows Hello (`UserConsentVerifier`): the face, fingerprint
//!   or PIN the user set up, shown over the foreground window. Where Hello is
//!   not set up (VMs, Server, many managed PCs) or a policy turns it off, the
//!   fallback is Windows' own credential prompt
//!   (`CredUIPromptForWindowsCredentials`, as UAC asks): the user types their
//!   account password, which `LogonUserW` verifies, and the logon must be for
//!   the very account this process runs as (compared by SID). An account with
//!   no password, or a prompt that cannot be shown, is
//!   [`AuthError::Unavailable`].
//! - **Linux**: polkit. `pkcheck` asks the desktop's authentication agent to
//!   confirm the user ([`POLKIT_ACTION`], `auth_self`: their own password or
//!   fingerprint, every time). No polkit, no agent or no installed action is
//!   [`AuthError::Unavailable`].
//! - **Other platforms**: fail **closed** — [`AuthError::Unavailable`] — unless
//!   the operator sets `CUA_ENV_ALLOW_UNVERIFIED_SENSITIVE_EXPORT=1`, an explicit
//!   override that logs to stderr and proceeds. The same override applies on
//!   Windows and Linux when their prompt is unavailable. This keeps those
//!   systems from silently exfiltrating while not hard-breaking deliberate use.
//!
//! ## Escape hatch (test/CI only)
//!
//! Setting `CUA_ENV_SKIP_SENSITIVE_AUTH=1` bypasses the gate entirely (returns
//! `Ok`). It exists so unit tests never raise an interactive prompt; it must not
//! be set in production.

use std::collections::HashSet;

use cua_teleport_bundle::ManifestItem;

/// Env var that bypasses the gate entirely (returns `Ok`). **Test/CI only** —
/// documented so unit tests never raise an interactive prompt.
pub const SKIP_ENV: &str = "CUA_ENV_SKIP_SENSITIVE_AUTH";

/// Env var that, on non-macOS platforms, lets a sensitive export proceed
/// without OS authorization. An explicit operator override; when set, the gate
/// logs to stderr that it proceeded unverified.
pub const ALLOW_UNVERIFIED_ENV: &str = "CUA_ENV_ALLOW_UNVERIFIED_SENSITIVE_EXPORT";

/// What the user needs for the confirmation to be possible on this system.
#[cfg(target_os = "macos")]
const UNAVAILABLE_HELP: &str = "this Mac needs Touch ID or a login password";
#[cfg(target_os = "windows")]
const UNAVAILABLE_HELP: &str = "Cua asks for Windows Hello, or your Windows account password when Hello is not set up; this account has neither (set a password in Settings, Accounts, Sign-in options)";
#[cfg(target_os = "linux")]
const UNAVAILABLE_HELP: &str = "on Linux this needs Cua Spaces installed from the .deb (it sets up the password prompt; the AppImage cannot) and a desktop session with a polkit authentication agent running";
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
const UNAVAILABLE_HELP: &str = "this platform has no confirmation prompt";

/// What a refusal means, and what to do when no prompt was shown. On Linux
/// polkit refuses without asking when its policy does not let this session
/// ask (an older policy, in a remote desktop session: CUA-1725),
/// so the words say how to get the prompt.
#[cfg(target_os = "linux")]
const DENIED: &str = "polkit did not confirm it is you. If no password prompt appeared, update Cua Spaces from the latest .deb (it sets up the prompt) and use a desktop session with a polkit authentication agent running";
#[cfg(not(target_os = "linux"))]
const DENIED: &str = "biometric authorization was denied";

/// Why an interactive authorization did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The user (or the OS) refused: wrong biometric/passcode, or the policy
    /// evaluation failed for a non-cancel reason.
    Denied,
    /// No authorization mechanism is available on this platform/build (for
    /// example a non-macOS host without the operator override, or macOS with no
    /// Touch ID and no passcode set).
    Unavailable,
    /// The user actively dismissed the prompt, or the OS cancelled it.
    Cancelled,
    /// The system's prompt could not run (polkit answered with an error), so
    /// nobody was asked: an error to report, not a refusal.
    Failed(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => f.write_str(DENIED),
            Self::Unavailable => write!(f, "user confirmation is unavailable: {UNAVAILABLE_HELP}"),
            Self::Cancelled => write!(f, "biometric authorization was cancelled"),
            Self::Failed(detail) => write!(f, "the confirmation prompt failed: {detail}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Whether the current selection would pack at least one `sensitive: true`
/// manifest item into the bundle.
///
/// `include == None` means "everything the scope implies" (every manifest
/// item); a set restricts the selection to the checked `rel_path`s, matching
/// the export choke point's semantics. Pure, so the gate wiring is unit-testable
/// without touching the OS or any env var.
pub fn selection_is_sensitive(items: &[ManifestItem], include: Option<&HashSet<String>>) -> bool {
    items
        .iter()
        .filter(|item| include.is_none_or(|set| set.contains(&item.rel_path)))
        .any(|item| item.sensitive)
}

/// Obtain interactive OS user authorization before a sensitive session export
/// leaves this machine.
///
/// `reason` is shown to the user (on macOS it fills the LocalAuthentication
/// prompt's "…is trying to <reason>" line). Returns `Ok(())` when authorized and
/// an [`AuthError`] otherwise; callers MUST abort the export on `Err`.
///
/// Honors `CUA_ENV_SKIP_SENSITIVE_AUTH=1` (test/CI escape hatch) first, before any
/// platform-specific work, so tests never prompt.
///
/// The bypass is honored only in debug/test builds. A released (optimized)
/// binary compiles with `debug_assertions` off, so [`skip_is_honored`] returns
/// false there regardless of the environment: a shipped Cua has no bypass
/// (red-team E5). Under `cargo test` the crate is built with `debug_assertions`
/// on, so unit tests still never raise an interactive prompt.
pub fn authorize_sensitive_export(reason: &str) -> Result<(), AuthError> {
    authorize_unless_skipped(
        skip_is_honored(cfg!(debug_assertions), env_flag(SKIP_ENV)),
        reason,
    )
}

/// Whether the `CUA_ENV_SKIP_SENSITIVE_AUTH` escape hatch takes effect.
///
/// Pure, so a test can assert that in a non-debug (released) build path the
/// environment variable has no effect at all. `debug_build` is normally
/// `cfg!(debug_assertions)`.
pub fn skip_is_honored(debug_build: bool, env_set: bool) -> bool {
    debug_build && env_set
}

/// The gate with the escape hatch passed in, so it is testable without
/// mutating process-global environment variables.
fn authorize_unless_skipped(skip: bool, reason: &str) -> Result<(), AuthError> {
    if skip {
        return Ok(());
    }
    authorize_platform(reason)
}

/// Read a boolean env var: true only when set to exactly `"1"`.
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| value == "1")
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn authorize_platform(reason: &str) -> Result<(), AuthError> {
    macos::authorize(reason)
}

/// Windows Hello or polkit; where that prompt is unavailable, the operator's
/// explicit override alone lets the export through. A prompt the user
/// answered, either way, is final.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn authorize_platform(reason: &str) -> Result<(), AuthError> {
    #[cfg(target_os = "windows")]
    let asked = windows_authorize(reason);
    #[cfg(target_os = "linux")]
    let asked = polkit::authorize(POLKIT_ACTION, reason);
    match asked {
        Err(AuthError::Unavailable) => non_macos_decision(env_flag(ALLOW_UNVERIFIED_ENV), reason),
        other => other,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn authorize_platform(reason: &str) -> Result<(), AuthError> {
    non_macos_decision(env_flag(ALLOW_UNVERIFIED_ENV), reason)
}

/// Fail-closed decision for platforms without a (usable) prompt.
///
/// Split out (and pure) so it can be unit-tested without mutating process env.
/// With the operator override it logs to stderr and proceeds; otherwise it
/// refuses so the sensitive data never leaves the machine unverified.
#[cfg(not(target_os = "macos"))]
fn non_macos_decision(allow_unverified: bool, reason: &str) -> Result<(), AuthError> {
    if allow_unverified {
        eprintln!(
            "cua-spacesd-client: WARNING: sensitive session export proceeding WITHOUT OS biometric \
             authorization because {ALLOW_UNVERIFIED_ENV}=1 is set ({reason})"
        );
        Ok(())
    } else {
        Err(AuthError::Unavailable)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! macOS LocalAuthentication backend.
    //!
    //! `evaluatePolicy:localizedReason:reply:` is asynchronous: it returns
    //! immediately and invokes the completion block on a framework-private
    //! queue. We bridge that back to a synchronous call by handing the block an
    //! `mpsc::Sender` and blocking the caller on the receiver until the reply
    //! arrives. The `LAContext` is held on the stack across that wait so the
    //! evaluation is not cancelled by the context being deallocated.

    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    use super::AuthError;

    /// Upper bound on how long we wait for the user to answer the prompt before
    /// treating it as cancelled, so a forgotten dialog cannot wedge the export
    /// forever.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn authorize(reason: &str) -> Result<(), AuthError> {
        // Touch ID OR device passcode fallback (LAPolicyDeviceOwnerAuthentication,
        // value 2): the robust choice for an unsigned CLI that may run where no
        // biometric is enrolled but a passcode is set.
        let policy = LAPolicy::DeviceOwnerAuthentication;

        // SAFETY: `LAContext::new` returns a fresh, owned context.
        let context = unsafe { LAContext::new() };

        // Preflight: if the policy can't be evaluated at all (no passcode, no
        // biometric enrolled, unsupported hardware) treat it as unavailable
        // rather than raising a prompt that is guaranteed to fail.
        // SAFETY: `context` is a valid LAContext; the call only reads state.
        if unsafe { context.canEvaluatePolicy_error(policy) }.is_err() {
            return Err(AuthError::Unavailable);
        }

        let ns_reason = NSString::from_str(reason);
        let (tx, rx) = mpsc::channel::<Result<(), AuthError>>();

        // The reply block runs on a private framework queue in an unspecified
        // thread; `Sender` is `Send`, so handing the result back over the channel
        // is sound. `error` is only borrowed for the duration of the block.
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let result = if success.as_bool() {
                Ok(())
            } else {
                // SAFETY: on failure LocalAuthentication passes a valid NSError;
                // guard against null defensively.
                let code = if error.is_null() {
                    0
                } else {
                    unsafe { (*error).code() }
                };
                Err(classify(code))
            };
            let _ = tx.send(result);
        });

        // SAFETY: all arguments are valid; the reply block is kept alive by
        // `reply` until after the evaluation completes (we block on `rx` below,
        // and `reply`/`context` outlive that wait).
        unsafe {
            context.evaluatePolicy_localizedReason_reply(policy, &ns_reason, &reply);
        }

        match rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(result) => result,
            // No reply within the budget: treat as cancelled rather than a
            // silent success.
            Err(_) => Err(AuthError::Cancelled),
        }
    }

    /// Map an `LAError` code to an [`AuthError`]. User/OS cancellations are
    /// distinguished from an outright denial so callers can message accordingly.
    fn classify(code: isize) -> AuthError {
        // LAErrorUserCancel = -2, LAErrorSystemCancel = -4, LAErrorAppCancel = -9.
        match code {
            -2 | -4 | -9 => AuthError::Cancelled,
            _ => AuthError::Denied,
        }
    }
}

/// The polkit action Linux asks the desktop to confirm
/// (`apps/cua-spaces-desktop/packaging/ai.cua.spaces.policy` installs it).
pub const POLKIT_ACTION: &str = "ai.cua.spaces.keyvault";

/// The polkit action the Cua Spaces app asks for before it approves another
/// device for the account (the same policy file).
pub const POLKIT_DEVICE_ACTION: &str = "ai.cua.spaces.devices";

/// Confirms the person at this computer with the system's own prompt: Touch
/// ID, an Apple Watch or the login password on macOS, Windows Hello on
/// Windows, polkit's `polkit_action` on Linux. Unlike
/// [`authorize_sensitive_export`] there is no operator override: where the
/// system has no usable prompt the answer is [`AuthError::Unavailable`], so
/// the caller can say so and decide. The test escape hatch
/// ([`skip_is_honored`]) still applies in debug builds.
pub fn confirm_user(polkit_action: &str, reason: &str) -> Result<(), AuthError> {
    if skip_is_honored(cfg!(debug_assertions), env_flag(SKIP_ENV)) {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let _ = polkit_action;
        macos::authorize(reason)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = polkit_action;
        windows_authorize(reason)
    }
    #[cfg(target_os = "linux")]
    {
        polkit::authorize(polkit_action, reason)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (polkit_action, reason);
        Err(AuthError::Unavailable)
    }
}

/// Windows: Hello when it is set up, else the account password prompt. A
/// Hello prompt the user answered, either way, is final: a refusal or a
/// cancel never falls through to the password prompt.
#[cfg(target_os = "windows")]
fn windows_authorize(reason: &str) -> Result<(), AuthError> {
    match hello::authorize(reason) {
        Err(AuthError::Unavailable) => password::authorize(reason),
        other => other,
    }
}

/// How many wrong passwords one confirmation allows before it is refused.
/// Each wrong one also counts toward the account's lockout policy, as at the
/// sign-in screen.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const PASSWORD_ATTEMPTS: u32 = 3;

/// The pieces of the name a credential prompt returned: `DOMAIN\user`,
/// `user@domain` (a UPN, passed whole with no domain) or a bare `user`.
/// Returns (user, domain); an absent domain is `.`, this computer's own
/// accounts.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn split_account(name: &str) -> (String, Option<String>) {
    if let Some((domain, user)) = name.split_once('\\') {
        let domain = if domain.is_empty() { "." } else { domain };
        return (user.to_string(), Some(domain.to_string()));
    }
    if name.contains('@') {
        return (name.to_string(), None);
    }
    (name.to_string(), Some(".".to_string()))
}

/// The password prompt's outcome for one typed credential, decided from what
/// the system said about it. Pure so every branch is testable off Windows:
/// `password_empty` (a blank password proves nothing), `logon_ok`
/// (`LogonUserW` accepted it), `same_account` (the logon token's SID is this
/// process's own).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn password_verdict(password_empty: bool, logon_ok: bool, same_account: bool) -> PasswordVerdict {
    if password_empty || !logon_ok {
        PasswordVerdict::Retry
    } else if !same_account {
        // Right password, another account: never confirms this user.
        PasswordVerdict::Refuse
    } else {
        PasswordVerdict::Confirmed
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasswordVerdict {
    Confirmed,
    Retry,
    Refuse,
}

/// `UserConsentVerificationResult`'s values, as the system numbers them.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn hello_result(value: i32) -> Result<(), AuthError> {
    match value {
        0 => Ok(()),
        // DeviceNotPresent, NotConfiguredForUser, DisabledByPolicy: no Hello to ask.
        1..=3 => Err(AuthError::Unavailable),
        // Canceled.
        6 => Err(AuthError::Cancelled),
        // DeviceBusy, RetriesExhausted, and anything the system adds later.
        _ => Err(AuthError::Denied),
    }
}

/// `pkcheck`'s exit status (pkcheck(1)): 0 authorized, 1 not authorized,
/// 2 no authentication agent to ask (the challenge could not be made),
/// 3 the user dismissed the dialog, 127 an error, said on stderr. An error
/// (or a signal) is reported as one, never as the user's refusal, except
/// an action polkit does not know: the policy is not installed (the
/// AppImage cannot install it), so no prompt can be shown here
/// (CUA-1855).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pkcheck_result(code: Option<i32>, stderr: &str) -> Result<(), AuthError> {
    let detail = || {
        let line = stderr.lines().map(str::trim).find(|l| !l.is_empty());
        match (line, code) {
            (Some(line), _) => line.chars().take(300).collect(),
            (None, Some(code)) => format!("pkcheck exited with status {code}"),
            (None, None) => "pkcheck was stopped".to_string(),
        }
    };
    match code {
        Some(0) => Ok(()),
        Some(1) => Err(AuthError::Denied),
        Some(2) => Err(AuthError::Unavailable),
        Some(3) => Err(AuthError::Cancelled),
        _ if stderr.contains("is not registered") => Err(AuthError::Unavailable),
        _ => Err(AuthError::Failed(detail())),
    }
}

/// The process start time (clock ticks since boot) in a `/proc/<pid>/stat`
/// line: the 22nd field, counted after the process name, which may hold
/// spaces and parentheses itself. polkit names a process by id and start time,
/// so a recycled id is never mistaken for it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    // The fields after the name begin with the state (field 3).
    after_name.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "windows")]
mod hello {
    //! Windows Hello through `UserConsentVerifier`.
    //!
    //! The prompt is a system dialog (CredentialUIBroker), so it needs a window
    //! to sit over: the foreground one, which is the app that asked when the
    //! app brought itself forward first. The call blocks until the user
    //! answers, on a thread of its own in the multithreaded apartment.

    use windows::Foundation::IAsyncOperation;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::System::WinRT::{
        IUserConsentVerifierInterop, RO_INIT_MULTITHREADED, RoInitialize,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetDesktopWindow, GetForegroundWindow};
    use windows::core::{HSTRING, factory};

    use super::{AuthError, hello_result};

    pub(super) fn authorize(reason: &str) -> Result<(), AuthError> {
        // SAFETY: RoInitialize takes no pointers. A thread that is already
        // initialized answers an error that changes nothing.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let available = UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|op| op.get())
            .map_err(|_| AuthError::Unavailable)?;
        if available != UserConsentVerifierAvailability::Available {
            return Err(AuthError::Unavailable);
        }
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(|_| AuthError::Unavailable)?;
        // SAFETY: both calls take no arguments and answer a handle (or null).
        let window = unsafe {
            let w = GetForegroundWindow();
            if w.is_invalid() {
                GetDesktopWindow()
            } else {
                w
            }
        };
        // SAFETY: `window` is a window handle of this desktop and the message
        // lives through the call.
        let op: IAsyncOperation<UserConsentVerificationResult> =
            unsafe { interop.RequestVerificationForWindowAsync(window, &HSTRING::from(reason)) }
                .map_err(|_| AuthError::Unavailable)?;
        let result = op.get().map_err(|_| AuthError::Denied)?;
        hello_result(result.0)
    }
}

#[cfg(target_os = "windows")]
mod password {
    //! The fallback where Windows Hello is not set up: Windows' own
    //! credential prompt, the one UAC uses (`CredUIPromptForWindowsCredentials`),
    //! then `LogonUserW` to check what was typed. The prompt shows this
    //! user's name already filled in. The logon must produce a token for the
    //! SID this process runs as, so another account's password (an
    //! administrator's, say) never confirms this user. The password lives in
    //! buffers that are overwritten before they are freed, and is never
    //! logged or returned.

    use std::ffi::c_void;

    use windows::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, ERROR_LOGON_FAILURE, ERROR_SUCCESS, HANDLE,
    };
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
    use windows::Win32::Security::Credentials::{
        CREDUI_INFOW, CREDUIWIN_GENERIC, CredPackAuthenticationBufferW,
        CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW,
    };
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT,
        LogonUserW, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    use super::{AuthError, PASSWORD_ATTEMPTS, PasswordVerdict, password_verdict, split_account};

    /// Overwrites `len` bytes in a way the optimizer may not drop.
    fn wipe(ptr: *mut u8, len: usize) {
        for i in 0..len {
            // SAFETY: the caller passes a live allocation of `len` bytes.
            unsafe { std::ptr::write_volatile(ptr.add(i), 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }

    /// A UTF-16 buffer overwritten with zeros when dropped.
    struct Secret(Vec<u16>);

    impl Drop for Secret {
        fn drop(&mut self) {
            wipe(self.0.as_mut_ptr().cast(), self.0.len() * 2);
        }
    }

    /// A prompt result the system allocated, wiped and freed on drop.
    struct AuthBuffer(*mut c_void, u32);

    impl Drop for AuthBuffer {
        fn drop(&mut self) {
            if !self.0.is_null() {
                wipe(self.0.cast(), self.1 as usize);
                // SAFETY: the prompt allocated it with CoTaskMemAlloc.
                unsafe { CoTaskMemFree(Some(self.0)) };
            }
        }
    }

    /// A kernel handle closed on drop.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a call that opened it.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// A token's TOKEN_USER, with the SID it points to, in one allocation.
    struct TokenUserBuf(Vec<u64>);

    impl TokenUserBuf {
        fn of(token: HANDLE) -> Option<Self> {
            let mut need = 0u32;
            // SAFETY: a null buffer asks for the size.
            unsafe {
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut need);
            }
            if need == 0 {
                return None;
            }
            // u64 elements keep the buffer aligned for TOKEN_USER.
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            // SAFETY: `buf` is at least `need` bytes.
            unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    Some(buf.as_mut_ptr().cast()),
                    need,
                    &mut need,
                )
            }
            .ok()?;
            Some(Self(buf))
        }

        fn sid(&self) -> PSID {
            // SAFETY: the buffer holds a TOKEN_USER written by the system.
            unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
        }
    }

    fn wide_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `DOMAIN\user` of the account this process runs as.
    fn current_account() -> Option<String> {
        let mut len = 0u32;
        // SAFETY: a null buffer asks for the size (the call reports failure).
        unsafe {
            let _ = GetUserNameExW(NameSamCompatible, PWSTR::null(), &mut len);
        }
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let mut cap = buf.len() as u32;
        // SAFETY: `buf` holds `cap` UTF-16 units.
        let ok = unsafe { GetUserNameExW(NameSamCompatible, PWSTR(buf.as_mut_ptr()), &mut cap) };
        (ok.as_bool() || cap > 0).then(|| String::from_utf16_lossy(&buf[..cap as usize]))
    }

    fn own_token_user() -> Option<TokenUserBuf> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle of this process and an out handle.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
        let token = Handle(token);
        TokenUserBuf::of(token.0)
    }

    /// The packed credential with this user's name filled in, an empty
    /// password, to start the prompt on the right account.
    fn prefilled(account: &str) -> Option<Vec<u8>> {
        let user = wide_nul(account);
        let empty = [0u16];
        let mut size = 0u32;
        // SAFETY: a null buffer asks for the size.
        unsafe {
            let _ = CredPackAuthenticationBufferW(
                Default::default(),
                PCWSTR(user.as_ptr()),
                PCWSTR(empty.as_ptr()),
                None,
                &mut size,
            );
        }
        if size == 0 {
            return None;
        }
        let mut out = vec![0u8; size as usize];
        // SAFETY: `out` is `size` bytes.
        unsafe {
            CredPackAuthenticationBufferW(
                Default::default(),
                PCWSTR(user.as_ptr()),
                PCWSTR(empty.as_ptr()),
                Some(out.as_mut_ptr()),
                &mut size,
            )
        }
        .ok()?;
        Some(out)
    }

    enum Typed {
        Credential(Secret, String),
        Cancelled,
    }

    /// Shows the prompt once. `auth_error` names why the last try failed.
    fn prompt(reason: &str, input: &[u8], auth_error: u32) -> Result<Typed, AuthError> {
        let caption = HSTRING::from("Cua Spaces");
        let message = HSTRING::from(reason);
        let info = CREDUI_INFOW {
            cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
            // SAFETY: no arguments; a null window is allowed.
            hwndParent: unsafe { GetForegroundWindow() },
            pszMessageText: PCWSTR(message.as_ptr()),
            pszCaptionText: PCWSTR(caption.as_ptr()),
            hbmBanner: Default::default(),
        };
        let mut package = 0u32;
        let mut out: *mut c_void = std::ptr::null_mut();
        let mut out_len = 0u32;
        // SAFETY: every pointer is to a live local; `input` is a packed buffer.
        let rc = unsafe {
            CredUIPromptForWindowsCredentialsW(
                Some(&info),
                auth_error,
                &mut package,
                Some(input.as_ptr().cast()),
                input.len() as u32,
                &mut out,
                &mut out_len,
                None,
                CREDUIWIN_GENERIC,
            )
        };
        let buffer = AuthBuffer(out, out_len);
        if rc == ERROR_CANCELLED.0 {
            return Ok(Typed::Cancelled);
        }
        if rc != ERROR_SUCCESS.0 {
            return Err(AuthError::Failed(format!(
                "the Windows credential prompt failed ({rc})"
            )));
        }
        let (mut user, mut domain) = (vec![0u16; 513], vec![0u16; 257]);
        let mut pass = Secret(vec![0u16; 513]);
        let (mut ul, mut dl, mut pl) =
            (user.len() as u32, domain.len() as u32, pass.0.len() as u32);
        // SAFETY: each buffer is as long as the length given beside it.
        let unpacked = unsafe {
            CredUnPackAuthenticationBufferW(
                Default::default(),
                buffer.0,
                buffer.1,
                PWSTR(user.as_mut_ptr()),
                &mut ul,
                PWSTR(domain.as_mut_ptr()),
                Some(&mut dl),
                PWSTR(pass.0.as_mut_ptr()),
                &mut pl,
            )
        };
        unpacked.map_err(|_| AuthError::Failed("the typed credential could not be read".into()))?;
        let name =
            String::from_utf16_lossy(&user[..(ul as usize).saturating_sub(1).min(user.len())]);
        pass.0.truncate((pl as usize).saturating_sub(1));
        Ok(Typed::Credential(pass, name))
    }

    /// Whether `LogonUserW` takes this password, and for which SID.
    fn logon(name: &str, password: &Secret, mine: &TokenUserBuf) -> (bool, bool) {
        let (user, domain) = split_account(name);
        let user = wide_nul(&user);
        let domain = domain.map(|d| wide_nul(&d));
        let mut pw = password.0.clone();
        pw.push(0);
        let pw = Secret(pw);
        let mut token = HANDLE::default();
        // SAFETY: NUL-terminated strings that outlive the call, and an out handle.
        let ok = unsafe {
            LogonUserW(
                PCWSTR(user.as_ptr()),
                domain
                    .as_ref()
                    .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
                PCWSTR(pw.0.as_ptr()),
                LOGON32_LOGON_INTERACTIVE,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        };
        if ok.is_err() {
            return (false, false);
        }
        let token = Handle(token);
        let same = TokenUserBuf::of(token.0)
            // SAFETY: both SIDs live in their buffers.
            .is_some_and(|theirs| unsafe { EqualSid(mine.sid(), theirs.sid()) }.is_ok());
        (true, same)
    }

    pub(super) fn authorize(reason: &str) -> Result<(), AuthError> {
        let (Some(account), Some(mine)) = (current_account(), own_token_user()) else {
            return Err(AuthError::Unavailable);
        };
        let Some(input) = prefilled(&account) else {
            return Err(AuthError::Unavailable);
        };
        let mut auth_error = 0u32;
        for _ in 0..PASSWORD_ATTEMPTS {
            let (password, name) = match prompt(reason, &input, auth_error)? {
                Typed::Cancelled => return Err(AuthError::Cancelled),
                Typed::Credential(p, n) => (p, n),
            };
            let empty = password.0.is_empty();
            let (logon_ok, same) = if empty {
                (false, false)
            } else {
                logon(&name, &password, &mine)
            };
            drop(password);
            match password_verdict(empty, logon_ok, same) {
                PasswordVerdict::Confirmed => return Ok(()),
                PasswordVerdict::Refuse => return Err(AuthError::Denied),
                PasswordVerdict::Retry => auth_error = ERROR_LOGON_FAILURE.0,
            }
        }
        Err(AuthError::Denied)
    }
}

#[cfg(target_os = "linux")]
mod polkit {
    //! polkit through `pkcheck`, which asks the session's authentication agent.
    //! The process itself is spawned by [`crate::host`], the one place this
    //! crate touches the host.
    //!
    //! The prompt shows the action's own message
    //! (apps/cua-spaces-desktop/packaging/ai.cua.spaces.policy): polkit
    //! takes `--detail` only from root, and refuses the whole check otherwise.

    use std::io::ErrorKind;
    use std::time::Duration;

    use super::{AuthError, pkcheck_result, proc_start_time};
    use crate::host::{EffectKind, HostCommand, run_for_status};

    /// How long the user has to answer before the prompt counts as dismissed.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn authorize(action: &str, _reason: &str) -> Result<(), AuthError> {
        // This process, named the way polkit wants: id and start time.
        let start = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|stat| proc_start_time(&stat))
            .ok_or(AuthError::Unavailable)?;
        let check = HostCommand::new(EffectKind::UserAuthorization, "pkcheck")
            .args([
                "--action-id",
                action,
                "--allow-user-interaction",
                "--process",
            ])
            .arg(format!("{},{start}", std::process::id()))
            .timeout(REPLY_TIMEOUT);
        match run_for_status(&check) {
            // One diagnostic line at most on stderr.
            Ok((code, stderr)) => pkcheck_result(code, &stderr),
            // No pkcheck, or a test sandbox that refuses host effects.
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::PermissionDenied) => {
                Err(AuthError::Unavailable)
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => Err(AuthError::Cancelled),
            Err(e) => Err(AuthError::Failed(format!("cannot run pkcheck: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(rel: &str, sensitive: bool) -> ManifestItem {
        ManifestItem {
            label: rel.to_string(),
            rel_path: rel.to_string(),
            est_bytes: 0,
            count: None,
            count_noun: None,
            sensitive,
            default_checked: true,
        }
    }

    #[test]
    fn selection_none_is_sensitive_when_any_item_is() {
        let items = vec![item("a", false), item("secret", true)];
        // `None` selects everything, so the sensitive item counts.
        assert!(selection_is_sensitive(&items, None));
    }

    #[test]
    fn selection_only_nonsensitive_items_is_not_sensitive() {
        let items = vec![item("a", false), item("secret", true)];
        // Include only the non-sensitive item: no auth required.
        let include: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(!selection_is_sensitive(&items, Some(&include)));
    }

    #[test]
    fn selection_including_a_sensitive_item_is_sensitive() {
        let items = vec![item("a", false), item("secret", true)];
        let include: HashSet<String> = ["secret".to_string()].into_iter().collect();
        assert!(selection_is_sensitive(&items, Some(&include)));
    }

    #[test]
    fn empty_items_never_sensitive() {
        assert!(!selection_is_sensitive(&[], None));
    }

    #[test]
    fn skip_bypasses_the_gate() {
        // The test/CI escape hatch must short-circuit before any platform work,
        // so this never raises a real prompt (including on macOS dev machines).
        assert!(authorize_unless_skipped(true, "test reason").is_ok());
    }

    #[test]
    fn skip_env_has_no_effect_in_a_released_build() {
        // Red-team E5: a released (non-debug) binary must have NO bypass. Even
        // with the env var set, the skip is not honored when debug_assertions
        // is off, so a shipped Cua always runs the real OS authorization gate.
        assert!(
            !skip_is_honored(false, true),
            "released build must ignore the bypass env"
        );
        assert!(!skip_is_honored(false, false));
        // In a debug/test build the escape hatch still works so tests never prompt.
        assert!(skip_is_honored(true, true));
        assert!(!skip_is_honored(true, false));
    }

    #[test]
    fn auth_error_displays_are_distinct() {
        assert_ne!(
            AuthError::Denied.to_string(),
            AuthError::Unavailable.to_string()
        );
        assert_ne!(
            AuthError::Denied.to_string(),
            AuthError::Cancelled.to_string()
        );
    }

    // Fail-closed behavior for non-macOS hosts. Tested via the pure decision
    // function with explicit inputs so there is no dependency on (or race over)
    // process-global env vars.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_fails_closed_without_override() {
        assert_eq!(
            non_macos_decision(false, "reason"),
            Err(AuthError::Unavailable)
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_override_allows_export() {
        assert_eq!(non_macos_decision(true, "reason"), Ok(()));
    }

    #[test]
    fn typed_names_split_into_user_and_domain() {
        assert_eq!(
            split_account("WIN\\alice"),
            ("alice".into(), Some("WIN".into()))
        );
        assert_eq!(
            split_account(".\\alice"),
            ("alice".into(), Some(".".into()))
        );
        assert_eq!(split_account("\\alice"), ("alice".into(), Some(".".into())));
        // A UPN goes whole, with no domain.
        assert_eq!(
            split_account("alice@corp.example"),
            ("alice@corp.example".into(), None)
        );
        assert_eq!(split_account("alice"), ("alice".into(), Some(".".into())));
    }

    #[test]
    fn only_the_right_password_for_this_account_confirms() {
        use PasswordVerdict::*;
        // Blank proves nothing, even if the system would accept it.
        assert_eq!(password_verdict(true, true, true), Retry);
        // Wrong password.
        assert_eq!(password_verdict(false, false, false), Retry);
        assert_eq!(password_verdict(false, false, true), Retry);
        // Right password for another account (an administrator's): refused.
        assert_eq!(password_verdict(false, true, false), Refuse);
        assert_eq!(password_verdict(false, true, true), Confirmed);
        const { assert!(PASSWORD_ATTEMPTS <= 3) };
    }

    #[test]
    fn the_unavailable_message_does_not_advertise_the_override() {
        let m = AuthError::Unavailable.to_string();
        assert!(
            !m.contains(ALLOW_UNVERIFIED_ENV) && !m.contains("CUA_ENV"),
            "{m}"
        );
    }

    #[test]
    fn windows_hello_results_map_to_the_gate() {
        assert_eq!(hello_result(0), Ok(()));
        for unavailable in [1, 2, 3] {
            assert_eq!(
                hello_result(unavailable),
                Err(AuthError::Unavailable),
                "{unavailable}"
            );
        }
        assert_eq!(hello_result(6), Err(AuthError::Cancelled));
        for denied in [4, 5, 99] {
            assert_eq!(hello_result(denied), Err(AuthError::Denied), "{denied}");
        }
    }

    #[test]
    fn pkcheck_exit_statuses_map_to_the_gate() {
        assert_eq!(pkcheck_result(Some(0), ""), Ok(()));
        assert_eq!(pkcheck_result(Some(1), ""), Err(AuthError::Denied));
        // No authentication agent in the session: nobody could be asked.
        assert_eq!(pkcheck_result(Some(2), ""), Err(AuthError::Unavailable));
        assert_eq!(pkcheck_result(Some(3), ""), Err(AuthError::Cancelled));
    }

    #[test]
    fn a_pkcheck_error_is_an_error_not_a_refusal() {
        // What polkit answers a non-root caller that passes --detail.
        let refused = "Error checking for authorization ai.cua.spaces.keyvault: GDBus.Error:org.freedesktop.PolicyKit1.Error.NotAuthorized: Only trusted callers (e.g. uid 0 or an action owner) can use CheckAuthorization() and pass details\n";
        let Err(AuthError::Failed(detail)) = pkcheck_result(Some(127), refused) else {
            panic!("127 is an error");
        };
        assert!(
            detail.starts_with("Error checking for authorization"),
            "{detail}"
        );
        assert!(!detail.contains('\n'));
        assert_eq!(
            pkcheck_result(Some(127), ""),
            Err(AuthError::Failed("pkcheck exited with status 127".into()))
        );
        assert_eq!(
            pkcheck_result(None, ""),
            Err(AuthError::Failed("pkcheck was stopped".into()))
        );
        assert!(matches!(
            pkcheck_result(Some(42), ""),
            Err(AuthError::Failed(_))
        ));
        // No policy installed (an AppImage), as polkit 124 says it.
        assert_eq!(
            pkcheck_result(
                Some(127),
                "Error checking for authorization ai.cua.spaces.keyvault: GDBus.Error:org.freedesktop.PolicyKit1.Error.Failed: Action ai.cua.spaces.keyvault is not registered\n"
            ),
            Err(AuthError::Unavailable)
        );
        assert_ne!(
            AuthError::Failed("x".into()).to_string(),
            AuthError::Denied.to_string()
        );
    }

    /// CUA-1855: on Linux a refusal and a missing prompt say what to do.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_errors_name_the_next_step() {
        let denied = AuthError::Denied.to_string();
        assert!(denied.contains("If no password prompt appeared") && denied.contains(".deb"));
        let unavailable = AuthError::Unavailable.to_string();
        assert!(
            unavailable.contains(".deb") && unavailable.contains("polkit authentication agent")
        );
    }

    #[test]
    fn the_start_time_is_the_22nd_field_even_with_an_odd_process_name() {
        let stat = "4242 (cua daemon) (x) S 1 4242 4242 0 -1 4194560 1000 0 0 0 5 3 0 0 20 0 8 0 987654 1000000 2000 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 2 0 0 0 0 0";
        assert_eq!(proc_start_time(stat), Some(987654));
        assert_eq!(proc_start_time("no name here"), None);
        assert_eq!(proc_start_time("1 (x) S 1"), None);
    }

    /// The policy file the `.deb` installs names the action the gate asks for.
    #[test]
    fn the_polkit_policy_names_the_action_the_gate_asks_for() {
        let policy = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../apps/cua-spaces-desktop/packaging/ai.cua.spaces.policy"
        ))
        .expect("the policy file the deb installs");
        assert!(policy.contains(&format!("<action id=\"{POLKIT_ACTION}\">")));
        assert!(policy.contains(&format!("<action id=\"{POLKIT_DEVICE_ACTION}\">")));
        // The user's own password or fingerprint, every time, in a session
        // they are at: a local desktop (active) or a remote desktop, which
        // polkit files under "any" (xrdp, VNC: CUA-1725). Never without
        // asking, and never from an inactive session.
        assert_eq!(
            policy
                .matches("<allow_active>auth_self</allow_active>")
                .count(),
            2
        );
        assert_eq!(
            policy.matches("<allow_any>auth_self</allow_any>").count(),
            2
        );
        assert_eq!(
            policy
                .matches("<allow_inactive>no</allow_inactive>")
                .count(),
            2
        );
        assert!(!policy.contains(">yes<") && !policy.contains("auth_self_keep"));
        // Only root may pass polkit details, so the messages carry no `$(...)`.
        assert!(
            !policy.contains("$("),
            "the prompt shows the message as written"
        );
    }
}
