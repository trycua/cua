use super::packaged_installed_match;
use crate::win32::{InstalledApp, PackageIdentity};

fn app(kind: &str, bundle_id: &str, launch_path: &str) -> InstalledApp {
    InstalledApp {
        name: bundle_id.to_owned(),
        bundle_id: bundle_id.to_owned(),
        kind: kind.to_owned(),
        launch_path: launch_path.to_owned(),
        last_used: None,
    }
}

fn package(family_name: Option<&str>, app_user_model_id: Option<&str>) -> PackageIdentity {
    PackageIdentity {
        family_name: family_name.map(str::to_owned),
        app_user_model_id: app_user_model_id.map(str::to_owned),
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
    let identity = package(Some(NOTEPAD), Some(&format!("{NOTEPAD}!App")));
    assert_eq!(packaged_installed_match(&identity, &installed()), Some(2));
}

#[test]
fn aumid_match_ignores_ascii_case() {
    let identity = package(
        Some(NOTEPAD),
        Some("microsoft.windowsnotepad_8wekyb3d8bbwe!app"),
    );
    assert_eq!(packaged_installed_match(&identity, &installed()), Some(2));
}

#[test]
fn unpackaged_process_never_matches_a_store_entry() {
    assert_eq!(
        packaged_installed_match(&PackageIdentity::default(), &installed()),
        None
    );
}

#[test]
fn sibling_app_of_the_same_package_does_not_borrow_its_launch_path() {
    let identity = package(Some(NOTEPAD), Some(&format!("{NOTEPAD}!OtherApp")));
    assert_eq!(packaged_installed_match(&identity, &installed()), None);
}

#[test]
fn packaged_helper_without_aumid_matches_its_only_family_entry() {
    let identity = package(Some(NOTEPAD), None);
    assert_eq!(packaged_installed_match(&identity, &installed()), Some(2));
}

#[test]
fn family_fallback_refuses_an_ambiguous_family() {
    let mut apps = installed();
    apps.push(app(
        "uwp",
        NOTEPAD,
        &format!(r"shell:appsFolder\{NOTEPAD}!Second"),
    ));
    let identity = package(Some(NOTEPAD), None);
    assert_eq!(packaged_installed_match(&identity, &apps), None);
}

#[test]
fn desktop_entries_are_never_package_matches() {
    let apps = vec![app(
        "desktop",
        NOTEPAD,
        &format!(r"shell:appsFolder\{NOTEPAD}!App"),
    )];
    let identity = package(Some(NOTEPAD), Some(&format!("{NOTEPAD}!App")));
    assert_eq!(packaged_installed_match(&identity, &apps), None);
}
