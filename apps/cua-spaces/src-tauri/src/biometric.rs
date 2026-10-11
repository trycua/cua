// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS biometric (Touch ID / device passcode) gate.
//!
//! Used to authorize sensitive settings changes — currently the
//! unattended-teleport permissions, so they cannot be loosened without the
//! device owner present. Mirrors rcdp's app-session biometric gate.
//!
//! `CUA_SKIP_BIOMETRIC=1` bypasses the gate (tests only; never in shipping use).

/// Authorize a sensitive action with the OS. Returns `Ok(())` when the device
/// owner authenticates, else a human-readable error.
pub fn authorize(reason: &str) -> Result<(), String> {
    if std::env::var("CUA_SKIP_BIOMETRIC")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        macos::authorize(reason)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = reason;
        Err("biometric authorization is only available on macOS".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! `evaluatePolicy:localizedReason:reply:` is asynchronous — it invokes the
    //! completion block on a framework-private queue. We bridge back to a
    //! synchronous call by handing the block an `mpsc::Sender` and blocking on
    //! the receiver until the reply arrives; `LAContext` and the block are held
    //! on the stack across that wait so the evaluation is not cancelled.

    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    /// Upper bound on waiting for the user to answer before treating the prompt
    /// as cancelled, so a forgotten dialog cannot wedge the caller forever.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn authorize(reason: &str) -> Result<(), String> {
        // Touch ID OR device-passcode fallback (DeviceOwnerAuthentication): robust
        // where no biometric is enrolled but a passcode is set.
        let policy = LAPolicy::DeviceOwnerAuthentication;
        // SAFETY: `LAContext::new` returns a fresh, owned context.
        let context = unsafe { LAContext::new() };

        // Preflight: unevaluatable policy (no passcode / unsupported) is treated
        // as unavailable rather than raising a doomed prompt.
        // SAFETY: `context` is valid; the call only reads state.
        if unsafe { context.canEvaluatePolicy_error(policy) }.is_err() {
            return Err(
                "device authentication is not available (no passcode/biometric enrolled)".into(),
            );
        }

        let ns_reason = NSString::from_str(reason);
        let (tx, rx) = mpsc::channel::<Result<(), String>>();

        // The reply block runs on a private framework queue; `Sender` is `Send`,
        // so handing the result back over the channel is sound.
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let result = if success.as_bool() {
                Ok(())
            } else {
                // SAFETY: on failure a valid NSError is passed; guard null.
                let code = if error.is_null() {
                    0
                } else {
                    unsafe { (*error).code() }
                };
                Err(classify(code))
            };
            let _ = tx.send(result);
        });

        // SAFETY: arguments are valid; `reply`/`context` outlive the wait below.
        unsafe {
            context.evaluatePolicy_localizedReason_reply(policy, &ns_reason, &reply);
        }

        match rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(result) => result,
            Err(_) => Err("authentication timed out".into()),
        }
    }

    /// Map an `LAError` code to a message. User/OS cancellations are
    /// distinguished from an outright denial.
    fn classify(code: isize) -> String {
        // LAErrorUserCancel = -2, LAErrorSystemCancel = -4, LAErrorAppCancel = -9.
        match code {
            -2 | -4 | -9 => "authentication was cancelled".into(),
            _ => "authentication was denied".into(),
        }
    }
}
