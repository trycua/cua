// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Integration test: import a Chrome bundle through the receiver.
//!
//! Hermetic by construction: every importer and the app launcher act on a
//! [`FakeHost`], and imports land in a temp directory. Nothing here may
//! probe, launch, kill or script a real app, or touch the Keychain or the
//! real `$HOME`. The bundle is written with the shared `BundleWriter` in the
//! sender's canonical layout; the driver never links the sender.

use std::path::Path;
use std::sync::Arc;

use cua_spacesd_teleport::bundle::BundleWriter;
use cua_spacesd_teleport::host::HostOutput;
use cua_spacesd_teleport::{EffectKind, FakeHost, ImportError, Receiver, TransferScope};

fn fake_receiver(dest: &Path) -> (Arc<FakeHost>, Receiver) {
    // Every host command "succeeds" (`open` returns 0), nothing real runs, and
    // no browser is running (a `pgrep` that matched would make the import wait
    // for it to quit).
    let host = Arc::new(FakeHost::new().with_responder(|c| {
        Ok(if c.kind == EffectKind::ProcessLookup {
            HostOutput::failed()
        } else {
            HostOutput::ok("501")
        })
    }));
    let receiver = Receiver::with_host(dest.to_path_buf(), host.clone());
    (host, receiver)
}

fn chrome_bundle() -> Vec<u8> {
    let mut writer = BundleWriter::new(
        Vec::new(),
        "chrome",
        "Google Chrome",
        TransferScope::FullProfile,
    );
    writer.add_bytes("tabs.json", 0o644, b"[]").unwrap();
    for (name, bytes) in [
        ("Cookies", &b"cookie-bytes"[..]),
        ("Preferences", b"{\"p\":1}"),
        ("Current Session", b"snss"),
    ] {
        writer
            .add_bytes(
                format!(".config/google-chrome/Default/{name}"),
                0o600,
                bytes,
            )
            .unwrap();
    }
    writer.finish().unwrap()
}

#[test]
fn import_lands_files_and_launches_through_the_fake_host() {
    let dest = tempfile::tempdir().unwrap();
    let (host, receiver) = fake_receiver(dest.path());
    let outcome = receiver
        .import_bytes(&chrome_bundle(), "chrome", true)
        .unwrap();
    assert_eq!(outcome.provider_id, "chrome");
    assert!(outcome.launched, "{:?}", outcome.launch_error);
    assert!(outcome.launch_error.is_none());
    // macOS opens the app through LaunchServices (no pid); elsewhere the
    // program is spawned.
    assert_eq!(
        outcome.pid,
        if cfg!(target_os = "macos") {
            None
        } else {
            Some(0)
        }
    );
    // The import remaps the Chrome profile to the DESTINATION platform's
    // user-data directory.
    let user_data_dir = if cfg!(target_os = "macos") {
        "Library/Application Support/Google/Chrome"
    } else if cfg!(windows) {
        "AppData/Local/Google/Chrome/User Data"
    } else {
        ".config/google-chrome"
    };
    let cookies = dest.path().join(user_data_dir).join("Default/Cookies");
    assert_eq!(
        std::fs::read(&cookies).unwrap_or_else(|e| panic!("{}: {e}", cookies.display())),
        b"cookie-bytes"
    );
    let launches = host.calls_of(EffectKind::AppLaunch);
    assert_eq!(launches.len(), 1, "{:?}", host.calls());
    if cfg!(target_os = "macos") {
        assert_eq!(launches[0].program, "/usr/bin/open");
        assert_eq!(
            launches[0].args[..2],
            ["-a", "/Applications/Google Chrome.app"]
        );
    }
    assert!(launches[0]
        .args
        .iter()
        .any(|a| a.starts_with("--user-data-dir=")));
}

/// A launch that fails is reported with its reason, not silently dropped, and
/// does not fail the import.
#[cfg(target_os = "macos")]
#[test]
fn a_failed_launch_is_reported_and_the_import_still_succeeds() {
    let dest = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::new().with_responder(|c| {
        Ok(if c.kind == EffectKind::AppLaunch {
            HostOutput {
                success: false,
                stdout: vec![],
                stderr: b"The application cannot be opened".to_vec(),
            }
        } else if c.kind == EffectKind::ProcessLookup {
            HostOutput::failed()
        } else {
            HostOutput::ok("501")
        })
    }));
    let receiver = Receiver::with_host(dest.path().to_path_buf(), host);
    let outcome = receiver
        .import_bytes(&chrome_bundle(), "chrome", true)
        .unwrap();
    assert!(!outcome.launched);
    let why = outcome.launch_error.expect("reason");
    assert!(why.contains("cannot be opened"), "{why}");
}

#[test]
fn import_without_launch_records_nothing() {
    let dest = tempfile::tempdir().unwrap();
    let (host, receiver) = fake_receiver(dest.path());
    let outcome = receiver.import_bytes(&chrome_bundle(), "", false).unwrap();
    assert!(!outcome.launched);
    // Only the look for a running Chrome (to quit before its profile is
    // replaced): nothing is launched, killed, or written to the Keychain.
    assert!(
        host.calls()
            .iter()
            .all(|c| c.kind == EffectKind::ProcessLookup),
        "{:?}",
        host.calls()
    );
}

#[test]
fn app_mismatch_unknown_provider_and_garbage_are_refused() {
    let dest = tempfile::tempdir().unwrap();
    let (host, receiver) = fake_receiver(dest.path());
    assert!(matches!(
        receiver.import_bytes(&chrome_bundle(), "firefox", true),
        Err(ImportError::AppMismatch { .. })
    ));
    assert!(matches!(
        receiver.import_bytes(b"not a tar", "", false),
        Err(ImportError::InvalidBundle(_))
    ));
    let unknown = BundleWriter::new(Vec::new(), "vscode", "VS Code", TransferScope::TabsOnly)
        .finish()
        .unwrap();
    assert!(matches!(
        receiver.import_bytes(&unknown, "", true),
        Err(ImportError::UnknownProvider(_))
    ));
    assert!(host.calls_of(EffectKind::AppLaunch).is_empty());
    // Nothing landed.
    assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
}
