
use super::*;
use crate::installed_apps::InstalledApp;

fn app(name: &str, bundle_id: &str, launch_path: &str) -> InstalledApp {
    InstalledApp {
        name: name.to_owned(),
        bundle_id: bundle_id.to_owned(),
        launch_path: launch_path.to_owned(),
        last_used: None,
        startup_wm_class: None,
    }
}

fn fixture() -> Vec<InstalledApp> {
    vec![
        app("Galculator", "galculator", "galculator"),
        app(
            "Google Chrome",
            "google-chrome",
            "/usr/bin/google-chrome-stable",
        ),
        app("File Manager", "thunar", "thunar"),
        app(
            "File Manager Settings",
            "thunar-settings",
            "thunar-settings",
        ),
    ]
}

/// Process state letter from `/proc/<pid>/stat`, or `None` once the entry
/// is gone. `comm` may itself contain spaces and parentheses, so the state
/// is read after the final `)` rather than by splitting from the left.
fn proc_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().next()?.chars().next()
}

#[test]
fn launched_children_are_reaped_instead_of_lingering_as_zombies() {
    // A launched app that exits must not stay in the process table. The
    // driver never waits on what it launches, so without an explicit
    // reaper every launch leaked a pid slot for the daemon's lifetime and
    // left "does this pid exist" liveness checks reporting a terminated
    // app as still running.
    // `/bin/sh` rather than a richer coreutils binary: it is the one
    // executable POSIX and the Nix build sandbox both guarantee, and the
    // sandbox has no `/bin/true`.
    let pid = spawn_launch_command("/bin/sh -c exit", &[]).expect("/bin/sh should spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match proc_state(pid) {
            None => break,                    // reaped, entry gone
            Some(state) if state != 'Z' => {} // still running or exiting
            Some(_) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "pid {pid} was still a zombie after the reaper deadline"
                );
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pid {pid} was never reaped"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[test]
fn matches_exact_display_name_case_insensitively() {
    let apps = fixture();
    let hit = match_installed_app(&apps, "google chrome").expect("should match");
    assert_eq!(hit.bundle_id, "google-chrome");
}

#[test]
fn matches_desktop_file_id_and_exec_basename() {
    let apps = fixture();
    assert_eq!(
        match_installed_app(&apps, "galculator").unwrap().name,
        "Galculator"
    );
    assert_eq!(
        match_installed_app(&apps, "google-chrome-stable")
            .unwrap()
            .name,
        "Google Chrome"
    );
}

#[test]
fn matches_unambiguous_display_name_substring() {
    let apps = fixture();
    assert_eq!(
        match_installed_app(&apps, "chrome").unwrap().name,
        "Google Chrome"
    );
}

#[test]
fn refuses_ambiguous_substring_and_unknown_names() {
    let apps = fixture();
    // "file manager" is a substring of two entries — refuse to guess.
    // ("File Manager" itself still resolves via the exact-name rung.)
    assert!(match_installed_app(&apps, "file man").is_none());
    assert_eq!(
        match_installed_app(&apps, "file manager")
            .unwrap()
            .bundle_id,
        "thunar"
    );
    assert!(match_installed_app(&apps, "gnome-calculator").is_none());
    assert!(match_installed_app(&apps, "").is_none());
}
