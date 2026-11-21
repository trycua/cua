// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use super::packaged_installed_match;
use crate::win32::InstalledApp;

fn app(kind: &str, bundle_id: &str, launch_path: &str) -> InstalledApp {
    InstalledApp {
        name: bundle_id.to_owned(),
        bundle_id: bundle_id.to_owned(),
        kind: kind.to_owned(),
        launch_path: launch_path.to_owned(),
        last_used: None,
    }
}

const NOTEPAD: &str = "Microsoft.WindowsNotepad_8wekyb3d8bbwe";

fn installed() -> Vec<InstalledApp> {
    vec![
        app(
            "desktop",
            r"C:\Windows\System32\notepad.exe",
            r"C:\Windows\System32\notepad.exe",
        ),
        app(
            "uwp",
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe",
            r"shell:appsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
        ),
        app("uwp", NOTEPAD, &format!(r"shell:appsFolder\{NOTEPAD}!App")),
    ]
}

#[test]
fn running_store_app_matches_its_installed_entry_by_aumid() {
    let aumid = format!("{NOTEPAD}!App");
    assert_eq!(
        packaged_installed_match(Some(&aumid), &installed()),
        Some(2)
    );
}

#[test]
fn aumid_match_ignores_ascii_case() {
    let aumid = "microsoft.windowsnotepad_8wekyb3d8bbwe!app";
    assert_eq!(packaged_installed_match(Some(aumid), &installed()), Some(2));
}

#[test]
fn process_without_aumid_never_matches_a_store_entry() {
    assert_eq!(packaged_installed_match(None, &installed()), None);
}

#[test]
fn sibling_app_of_the_same_package_does_not_borrow_its_launch_path() {
    let aumid = format!("{NOTEPAD}!OtherApp");
    assert_eq!(packaged_installed_match(Some(&aumid), &installed()), None);
}

#[test]
fn desktop_entries_are_never_package_matches() {
    let apps = vec![app(
        "desktop",
        NOTEPAD,
        &format!(r"shell:appsFolder\{NOTEPAD}!App"),
    )];
    let aumid = format!("{NOTEPAD}!App");
    assert_eq!(packaged_installed_match(Some(&aumid), &apps), None);
}

#[test]
fn aumid_match_uses_the_manifest_application_id_not_app() {
    const PAINT: &str = "Contoso.Paint_8wekyb3d8bbwe";
    let apps = vec![app(
        "uwp",
        PAINT,
        &format!(r"shell:appsFolder\{PAINT}!Editor"),
    )];
    let aumid = format!("{PAINT}!Editor");
    assert_eq!(packaged_installed_match(Some(&aumid), &apps), Some(0));
}
