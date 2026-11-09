//! A Linux Space on the built-in Linux runtime, through the Spaces SDK the
//! apps use (`CUA_E2E_BUILTIN_LINUX=1`, macOS only).
//!
//! 1. creates `ghcr.io/trycua/linux:24.04` on This Mac with
//!    `runtime.linux = builtin`: the runtime is set up (downloaded) and its
//!    VM booted on demand, reported as the create's progress;
//! 2. checks the Space runs under gVisor (`dmesg` starts with gVisor's
//!    banner), a shell, and computer use (the driver moves the pointer; a
//!    capture of the painted desktop);
//! 3. stops the Space (the idle VM stops with it), starts it again (the VM
//!    boots on demand), deletes it (the VM stops again).
//!
//! Timings print as `TIMING` lines. With `CUA_E2E_EVIDENCE=<dir>`, the
//! progress reports (`progress.json`), the timings (`timings.json`), the
//! guest's `dmesg` and the desktop capture go there.
//!
//! | env | meaning |
//! |---|---|
//! | `CUA_HOME` | required, a temporary directory (never `~/.cua`); keep it short (Unix socket paths) |
//! | `CUA_RUNTIME_LINUX=builtin` | required: the built-in runtime even with Docker on this Mac |
//! | `CUA_E2E_BUILTIN_LINUX_FRESH=1` | remove the built-in runtime first (measures the first use) |
//! | `CUA_E2E_BUILTIN_LINUX_REMOVE=1` | remove it at the end (what Remove host setup does) |
//! | `CUA_E2E_BUILTIN_LINUX_IMAGE` | image (default `ghcr.io/trycua/linux:24.04`) |

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_daemon::local::VmmLocal;
use cua_e2e_local_runtimes::{gated, init_tracing, run_id};
use cua_sandbox_core::placement::On;
use cua_spaces::{ProgressSink, SpaceCreate, SpaceCreated, Spaces};
use cua_vmm::managed::{self, LinuxSource};
use serde_json::{Map, Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn a_linux_space_on_the_builtin_runtime() {
    if !gated("CUA_E2E_BUILTIN_LINUX") {
        return;
    }
    init_tracing();
    let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
    assert!(
        !home.is_empty() && !home.ends_with("/.cua"),
        "never the real cua home"
    );
    assert_eq!(
        LinuxSource::current(),
        LinuxSource::Builtin,
        "set CUA_RUNTIME_LINUX=builtin"
    );
    let evidence = std::env::var("CUA_E2E_EVIDENCE")
        .ok()
        .map(std::path::PathBuf::from);
    if let Some(d) = &evidence {
        std::fs::create_dir_all(d).unwrap();
    }
    let save = |name: &str, bytes: &[u8]| {
        if let Some(d) = &evidence {
            std::fs::write(d.join(name), bytes).unwrap();
        }
    };
    if std::env::var("CUA_E2E_BUILTIN_LINUX_FRESH").as_deref() == Ok("1") {
        managed::remove().await.unwrap();
    }
    let fresh = !managed::exists();
    let was_running = managed::running();
    let image = std::env::var("CUA_E2E_BUILTIN_LINUX_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04".into());
    let mut timings = Map::new();
    let mut lap = |label: &str, secs: f64| {
        eprintln!("TIMING {label}: {secs:.1}s");
        timings.insert(label.into(), json!((secs * 10.0).round() / 10.0));
    };

    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .download_dir(reg.path().join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .local_runtime(Arc::new(VmmLocal::default()))
        .probe_timeout(Duration::from_secs(20))
        .build();

    // 1. Create, recording every progress report.
    let t0 = Instant::now();
    let reports: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = {
        let reports = reports.clone();
        ProgressSink::new(move |p| {
            reports.lock().unwrap().push(json!({
                "t": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
                "phase": p.phase.as_str(),
                "fraction": p.fraction,
                "detail": p.detail,
                "bytes": p.bytes.map(|b| json!([b.done, b.total])),
            }));
        })
    };
    let name = format!("e2e-builtin-{}", run_id());
    let created = spaces
        .create(SpaceCreate {
            image: Some(image.clone()),
            on: Some(On::Local),
            name: Some(name.clone()),
            cpus: Some(2),
            memory_mb: Some(3072),
            progress: Some(sink),
            ..Default::default()
        })
        .await
        .expect("create");
    let create_secs = t0.elapsed().as_secs_f64();
    let reports = reports.lock().unwrap().clone();
    save(
        "progress.json",
        &serde_json::to_vec_pretty(&reports).unwrap(),
    );
    // The runtime's set up is the create's own progress, in words.
    let setup: Vec<&Value> = reports
        .iter()
        .filter(|r| r["detail"] == managed::SETTING_UP)
        .collect();
    let mut setup_end = 0.0;
    if !was_running {
        assert!(
            !setup.is_empty(),
            "no runtime set-up progress: {reports:#?}"
        );
        setup_end = setup.last().unwrap()["t"].as_f64().unwrap();
        if fresh {
            let dl_end = setup
                .iter()
                .filter(|r| !r["bytes"].is_null())
                .map(|r| r["t"].as_f64().unwrap())
                .fold(0.0, f64::max);
            lap("first-use download", dl_end);
            lap("runtime VM cold boot", setup_end - dl_end);
        } else {
            lap("runtime VM boot", setup_end);
        }
    }
    lap("Space create (total)", create_secs);
    lap(
        "Space create after the runtime is up",
        create_secs - setup_end,
    );
    let SpaceCreated::Ready { info, .. } = created else {
        panic!("not ready: {created:?}");
    };
    assert!(managed::running());

    // 2. gVisor, a shell, computer use.
    let space = spaces.space(&info.id).await.unwrap();
    let dmesg = space
        .bash("dmesg | head -5; uname -r", Duration::from_secs(60))
        .await
        .unwrap();
    eprintln!("dmesg:\n{}", dmesg.stdout);
    save("dmesg.txt", dmesg.stdout.as_bytes());
    assert!(dmesg.success(), "{dmesg:?}");
    assert!(
        dmesg.stdout.contains("Starting gVisor"),
        "not gVisor: {}",
        dmesg.stdout
    );
    let echo = space
        .bash(
            "echo hi from $(whoami) && ls / | head -3",
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(echo.success() && echo.stdout.starts_with("hi from"));
    let (tools, _) = space.list_tools(None).await.expect("driver tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("{} driver tools: {names:?}", names.len());
    assert!(names.contains(&"get_desktop_state") && names.contains(&"move_cursor"));
    let args = |v: Value| v.as_object().cloned().unwrap();
    let desktop = json!({"kind": "desktop", "display_id": "primary"});
    // Computer use: move the pointer, check it moved from inside the guest.
    let moved = space
        .call_tool(
            None,
            "move_cursor",
            args(json!({"x": 211, "y": 157, "target": desktop})),
            Some(Duration::from_secs(60)),
        )
        .await
        .expect("move_cursor");
    assert!(!moved.is_error, "{:?}", moved.content);
    let pointer = space
        .bash(
            "DISPLAY=:1 xdotool getmouselocation",
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert!(pointer.stdout.contains("x:211 y:157"), "{}", pointer.stdout);
    // And see the desktop (ready is cua-spacesd up; XFCE paints a few
    // seconds later).
    use base64::Engine as _;
    let mut png = Vec::new();
    for _ in 0..24 {
        let shot = space
            .call_tool(
                None,
                "get_desktop_state",
                Map::new(),
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("get_desktop_state");
        assert!(!shot.is_error, "{:?}", shot.content);
        let data = shot
            .content
            .iter()
            .find(|c| c["type"] == "image")
            .and_then(|c| c["data"].as_str())
            .expect("an image part");
        png = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        // A painted desktop compresses to tens of KB; a blank frame to
        // a few hundred bytes.
        if png.len() > 10_000 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    save("desktop.png", &png);
    eprintln!("desktop capture: {} bytes", png.len());
    assert!(png.len() > 10_000, "a painted desktop, not a blank frame");

    // 3. Stop (the idle VM stops), start (it boots again), delete.
    let t = Instant::now();
    spaces.stop(&info.id).await.expect("stop");
    lap("Space stop (and idle VM stop)", t.elapsed().as_secs_f64());
    assert!(!managed::running(), "the idle VM stops with the last Space");
    let t = Instant::now();
    spaces.start(&info.id).await.expect("start");
    lap(
        "Space start (VM boots on demand)",
        t.elapsed().as_secs_f64(),
    );
    assert!(managed::running());
    let again = spaces.space(&info.id).await.unwrap();
    let out = again
        .bash("dmesg | head -1", Duration::from_secs(60))
        .await
        .unwrap();
    assert!(out.stdout.contains("gVisor"), "{out:?}");
    let t = Instant::now();
    spaces.delete(&info.id).await.expect("delete");
    lap("Space delete (and idle VM stop)", t.elapsed().as_secs_f64());
    assert!(!managed::running());
    save(
        "timings.json",
        &serde_json::to_vec_pretty(&Value::Object(timings)).unwrap(),
    );
    if std::env::var("CUA_E2E_BUILTIN_LINUX_REMOVE").as_deref() == Ok("1") {
        managed::remove().await.unwrap();
        assert!(!managed::root().exists());
    }
}
