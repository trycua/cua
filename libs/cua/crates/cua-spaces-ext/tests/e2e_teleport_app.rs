// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live "Teleport an app…" into a real linux container. Run
//! through `libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh --test e2e_teleport_app` (the
//! container runs with `--memory=4g`); without `CUA_SPACES_E2E_URL` the test
//! skips.
//!
//! Host safety: the host app is a fixture bundle ("Visual Studio Code.app"
//! with only an Info.plist) in a temp directory, the files are a generated
//! project, the recents file is temporary, and the teleport sender runs on
//! a `FakeHost`. Nothing reads the real /Applications or any profile.
//!
//! What it proves, inside the Space: the catalog classifies the fixture as
//! install-only; the plan lists the pinned install and each host path; the
//! run installs VS Code from the pinned, sha256-verified manifest, sends
//! the files (sha256 checked by the guest's own sha256sum), and starts the
//! editor with them.

use cua_spaces::Spaces;
use cua_spaces_ext::teleport::AppSessions;
use cua_spaces_ext::teleport_app::SpaceAppTeleport as _;
use cua_spaces_ext::teleport_app::{Consent, PlanOptions, PlanStep, RunPhase};
use cua_teleport::ux::{self, Capability, CatalogRequest, MoveKind, TargetHint};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some((
        url,
        std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default(),
    ))
}

fn fixture_bundle(root: &std::path::Path) {
    let c = root.join("Visual Studio Code.app/Contents");
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(
        c.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.microsoft.VSCode</string>
<key>CFBundleName</key><string>Code</string>
<key>CFBundleShortVersionString</key><string>0.0.0-fixture</string></dict></plist>"#,
    )
    .unwrap();
}

#[tokio::test]
async fn e2e_teleport_a_fixture_app_with_files_into_the_space() {
    let Some((url, token)) = target() else {
        eprintln!(
            "skipped: set CUA_SPACES_E2E_URL (run libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh --test e2e_teleport_app)"
        );
        return;
    };
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .download_dir(reg.path().join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("e2e-teleport".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    let sessions = Arc::new(AppSessions::with_host(Arc::new(
        cua_spaces_ext::teleport::providers::FakeHost::new().with_home(reg.path()),
    )));

    // The catalog over a fixture app root, narrowed to this Space.
    let apps = tempfile::tempdir().unwrap();
    fixture_bundle(apps.path());
    let facts = space.app_teleport_facts().await.unwrap();
    eprintln!("space: {:?} {} home {}", facts.os, facts.arch, facts.home);
    let catalog = ux::catalog(
        sessions.registry(),
        &CatalogRequest {
            roots: Some(vec![apps.path().to_path_buf()]),
            hint: TargetHint {
                os: Some(facts.os),
                arch: Some(facts.arch.clone()),
            },
            recents_path: Some(reg.path().join("recents.json")),
            probe_providers: false,
        },
    )
    .unwrap();
    assert_eq!(catalog.len(), 1);
    let vscode = &catalog[0];
    assert_eq!(
        (vscode.id.as_str(), vscode.capability),
        ("vscode", Capability::InstallOnly)
    );

    // A generated project and a note.
    let src = tempfile::tempdir().unwrap();
    let proj = src.path().join("teleport-fixture-project");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    let main = b"fn main() { println!(\"teleported\"); }\n";
    std::fs::write(proj.join("src/main.rs"), main).unwrap();
    std::fs::write(proj.join("README.md"), b"fixture project\n").unwrap();
    let note = src.path().join("teleport-fixture-note.txt");
    std::fs::write(&note, b"hello from the host\n").unwrap();
    let mut options = PlanOptions::new(MoveKind::AppWithFiles);
    options.files = vec![
        proj.to_string_lossy().into_owned(),
        note.to_string_lossy().into_owned(),
    ];

    let plan = space
        .plan_app_teleport(sessions.clone(), vscode, &options)
        .await
        .unwrap();
    let steps: Vec<&str> = plan.steps.iter().map(|s| s.name()).collect();
    assert_eq!(steps, ["install", "files", "launch"]);
    assert_eq!(plan.consent.len(), 3, "{:#?}", plan.consent);
    assert!(!plan.sensitive);
    eprintln!("consent: {:#?}", plan.consent);

    let events = Arc::new(Mutex::new(vec![]));
    let ev = events.clone();
    // Wall-clock per event, so a slow phase shows up in the log.
    let started = std::time::Instant::now();
    let approved = plan
        .clone()
        .approve(Consent {
            approved: true,
            acknowledge_sensitive: false,
            save_to_keyvault: false,
            acknowledge_relay_plaintext: false,
            ..Default::default()
        })
        .unwrap();
    let report = tokio::time::timeout(
        Duration::from_secs(1200),
        space.run_app_teleport(sessions.clone(), &approved, move |e| {
            eprintln!(
                "[{:>7.2}s] {:?} {} {}",
                started.elapsed().as_secs_f64(),
                e.phase,
                e.kind,
                e.detail
            );
            ev.lock().unwrap().push(e);
        }),
    )
    .await
    .expect("the run finished in 20 minutes")
    .unwrap();
    let run_secs = started.elapsed().as_secs_f64();
    eprintln!("TIMING run_app_teleport {run_secs:.2}s");
    assert_eq!(report.installed, ["vscode"]);
    assert!(report.launched);
    {
        // Scope the guard so it is not held across the awaits below.
        let events = events.lock().unwrap();
        assert_eq!(events.last().unwrap().phase, RunPhase::Done);
        assert!(!events.iter().any(|e| e.phase == RunPhase::Failed));
    }

    // Inside the Space: the pinned install, the files (checked by the
    // guest's own sha256sum) and the running editor.
    let sh = |cmd: String| {
        let space = space.clone();
        async move {
            let out = space.bash(&cmd, Duration::from_secs(60)).await.unwrap();
            assert!(out.success(), "{cmd}: {}", out.render());
            out.stdout
        }
    };
    let version = cua_agents::installables::get("vscode")
        .unwrap()
        .version
        .clone();
    let marker = sh(format!(
        "cat \"$HOME/.cua/tools/vscode/{version}/.cua-installed\""
    ))
    .await;
    assert_eq!(marker.trim(), version);
    let dest = format!("{}/Downloads/Teleported", facts.home);
    let guest_main = sh(format!(
        "sha256sum '{dest}/teleport-fixture-project/src/main.rs' | cut -d' ' -f1"
    ))
    .await;
    assert_eq!(guest_main.trim(), hex::encode(Sha256::digest(main)));
    let guest_note = sh(format!("cat '{dest}/teleport-fixture-note.txt'")).await;
    assert_eq!(guest_note, "hello from the host\n");
    let PlanStep::Launch { files, .. } = plan.steps.last().unwrap() else {
        panic!("last step is launch")
    };
    assert_eq!(files[0], format!("{dest}/teleport-fixture-project"));
    // The editor process is up with the project (bounded wait).
    let mut running = String::new();
    for _ in 0..30 {
        let out = space
            .bash(
                "pgrep -af '[.]cua/tools/vscode' | grep -c 'teleport-fixture-projec[t]' || true",
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        running = out.stdout.trim().to_string();
        if running != "0" && !running.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(
        running.parse::<u32>().unwrap_or(0) > 0,
        "VS Code is running with the project"
    );
    // Its window appears on the Space's desktop (bounded wait).
    let is_code = |w: &cua_spaces::stream::WindowTarget| {
        w.title.contains("teleport-fixture")
            || w.title.contains("Visual Studio Code")
            || w.app_name.to_lowercase().contains("code")
    };
    let mut windows = vec![];
    let mut window_secs = None;
    for _ in 0..90 {
        windows = space.windows(None).await.unwrap_or_default();
        if window_secs.is_none() && windows.iter().any(is_code) {
            window_secs = Some(started.elapsed().as_secs_f64());
        }
        // The workbench has loaded once the title names the project.
        if windows.iter().any(|w| w.title.contains("teleport-fixture")) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let ready_secs = started.elapsed().as_secs_f64();
    eprintln!(
        "windows: {:?}",
        windows
            .iter()
            .map(|w| (w.app_name.clone(), w.title.clone()))
            .collect::<Vec<_>>()
    );
    assert!(windows.iter().any(is_code), "a VS Code window appeared");
    eprintln!(
        "TIMING run {run_secs:.2}s, first window {:.2}s, workbench with the project {ready_secs:.2}s",
        window_secs.unwrap_or(ready_secs)
    );
    assert!(
        windows.iter().any(|w| w.title.contains("teleport-fixture")),
        "VS Code opened the teleported project"
    );
    // A screenshot of the Space's desktop, through the SDK.
    if let Some(dir) = std::env::var_os("CUA_TELEPORT_E2E_EVIDENCE") {
        // Let the editor finish painting.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let shot = space
            .spacesd()
            .unwrap()
            .screenshot(cua_spacesd_client::ScreenshotOptions::default())
            .await
            .unwrap();
        let path = std::path::Path::new(&dir).join("teleport-vscode.png");
        std::fs::write(&path, &shot.image).unwrap();
        eprintln!(
            "screenshot {}x{} -> {}",
            shot.width,
            shot.height,
            path.display()
        );
    }
}

/// Live "Teleport an app…" into a real local macOS Space with "Move
/// signed-in state" unticked: Chrome (which a macOS Space must already
/// have) moves with its profile's default items (no cookies, passwords or
/// sign-ins), then opens. Opt-in: `CUA_SPACES_E2E_MACOS_URL` / `_TOKEN`
/// name the Space's cua-spacesd (the launch reliability suite's `teleport`
/// scenario sets them); without them the test skips.
///
/// Host safety: the host Chrome is a fixture bundle (Info.plist only) and a
/// synthetic profile in temp directories; the provider runs on a
/// `FakeHost` with DevTools and AppleScript off. Nothing reads
/// /Applications, the real Chrome profile or the Keychain.
#[tokio::test]
async fn e2e_teleport_chrome_without_sign_ins_into_a_macos_space() {
    use cua_spaces_ext::teleport::providers::{ExportRegistry, FakeHost};
    use cua_teleport::providers::chrome::ChromeProvider;
    let Ok(url) = std::env::var("CUA_SPACES_E2E_MACOS_URL") else {
        eprintln!(
            "skipped: set CUA_SPACES_E2E_MACOS_URL (the reliability suite's teleport scenario)"
        );
        return;
    };
    let token = std::env::var("CUA_SPACES_E2E_MACOS_TOKEN").unwrap_or_default();
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .download_dir(reg.path().join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("e2e-teleport-macos".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();

    // The host: a fixture Chrome bundle and a synthetic Default profile.
    let host_home = tempfile::tempdir().unwrap();
    let profile = host_home.path().join("chrome-profile/Default");
    std::fs::create_dir_all(&profile).unwrap();
    let marker = format!("cua-e2e-{}", std::process::id());
    std::fs::write(
        profile.join("Preferences"),
        format!(r#"{{"cua_e2e_marker":"{marker}","profile":{{"name":"Reliability"}}}}"#),
    )
    .unwrap();
    std::fs::write(
        profile.join("Bookmarks"),
        format!(
            r#"{{"roots":{{"bookmark_bar":{{"children":[{{"name":"{marker}","type":"url","url":"https://example.com/"}}],"name":"Bookmarks bar","type":"folder"}}}},"version":1}}"#
        ),
    )
    .unwrap();
    std::fs::write(profile.join("Cookies"), b"synthetic cookies, never sent").unwrap();
    let host = Arc::new(FakeHost::new().with_home(host_home.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        ChromeProvider::new()
            .with_host(host)
            .with_profile_dir(&profile)
            .without_devtools(),
    ));
    let sessions = Arc::new(AppSessions::from_registry(registry));
    let apps = tempfile::tempdir().unwrap();
    let c = apps.path().join("Google Chrome.app/Contents");
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(
        c.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.google.Chrome</string>
<key>CFBundleName</key><string>Google Chrome</string>
<key>CFBundleShortVersionString</key><string>0.0.0-fixture</string></dict></plist>"#,
    )
    .unwrap();

    let facts = space.app_teleport_facts().await.unwrap();
    eprintln!("space: {:?} {} home {}", facts.os, facts.arch, facts.home);
    let catalog = ux::catalog(
        sessions.registry(),
        &CatalogRequest {
            roots: Some(vec![apps.path().to_path_buf()]),
            hint: TargetHint {
                os: Some(facts.os),
                arch: Some(facts.arch.clone()),
            },
            recents_path: Some(reg.path().join("recents.json")),
            probe_providers: false,
        },
    )
    .unwrap();
    let chrome = catalog
        .iter()
        .find(|e| e.id == "chrome")
        .unwrap_or_else(|| panic!("no Chrome row: {catalog:#?}"));
    assert_eq!(chrome.capability, Capability::Full, "{chrome:#?}");

    // "Move signed-in state" unticked: no sensitive group, default items.
    let plan = space
        .plan_app_teleport(
            sessions.clone(),
            chrome,
            &PlanOptions::new(MoveKind::AppWithState),
        )
        .await
        .unwrap();
    eprintln!(
        "plan: {:?}",
        plan.steps.iter().map(|s| s.name()).collect::<Vec<_>>()
    );
    assert!(!plan.sensitive, "no secret moves: {:#?}", plan.consent);
    assert!(
        !plan.consent.iter().any(|c| c.key.contains("Cookies")),
        "{:#?}",
        plan.consent
    );
    let approved = plan
        .clone()
        .approve(Consent {
            approved: true,
            acknowledge_sensitive: false,
            save_to_keyvault: false,
            acknowledge_relay_plaintext: false,
            ..Default::default()
        })
        .unwrap();
    let started = std::time::Instant::now();
    let events = Arc::new(Mutex::new(vec![]));
    let ev = events.clone();
    let report = tokio::time::timeout(
        Duration::from_secs(600),
        space.run_app_teleport(sessions.clone(), &approved, move |e| {
            eprintln!(
                "[{:>7.2}s] {:?} {} {}",
                started.elapsed().as_secs_f64(),
                e.phase,
                e.kind,
                e.detail
            );
            ev.lock().unwrap().push(e);
        }),
    )
    .await
    .expect("the run finished in 10 minutes")
    .unwrap();
    eprintln!(
        "TIMING run_app_teleport {:.2}s",
        started.elapsed().as_secs_f64()
    );
    assert!(report.launched, "{report:?}");
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.phase == RunPhase::Failed)
    );

    // Inside the Space: the profile's items landed, the cookies did not,
    // and Chrome runs.
    let sh = |cmd: String| {
        let space = space.clone();
        async move {
            let out = space.bash(&cmd, Duration::from_secs(60)).await.unwrap();
            out.stdout
        }
    };
    let root = "\"$HOME/Library/Application Support/Google/Chrome\"";
    let found = sh(format!("grep -rl '{marker}' {root} 2>/dev/null | head -5")).await;
    assert!(
        found.contains("Preferences") || found.contains("Bookmarks"),
        "profile landed: {found:?}"
    );
    let cookies = sh(format!(
        "grep -rl 'synthetic cookies' {root} 2>/dev/null | head -1"
    ))
    .await;
    assert!(
        cookies.trim().is_empty(),
        "cookies must not move: {cookies}"
    );
    let mut running = false;
    for _ in 0..30 {
        if !sh("pgrep -f 'Google Chrome.app/Contents/MacOS' | head -1".into())
            .await
            .trim()
            .is_empty()
        {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(running, "Chrome runs in the Space");
    eprintln!(
        "TIMING chrome running {:.2}s",
        started.elapsed().as_secs_f64()
    );
}
