// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The owner check before a change only the person at this computer may
//! make (approving a device for the account), the system's own prompt the
//! daemon's Keyvault asks for too: on macOS LocalAuthentication (Touch ID,
//! an Apple Watch or the login password, as the SwiftUI app's
//! `LivePresence`), on Windows Windows Hello (face, fingerprint or PIN), on
//! Linux polkit's `ai.cua.spaces.devices` (the user's password or
//! fingerprint through the session's agent). The Electron app calls it from
//! its main process, so the prompt is for Cua Spaces.

use cua_sdk::support::run;
use cua_sdk::{CuaError, Result};
use cua_teleport::biometric::{AuthError, POLKIT_DEVICE_ACTION, confirm_user};

/// Asks the person at this computer to confirm `reason` ("approve
/// “Work laptop” for your Cua account"). Returns when they did; fails with
/// `PermissionDenied` when it was cancelled or not confirmed, or when this
/// Mac has neither Touch ID nor a login password, and with `Unsupported`
/// where the system has no usable prompt (Windows Hello not set up, no
/// polkit agent or action), in words that say what is missing.
#[uniffi::export]
pub async fn app_confirm_presence(reason: String) -> Result<()> {
    run(async move {
        tokio::task::spawn_blocking(move || confirm_user(POLKIT_DEVICE_ACTION, &reason))
            .await
            .map_err(|e| CuaError::Internal(e.to_string()))?
            .map_err(presence_error)
    })
    .await
}

/// The SwiftUI app's words for each outcome (`LivePresence`).
fn presence_error(e: AuthError) -> CuaError {
    match e {
        AuthError::Cancelled => CuaError::PermissionDenied("Approval was cancelled".into()),
        AuthError::Denied => CuaError::PermissionDenied("Approval was not confirmed".into()),
        AuthError::Unavailable if cfg!(target_os = "macos") => {
            CuaError::PermissionDenied("Touch ID and the login password are not available".into())
        }
        AuthError::Unavailable if cfg!(target_os = "windows") => CuaError::Unsupported(
            "Windows Hello is not set up (a PIN, fingerprint or face in Settings, Accounts, Sign-in options)".into(),
        ),
        AuthError::Unavailable if cfg!(target_os = "linux") => CuaError::Unsupported(
            "polkit cannot ask (no authentication agent in this session, or the ai.cua.spaces.devices action is not installed)".into(),
        ),
        AuthError::Unavailable => {
            CuaError::Unsupported("this system has no owner check for Cua Spaces".into())
        }
        // The prompt could not run (polkit answered an error): nobody was asked.
        AuthError::Failed(detail) => {
            CuaError::Internal(format!("The owner check could not run: {detail}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_read_as_the_swift_app_words_them() {
        let words = |e| match presence_error(e) {
            CuaError::PermissionDenied(m) | CuaError::Unsupported(m) => m,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(words(AuthError::Cancelled), "Approval was cancelled");
        assert_eq!(words(AuthError::Denied), "Approval was not confirmed");
        assert!(matches!(
            presence_error(AuthError::Failed("pkcheck exited with status 127".into())),
            CuaError::Internal(m) if m.ends_with("pkcheck exited with status 127")
        ));
        let unavailable = presence_error(AuthError::Unavailable);
        if cfg!(target_os = "macos") {
            assert!(matches!(unavailable, CuaError::PermissionDenied(_)));
        } else {
            assert!(matches!(unavailable, CuaError::Unsupported(_)));
        }
    }
}
