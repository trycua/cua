// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The SDK side of the broker: find the installed Cua, or say it is needed.
//!
//! An app that embeds the cua SDK cannot teleport by itself. It reaches the
//! Keyvault in the installed Cua daemon; when there is none it gets
//! [`RequiresCuaApp`] with an install link, so the user installs Cua (which
//! owns the consent UI) instead of the app reading credentials itself.
//!
//! **Enterprise embedded mode** is a seam, not a feature: with the Cargo
//! feature `enterprise-embedded` compiled in, [`embedded_mode`] would accept
//! a license signed by Cua's pinned Ed25519 key. The key shipped here is a
//! placeholder that verifies nothing, and the SDK does not enable the
//! feature, so embedded teleport is off in every build.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use crate::ipc::{ConnectError, KeyvaultClient, ServerCheck};

/// Where users get Cua.
pub const INSTALL_URL: &str = "https://cua.ai/download";
/// Opens the Keyvault page of the installed Cua app.
pub const OPEN_URL: &str = "cua://keyvault";

/// Teleport needs the installed Cua app (or CLI daemon).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RequiresCuaApp {
    /// Cua is installed but its Keyvault is not running.
    pub installed: bool,
    /// Install link.
    pub install_url: String,
    /// Deep link that opens Cua's Keyvault page.
    pub open_url: String,
    /// Human message.
    pub message: String,
}

impl RequiresCuaApp {
    /// The error for "not installed" or "installed, not running".
    pub fn new(installed: bool, detail: &str) -> Self {
        let message = if installed {
            format!(
                "Teleport needs the Cua app running: open Cua ({OPEN_URL}) and try again. {detail}"
            )
        } else {
            format!(
                "Teleport requires the Cua app, which keeps your sessions in its Keyvault and asks you before anything moves. Install it from {INSTALL_URL}. {detail}"
            )
        };
        Self {
            installed,
            install_url: INSTALL_URL.into(),
            open_url: OPEN_URL.into(),
            message: message.trim().to_string(),
        }
    }
}

/// Places the Cua app or CLI is installed.
pub fn install_locations(home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/Applications/Cua Spaces.app"),
        PathBuf::from("/Applications/Cua.app"),
        PathBuf::from("/usr/local/bin/cua"),
        PathBuf::from("/opt/homebrew/bin/cua"),
    ];
    if let Some(h) = home {
        v.push(h.join("Applications/Cua Spaces.app"));
        v.push(h.join(".local/bin/cua"));
        v.push(h.join(".cua/bin/cua"));
    }
    v
}

/// Whether Cua looks installed (a read-only filesystem probe).
pub fn cua_installed(locations: &[PathBuf]) -> bool {
    locations.iter().any(|p| p.exists())
}

/// Connects to the Keyvault at `socket`, or explains what the user needs.
/// An impostor on the socket is reported, never talked to.
#[cfg(unix)]
pub async fn connect_or_require_app(
    socket: Option<PathBuf>,
    check: ServerCheck,
    locations: &[PathBuf],
) -> Result<KeyvaultClient, RequiresCuaApp> {
    let Some(path) = socket.or_else(crate::default_socket) else {
        return Err(RequiresCuaApp::new(
            cua_installed(locations),
            "HOME is not set.",
        ));
    };
    match KeyvaultClient::connect(&path, check).await {
        Ok(c) => Ok(c),
        Err(ConnectError::NotRunning(_)) => Err(RequiresCuaApp::new(cua_installed(locations), "")),
        Err(ConnectError::Impostor { who, .. }) => Err(RequiresCuaApp::new(
            cua_installed(locations),
            &format!(
                "The process holding the Keyvault socket is not Cua ({who}); it was not trusted."
            ),
        )),
        Err(ConnectError::Other(e)) => Err(RequiresCuaApp::new(cua_installed(locations), &e)),
    }
}

/// Whether this build can teleport without the Cua app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbeddedMode {
    /// Not compiled in (every public build).
    NotAvailable,
    /// Compiled in, but no valid license.
    LicenseRequired(String),
    /// Licensed for this organization.
    Licensed {
        /// Organization.
        org: String,
    },
}

/// An enterprise license (the seam's input). Red-team F13: it binds to the OS
/// *verified* identity of the calling app (team id + signing identifier), to a
/// set of device ids, and carries a serial so it can be revoked; it is not an
/// unbound bearer file keyed to a self-declared bundle id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnterpriseLicense {
    /// License serial, for the revocation list.
    pub serial: String,
    /// Organization.
    pub org: String,
    /// The Apple team id the embedding app must be signed by (checked against
    /// the OS code signature, not a self-declared value).
    pub team_id: String,
    /// Signing identifiers allowed to embed (checked against the OS-verified
    /// signing identifier of the caller).
    pub identifiers: Vec<String>,
    /// Hardware/device ids this license is bound to (device binding). A license
    /// leaked to another machine does not match.
    pub device_ids: Vec<String>,
    /// Expiry, Unix ms. Enterprise licenses are short-lived and re-issued.
    pub not_after_ms: u64,
    /// Base64url Ed25519 signature over the canonical body.
    pub signature: String,
}

impl EnterpriseLicense {
    /// The canonical body the signature covers (every bound field). Only used
    /// when the enterprise seam is compiled in.
    #[cfg_attr(not(feature = "enterprise-embedded"), allow(dead_code))]
    fn signed_body(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.serial,
            self.org,
            self.team_id,
            self.identifiers.join(","),
            self.device_ids.join(","),
            self.not_after_ms
        )
    }
}

/// Cua's pinned license-signing key. A placeholder: no signature verifies
/// against it, so even a build with the feature stays off until Cua ships a
/// real key through a reviewed change.
pub const LICENSE_PUBLIC_KEY: [u8; 32] = [0u8; 32];

/// The embedded mode for `license`, evaluated against the OS-verified identity
/// of the calling app (`caller`), this machine's `device_id`, and a revocation
/// predicate `is_revoked` (a CRL or an online check). Always
/// [`EmbeddedMode::NotAvailable`] unless the `enterprise-embedded` feature is
/// compiled in. Even then it fails closed on any unmet binding (red-team F13).
pub fn embedded_mode(
    license: Option<&EnterpriseLicense>,
    caller: &crate::caller::CallerIdentity,
    device_id: &str,
    is_revoked: &dyn Fn(&str) -> bool,
) -> EmbeddedMode {
    #[cfg(not(feature = "enterprise-embedded"))]
    {
        let _ = (license, caller, device_id, is_revoked);
        EmbeddedMode::NotAvailable
    }
    #[cfg(feature = "enterprise-embedded")]
    {
        use crate::caller::Signing;
        let Some(l) = license else {
            return EmbeddedMode::LicenseRequired("no license".into());
        };
        if crate::now_ms() >= l.not_after_ms {
            return EmbeddedMode::LicenseRequired("the license expired".into());
        }
        if is_revoked(&l.serial) {
            return EmbeddedMode::LicenseRequired("the license was revoked".into());
        }
        // The embedding app's identity comes from the OS code signature, never
        // a value it declares (red-team F13). Ad hoc, unsigned and unverified
        // callers are refused outright.
        let (team, identifier) = match &caller.signing {
            Signing::Signed {
                team_id,
                identifier,
                ..
            } if caller.is_verified() => (team_id.as_str(), identifier.as_str()),
            _ => {
                return EmbeddedMode::LicenseRequired(
                    "the embedding app is not team-signed and OS-verified".into(),
                );
            }
        };
        if team != l.team_id || !l.identifiers.iter().any(|i| i == identifier) {
            return EmbeddedMode::LicenseRequired(format!(
                "{identifier} (team {team}) is not licensed by this org"
            ));
        }
        // Device binding: a leaked license does not embed on another machine.
        if !l.device_ids.iter().any(|d| d == device_id) {
            return EmbeddedMode::LicenseRequired("this device is not licensed".into());
        }
        let sig = match crate::crypto::b64url_decode(&l.signature) {
            Ok(s) => s,
            Err(_) => return EmbeddedMode::LicenseRequired("malformed signature".into()),
        };
        let key =
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, LICENSE_PUBLIC_KEY);
        match key.verify(l.signed_body().as_bytes(), &sig) {
            Ok(()) => EmbeddedMode::Licensed { org: l.org.clone() },
            Err(_) => EmbeddedMode::LicenseRequired("the license signature does not verify".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_license() -> EnterpriseLicense {
        EnterpriseLicense {
            serial: "LIC-1".into(),
            org: "acme".into(),
            team_id: "ACME000000".into(),
            identifiers: vec!["com.acme.bot".into()],
            device_ids: vec!["device-abc".into()],
            not_after_ms: u64::MAX,
            signature: "AAAA".into(),
        }
    }

    fn licensed_caller() -> crate::caller::CallerIdentity {
        use crate::caller::{CallerIdentity, Signing};
        CallerIdentity {
            signing: Signing::Signed {
                team_id: "ACME000000".into(),
                identifier: "com.acme.bot".into(),
                cdhash: "00".into(),
            },
            ..CallerIdentity::for_tests("com.acme.bot", false)
        }
    }

    #[test]
    fn embedded_mode_is_off_in_default_builds() {
        let l = sample_license();
        let never = |_: &str| false;
        let m = embedded_mode(Some(&l), &licensed_caller(), "device-abc", &never);
        #[cfg(not(feature = "enterprise-embedded"))]
        assert_eq!(m, EmbeddedMode::NotAvailable);
        // Even with the feature it fails closed (the placeholder key verifies
        // nothing), and it must never be `Licensed` here.
        #[cfg(feature = "enterprise-embedded")]
        assert!(matches!(m, EmbeddedMode::LicenseRequired(_)));
    }

    /// Red-team F13: the seam binds to the OS-verified identity, a device id and
    /// revocation. These bindings are checked *before* the (placeholder)
    /// signature, so exercising them does not need a real signing key.
    #[cfg(feature = "enterprise-embedded")]
    #[test]
    fn embedded_seam_enforces_identity_device_and_revocation() {
        use crate::caller::{CallerIdentity, Signing};
        let l = sample_license();
        let never = |_: &str| false;
        let refused = |m: &EmbeddedMode| matches!(m, EmbeddedMode::LicenseRequired(_));

        // A self-declared bundle id cannot stand in for the OS signature: an
        // app signed by a different team is refused even if it claims the id.
        let wrong_team = CallerIdentity {
            signing: Signing::Signed {
                team_id: "EVIL000000".into(),
                identifier: "com.acme.bot".into(),
                cdhash: "00".into(),
            },
            ..CallerIdentity::for_tests("com.acme.bot", false)
        };
        assert!(refused(&embedded_mode(
            Some(&l),
            &wrong_team,
            "device-abc",
            &never
        )));

        // Unsigned / unverified callers are refused.
        let unsigned = CallerIdentity {
            signing: Signing::Unsigned,
            ..CallerIdentity::for_tests("x", false)
        };
        assert!(refused(&embedded_mode(
            Some(&l),
            &unsigned,
            "device-abc",
            &never
        )));

        // A leaked license on a different device is refused (device binding).
        assert!(refused(&embedded_mode(
            Some(&l),
            &licensed_caller(),
            "device-other",
            &never
        )));

        // A revoked serial is refused even before the signature check.
        let revoked = |s: &str| s == "LIC-1";
        assert!(refused(&embedded_mode(
            Some(&l),
            &licensed_caller(),
            "device-abc",
            &revoked
        )));
    }

    #[test]
    fn requires_cua_app_messages_carry_links() {
        let e = RequiresCuaApp::new(false, "");
        assert!(!e.installed && e.message.contains(INSTALL_URL));
        let e = RequiresCuaApp::new(true, "");
        assert!(e.installed && e.message.contains(OPEN_URL));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn no_socket_means_requires_cua_app() {
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("keyvault.sock");
        let err = connect_or_require_app(Some(sock.clone()), ServerCheck::Unverified, &[])
            .await
            .err()
            .unwrap();
        assert!(!err.installed);
        assert_eq!(err.install_url, INSTALL_URL);
        // "Installed" is detected from install locations.
        let fake_app = d.path().join("Cua Spaces.app");
        std::fs::create_dir(&fake_app).unwrap();
        let err = connect_or_require_app(Some(sock), ServerCheck::Unverified, &[fake_app])
            .await
            .err()
            .unwrap();
        assert!(err.installed);
    }
}
