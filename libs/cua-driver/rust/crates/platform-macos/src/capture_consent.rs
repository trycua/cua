// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS's one-time direct screen-capture consent (CUA-1792).
//!
//! From macOS 15, the first time an app captures the screen without the
//! system picker, `replayd` shows "<App> is requesting to bypass the system
//! private window picker and directly access your screen and audio". The
//! alert is a `UserNotificationCenter` window: it takes the front and key
//! focus from whatever the user is typing in.
//!
//! Measured on macOS 26.5.2 in a fresh VM with Screen Recording granted:
//! - Every capture path raises it: `SCScreenshotManager`,
//!   `SLSHWCaptureWindowList`, `CGWindowListCreateImage` and the
//!   `screencapture` CLI. Switching APIs does not avoid it.
//! - Listing `SCShareableContent` does not raise it.
//! - `replayd` records the app in
//!   `~/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist`,
//!   keyed by the app bundle's file URL, as soon as the alert is shown. With
//!   an entry present, later captures (also after the entry's hint date)
//!   raise nothing. An entry keyed by the bundle identifier also counts:
//!   that is how a managed or pre-provisioned machine (and the hosted CI
//!   runner) records the approval ahead of time.
//!
//! So the driver surfaces the consent in `cua-driver permissions grant`,
//! where the user expects system prompts (see `check_permissions`), and
//! skips implicit captures while no entry exists, rather than letting the
//! alert pop over the user's window in the middle of background work.
//!
//! A capture that `permissions grant` verified for this bundle also counts as
//! consent when the grant recorded that its probe frame raised the alert for
//! the bundle itself and macOS holds this daemon responsible for itself. On
//! macOS 26.6 a daemon can find no entry for the bundle after that grant
//! (#6831), so the approvals file alone would block every capture the user
//! just allowed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::permission_observation::DirectCaptureEvidenceStore;

/// Machine-readable code carried in the capture error.
pub const CONSENT_REQUIRED_CODE: &str = "direct_capture_consent_required";

/// Set to `1` to capture anyway and let macOS show its alert (for unattended
/// machines where nobody is at the screen).
pub const ALLOW_ALERT_ENV: &str = "CUA_DRIVER_ALLOW_CAPTURE_CONSENT_ALERT";

const APPROVALS_RELATIVE_PATH: &str =
    "Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist";

/// How long a "no consent yet" answer is reused before the file is read again.
const NEGATIVE_RECHECK: Duration = Duration::from_secs(2);

/// Error when an implicit capture would raise the consent alert, `None` when
/// the capture may run.
pub fn capture_blocked_reason() -> Option<String> {
    static RECORDED: AtomicBool = AtomicBool::new(false);
    static LAST_MISS: Mutex<Option<Instant>> = Mutex::new(None);

    if RECORDED.load(Ordering::Relaxed) {
        return None;
    }
    if std::env::var(ALLOW_ALERT_ENV).is_ok_and(|value| value == "1") {
        return None;
    }
    let Some(bundle) = own_app_bundle() else {
        // A bare binary is not the responsible app; its alert names the
        // terminal or host, which this check cannot look up.
        return None;
    };
    if !macos_has_direct_capture_consent() {
        RECORDED.store(true, Ordering::Relaxed);
        return None;
    }
    if let Ok(last) = LAST_MISS.lock() {
        if last.is_some_and(|at| at.elapsed() < NEGATIVE_RECHECK) {
            return Some(blocked_message(&bundle));
        }
    }
    let home = std::env::var_os("HOME")?;
    let home = Path::new(&home);
    let bundle_id = crate::tools::main_bundle_identifier();
    let evidence = crate::tools::driver_daemon_direct_capture_evidence_store();
    match consent_state(
        &home.join(APPROVALS_RELATIVE_PATH),
        &bundle,
        bundle_id.as_deref(),
        evidence.as_ref(),
    ) {
        ConsentState::Recorded | ConsentState::Unknown => {
            RECORDED.store(true, Ordering::Relaxed);
            None
        }
        ConsentState::Missing => {
            if let Ok(mut last) = LAST_MISS.lock() {
                *last = Some(Instant::now());
            }
            Some(blocked_message(&bundle))
        }
    }
}

/// Whether a capture is known not to raise the consent alert: this macOS is
/// older than 15, which has no such alert, or it recorded this app bundle's
/// approval. Unlike [`capture_blocked_reason`], an answer it cannot work out
/// (an unknown macOS version, a bare binary, an unreadable approvals file,
/// the alert override) is no, for optional captures the user never asked for.
pub fn direct_capture_consented() -> bool {
    static CONSENTED: AtomicBool = AtomicBool::new(false);

    if CONSENTED.load(Ordering::Relaxed) {
        return true;
    }
    let before_alert = crate::tools::macos_product_version()
        .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major < 15);
    let consented = before_alert
        || own_app_bundle().is_some_and(|bundle| {
            std::env::var_os("HOME").is_some_and(|home| {
                consent_state(
                    &Path::new(&home).join(APPROVALS_RELATIVE_PATH),
                    &bundle,
                    crate::tools::main_bundle_identifier().as_deref(),
                    crate::tools::driver_daemon_direct_capture_evidence_store().as_ref(),
                ) == ConsentState::Recorded
            })
        });
    if consented {
        CONSENTED.store(true, Ordering::Relaxed);
    }
    consented
}

fn blocked_message(bundle: &Path) -> String {
    format!(
        "{CONSENT_REQUIRED_CODE}: screen capture skipped. macOS has not yet asked whether {} \
         may capture the screen directly, and the first capture would show that system alert \
         over the user's window and take their keyboard focus. Ask the user to run \
         `cua-driver permissions grant` and choose Allow once; captures then work. The \
         accessibility tree is unaffected. Set {ALLOW_ALERT_ENV}=1 on the daemon to capture \
         anyway on a machine where nobody is at the screen.",
        bundle
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Cua Driver")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsentState {
    Recorded,
    Missing,
    /// The file exists but could not be read or parsed: do not block on a
    /// format this code does not understand.
    Unknown,
}

fn consent_state(
    approvals: &Path,
    bundle: &Path,
    bundle_id: Option<&str>,
    evidence: Option<&DirectCaptureEvidenceStore>,
) -> ConsentState {
    if evidence.is_some_and(DirectCaptureEvidenceStore::records_consent_alert) {
        return ConsentState::Recorded;
    }
    let bytes = match std::fs::read(approvals) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ConsentState::Missing,
        Err(_) => return ConsentState::Unknown,
    };
    match approval_keys(&bytes) {
        Some(keys) if keys.iter().any(|key| key_names_app(key, bundle, bundle_id)) => {
            ConsentState::Recorded
        }
        Some(_) => ConsentState::Missing,
        None => ConsentState::Unknown,
    }
}

/// Top-level keys of the approvals property list (binary or XML).
fn approval_keys(bytes: &[u8]) -> Option<Vec<String>> {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::propertylist::{
        create_with_data, kCFPropertyListImmutable, CFPropertyList,
    };
    use core_foundation::string::CFString;

    let (plist, _) = create_with_data(CFData::from_buffer(bytes), kCFPropertyListImmutable).ok()?;
    // SAFETY: `create_with_data` returns a +1 property list on success.
    let plist = unsafe { CFPropertyList::wrap_under_create_rule(plist.cast()) };
    let dictionary = plist.downcast_into::<CFDictionary>()?;
    let (keys, _) = dictionary.get_keys_and_values();
    Some(
        keys.into_iter()
            .filter_map(|key| {
                // SAFETY: `key` is a borrowed CF object owned by `dictionary`;
                // `wrap_under_get_rule` retains it for the CFString wrapper.
                let key = unsafe { core_foundation::base::CFType::wrap_under_get_rule(key) };
                key.downcast::<CFString>().map(|key| key.to_string())
            })
            .collect(),
    )
}

/// Whether an approvals key names this app: its bundle file URL, as replayd
/// writes it, or its bundle identifier, as a pre-provisioned approval does.
fn key_names_app(key: &str, bundle: &Path, bundle_id: Option<&str>) -> bool {
    bundle_id.is_some_and(|id| !id.is_empty() && key == id) || key_names_bundle(key, bundle)
}

/// Whether an approvals key (`file:///…/CuaDriver.app/`) names `bundle`.
fn key_names_bundle(key: &str, bundle: &Path) -> bool {
    let Some(path) = key.strip_prefix("file://") else {
        return false;
    };
    let Some(decoded) = percent_decode(path) else {
        return false;
    };
    let decoded = decoded.trim_end_matches('/');
    let wanted = bundle.to_string_lossy();
    if decoded == wanted.trim_end_matches('/') {
        return true;
    }
    std::fs::canonicalize(decoded).is_ok_and(|resolved| resolved == bundle)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The `.app` bundle this process runs from, canonicalized.
fn own_app_bundle() -> Option<PathBuf> {
    let executable = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    app_bundle_of(&executable)
}

fn app_bundle_of(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    is_bundle.then(|| bundle.to_path_buf())
}

/// macOS 15 introduced the direct-capture consent.
pub(crate) fn macos_has_direct_capture_consent() -> bool {
    crate::tools::macos_product_version()
        .and_then(|version| version.split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_bundle_is_found_only_for_bundled_executables() {
        assert_eq!(
            app_bundle_of(Path::new(
                "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"
            )),
            Some(PathBuf::from("/Applications/CuaDriver.app"))
        );
        assert_eq!(app_bundle_of(Path::new("/usr/local/bin/cua-driver")), None);
        assert_eq!(
            app_bundle_of(Path::new("/tmp/CuaDriver/Contents/MacOS/cua-driver")),
            None
        );
    }

    #[test]
    fn approval_key_matches_the_bundle_url() {
        let bundle = Path::new("/Users/lume/My Apps/CuaDriver.app");
        assert!(key_names_bundle(
            "file:///Users/lume/My%20Apps/CuaDriver.app/",
            bundle
        ));
        assert!(!key_names_bundle(
            "file:///Users/lume/Other/CuaDriver.app/",
            bundle
        ));
        assert!(!key_names_bundle(
            "/Users/lume/My Apps/CuaDriver.app",
            bundle
        ));
    }

    fn write_plist(path: &Path, xml_keys: &[&str]) {
        let entries: String = xml_keys
            .iter()
            .map(|key| {
                format!(
                    "<key>{key}</key><dict><key>kScreenCapturePrivacyHintPolicy</key>\
                     <integer>2592000</integer></dict>"
                )
            })
            .collect();
        std::fs::write(
            path,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\">\
                 <dict>{entries}</dict></plist>"
            ),
        )
        .unwrap();
    }

    #[test]
    fn consent_state_reads_the_approvals_file() {
        let dir = std::env::temp_dir().join(format!("cua-consent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ScreenCaptureApprovals.plist");
        let bundle = Path::new("/Applications/CuaDriver.app");
        let id = Some("com.trycua.driver");

        let _ = std::fs::remove_file(&file);
        assert_eq!(
            consent_state(&file, bundle, id, None),
            ConsentState::Missing
        );

        write_plist(&file, &["file:///Applications/Other.app/"]);
        assert_eq!(
            consent_state(&file, bundle, id, None),
            ConsentState::Missing
        );

        write_plist(&file, &["com.trycua.driver"]);
        assert_eq!(
            consent_state(&file, bundle, id, None),
            ConsentState::Recorded
        );
        assert_eq!(
            consent_state(&file, bundle, None, None),
            ConsentState::Missing
        );
        assert_eq!(
            consent_state(&file, bundle, Some("com.trycua.driver.local"), None),
            ConsentState::Missing
        );

        write_plist(
            &file,
            &[
                "file:///Applications/Other.app/",
                "file:///Applications/CuaDriver.app/",
            ],
        );
        assert_eq!(
            consent_state(&file, bundle, id, None),
            ConsentState::Recorded
        );

        std::fs::write(&file, b"not a plist").unwrap();
        assert_eq!(
            consent_state(&file, bundle, id, None),
            ConsentState::Unknown
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_verified_grant_counts_as_consent_without_an_approvals_entry() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = home.path().join("ScreenCaptureApprovals.plist");
        let bundle = Path::new("/Applications/CuaDriver.app");
        let id = Some("com.trycua.driver");
        let release = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver";
        let evidence_for = |executable: &str, attribution: Option<&str>| {
            crate::tools::consent_evidence_store_for_identity(
                executable,
                home.path(),
                attribution,
                true,
            )
        };
        let evidence =
            evidence_for(release, Some("driver-daemon")).expect("release evidence store");

        assert_eq!(
            consent_state(&file, bundle, id, Some(&evidence)),
            ConsentState::Missing
        );
        write_plist(&file, &["file:///Applications/Other.app/"]);
        assert_eq!(
            consent_state(&file, bundle, id, Some(&evidence)),
            ConsentState::Missing
        );

        // A grant on macOS 14, or by a process answering to another app,
        // never raised this bundle's alert.
        evidence
            .record_now(false)
            .expect("record grant verification");
        assert_eq!(
            consent_state(&file, bundle, id, Some(&evidence)),
            ConsentState::Missing
        );

        evidence
            .record_now(true)
            .expect("record grant verification");
        assert_eq!(
            consent_state(&file, bundle, id, Some(&evidence)),
            ConsentState::Recorded
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(
            consent_state(&file, bundle, id, Some(&evidence)),
            ConsentState::Recorded
        );

        let local = evidence_for(
            "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local",
            Some("driver-daemon"),
        )
        .expect("local evidence store");
        assert_eq!(
            consent_state(&file, bundle, id, Some(&local)),
            ConsentState::Missing
        );

        // Embedded in a host, or launched by a caller, the alert names that
        // app, so the driver's own grant does not count.
        for attribution in [Some("host"), Some("caller"), None] {
            assert_eq!(
                consent_state(
                    &file,
                    bundle,
                    id,
                    evidence_for(release, attribution).as_ref()
                ),
                ConsentState::Missing
            );
        }
        // A daemon macOS does not hold responsible for itself raises the
        // alert for whichever app it answers to.
        let reparented = crate::tools::consent_evidence_store_for_identity(
            release,
            home.path(),
            Some("driver-daemon"),
            false,
        );
        assert_eq!(
            consent_state(&file, bundle, id, reparented.as_ref()),
            ConsentState::Missing
        );
    }
}
