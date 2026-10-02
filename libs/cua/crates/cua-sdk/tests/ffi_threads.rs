//! Exported object methods called the way the foreign bindings call them:
//! from a plain thread with no Tokio runtime, polled by a minimal executor
//! (UniFFI polls Rust futures from the host language's own event loop).
//! A method that awaits Tokio I/O or timers without going through the SDK
//! runtime (`run`) panics with "there is no reactor running" here.
//!
//! Two checks:
//! - dynamic: env, Fleet, sandboxes, Spaces and presence methods against
//!   in-process fakes (MockServer spacesd with presence, FakeFleet), in
//!   the embedded and daemon topologies;
//! - static: every `pub async fn` in an exported impl either goes through
//!   the runtime (`run(`, `env_call!`, `fleet_call!`) or only delegates to
//!   methods that do, so new methods cannot skip it silently.
//!
//! Nothing touches host apps; every wait is bounded.

use cua_daemon::{
    Runtime, RuntimeConfig,
    fixtures::{self, SpacesdFixture},
    server::{self, DaemonHandle, ServerConfig},
};
use cua_fleet::testing::FakeFleet;
use cua_sdk::{
    Cua, CuaConfig, FleetPoolSpec, PresenceCursor, PresenceIdentity, PresencePoint,
    SandboxCreateOptions, SpacesdCommand, presence_now_ms,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    pin::pin,
    sync::{Arc, mpsc},
    task::{Context, Poll, Wake, Waker},
    thread::{self, Thread},
    time::Duration,
};

const TOKEN: &str = "env-token";
const POOL: &str = "cua-e2e-ffi";

/// Polls `f` on the current thread, parking between wakeups. No Tokio.
fn block_on<F: Future>(f: F) -> F::Output {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        thread::park_timeout(Duration::from_millis(50));
    }
}

/// Runs `body` on a fresh plain thread (no Tokio context) with a deadline.
fn on_foreign_thread(name: &str, body: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    let h = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
            let _ = tx.send(r.map_err(|p| {
                p.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default()
            }));
        })
        .unwrap();
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(())) => {
            h.join().unwrap();
        }
        Ok(Err(msg)) => panic!("{name}: {msg}"),
        Err(_) => panic!("{name}: timed out"),
    }
}

fn cmd(program: &str, args: &[&str]) -> SpacesdCommand {
    SpacesdCommand {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: HashMap::new(),
        cwd: None,
        user: None,
        timeout_ms: None,
        tag: None,
        stdin: false,
        pty: None,
    }
}

fn direct(url: &str, name: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        on: Some(format!("direct:{url}")),
        kind: None,
        runtime: None,
        image: String::new(),
        name: Some(name.into()),
        token: Some(TOKEN.into()),
        pool: None,
        os: None,
        cpus: None,
        memory_mb: None,
        ports: vec![],
        services: HashMap::new(),
        wait_for: vec![],
        ready_timeout_ms: None,
        env: HashMap::new(),
        fleet_replicas: None,
        fleet_ttl_seconds: None,
        warm: None,
        max_pool_size: None,
        command: None,
        cloud: None,
        sidecars: vec![],
        registry_secret: None,
        build: None,
        network: None,
        overlays: vec![],
        keep_on_failure: false,
        gpu: None,
    }
}

/// Fixtures and the SDK objects, built on a background Tokio runtime that
/// only serves the fakes.
struct World {
    rt: tokio::runtime::Runtime,
    env: SpacesdFixture,
    _gw: SpacesdFixture,
    _dirs: tempfile::TempDir,
    _daemon: Option<DaemonHandle>,
}

fn world(daemon: bool) -> (World, Arc<Cua>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dirs = tempfile::tempdir().unwrap();
    let (env, gw, cua, handle) = rt.block_on(async {
        let env = fixtures::start_env(Some(TOKEN), None).await;
        env.mock
            .state
            .advertise(&["presence", cua_spacesd_client::TUNNEL_FORWARD_FEATURE]);
        let gw = fixtures::start_env(None, Some(fixtures::fake_gateway(POOL, POOL))).await;
        let fake = FakeFleet::new();
        let runtime = Runtime::new(RuntimeConfig {
            state_dir: Some(dirs.path().join("sandboxes")),
            spaces_home: Some(dirs.path().join("cua")),
            teleport_home: Some(dirs.path().join("host-home")),
            fleet_client: Some(fake.client_with_base(&gw.url)),
            env_probe_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        })
        .unwrap();
        if !daemon {
            return (env, gw, Cua::from_runtime(runtime), None);
        }
        let h = server::start(
            runtime,
            ServerConfig {
                socket_path: None,
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            },
        )
        .await
        .unwrap();
        // `Cua::connect` itself is exercised on the foreign thread below.
        let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
        (env, gw, cua, Some(h))
    });
    (
        World {
            rt,
            env,
            _gw: gw,
            _dirs: dirs,
            _daemon: handle,
        },
        cua,
    )
}

async fn exercise(cua: Arc<Cua>, env_url: String, with_fleet: bool) {
    // ---- Cua
    let _ = cua.info().await.unwrap();

    // ---- env
    let env = cua
        .spacesd(env_url.clone(), Some(TOKEN.into()))
        .await
        .unwrap();
    env.capabilities().await.unwrap();
    assert!(env.has_feature("presence".into()).await.unwrap());
    env.health().await.unwrap();
    let out = env.run(cmd("echo", &["ffi"])).await.unwrap();
    assert_eq!(out.stdout, b"ffi\n");
    env.sh("echo sh".into(), Some(5_000)).await.unwrap();
    let p = env.spawn(cmd("echo", &["spawned"])).await.unwrap();
    let _ = p.next_event().await.unwrap();
    p.wait().await.unwrap();
    env.list_processes(true).await.unwrap();
    env.upload("/tmp/ffi.txt".into(), b"hi".to_vec(), None)
        .await
        .unwrap();
    assert_eq!(env.download("/tmp/ffi.txt".into()).await.unwrap(), b"hi");
    let _ = env.stat("/tmp/ffi.txt".into()).await;
    let _ = env.list_dir("/tmp".into(), 1).await;
    let _ = env.make_dir("/tmp/ffi-dir".into()).await;
    let _ = env.remove("/tmp/ffi.txt".into(), false).await;
    env.screenshot(None).await.unwrap();
    env.click(10.0, 10.0).await.unwrap();
    env.double_click(10.0, 10.0).await.unwrap();
    env.right_click(10.0, 10.0).await.unwrap();
    env.move_to(20.0, 20.0).await.unwrap();
    env.scroll(0.0, 1.0).await.unwrap();
    env.drag(1.0, 1.0, 5.0, 5.0).await.unwrap();
    env.type_text("x".into()).await.unwrap();
    env.press("enter".into()).await.unwrap();
    env.hotkey(vec!["ctrl".into(), "c".into()]).await.unwrap();
    env.set_clipboard("clip".into()).await.unwrap();
    let _ = env.get_clipboard().await.unwrap();
    env.cursor_position().await.unwrap();
    let _ = env.displays().await;
    env.call_json("/cua.env.v1.SystemService/Health".into(), "{}".into())
        .await
        .unwrap();

    // ---- Fleet (client side; embedded holds the fake Fleet client)
    if with_fleet {
        let fleet = cua.fleet().unwrap();
        // The runtime defaults from the image's manifest (a fixture here).
        cua_fleet::testing::set_image_variant(
            "registry.test/cua-e2e:fake",
            cua_fleet::ImageVariant::ContainerDisk,
        );
        fleet
            .apply_pool(FleetPoolSpec {
                name: POOL.into(),
                image: "registry.test/cua-e2e:fake".into(),
                runtime: None,
                replicas: None,
                cpu: None,
                memory_mb: None,
                services: HashMap::from([("env".to_string(), 3211)]),
                readiness_tcp_port: None,
                efi: false,
                command: None,
                ttl_seconds_after_created: None,
            })
            .await
            .unwrap();
        fleet.get_pool(POOL.into()).await.unwrap();
        fleet.list_pools(POOL.into()).await.unwrap();
        let claim = fleet
            .acquire(POOL.into(), Some(format!("{POOL}-c")), None)
            .await
            .unwrap();
        fleet.list_claims(POOL.into()).await.unwrap();
        fleet
            .keep_alive(POOL.into(), claim.claim.clone(), 600)
            .await
            .unwrap();
        fleet.release(POOL.into(), claim.claim).await.unwrap();
    }

    // ---- sandboxes (direct)
    let sbx = cua.sandboxes();
    let sb = sbx.create(direct(&env_url, "ffi-direct")).await.unwrap();
    sb.refresh().await.unwrap();
    let senv = sb.spacesd(Some(5_000)).await.unwrap();
    senv.sh("echo sandbox".into(), None).await.unwrap();
    let r = sb
        .service("env".into())
        .unwrap()
        .request("GET".into(), "/".into(), None, Some(5_000), None)
        .await;
    let _ = r; // Any HTTP answer (or typed error) is fine; no panic is the point.
    let fwd = sb.forward(3211).await.unwrap();
    fwd.close().await.unwrap();
    sbx.list(None).await.unwrap();
    sb.delete().await.unwrap();

    // ---- Spaces and presence
    let spaces = cua.spaces();
    let info = spaces
        .add(env_url.clone(), Some(TOKEN.into()), Some("ffi".into()))
        .await
        .unwrap();
    spaces.list().await.unwrap();
    let space = spaces.space(info.id.clone()).await.unwrap();
    let presence = space
        .join_presence(
            PresenceIdentity {
                id: "u1".into(),
                display_name: "FFI".into(),
                color: String::new(),
                agent: false,
            },
            Some(5_000),
        )
        .await
        .unwrap();
    let me = presence.me().await.unwrap();
    presence.roster().await.unwrap();
    presence
        .update_cursor(PresenceCursor {
            display_id: String::new(),
            window_id: None,
            x: 0.5,
            y: 0.25,
            visible: true,
            pressed: false,
            shape: "arrow".into(),
            shape_source: "unspecified".into(),
            at_ms: 0.0,
            received_ms: 0.0,
        })
        .await
        .unwrap();
    let mut moved = false;
    let view = presence.view();
    assert!(!presence.uses_datagrams());
    for _ in 0..20 {
        match presence.next_event(Some(2_000)).await {
            Ok(Some(e)) if e.kind == "cursor_moved" => {
                assert!(e.cursor.as_ref().unwrap().at_ms > 0.0, "{e:?}");
                view.apply(e, presence_now_ms());
                moved = true;
                break;
            }
            Ok(Some(_)) => continue,
            other => panic!("presence event: {other:?}"),
        }
    }
    assert!(moved, "{me:?} saw its own cursor move");
    // The caller draws its own cursor at the local pointer, never from the
    // network.
    let drawn = view.drawables(presence_now_ms(), Some(PresencePoint { x: 0.1, y: 0.2 }));
    let mine = drawn.iter().find(|d| d.is_me).unwrap();
    assert_eq!((mine.x, mine.y, mine.shape.as_str()), (0.1, 0.2, "arrow"));
    space.set_presence_settings(Some(true)).await.unwrap();
    presence.leave().await.unwrap();
    spaces.remove(info.id).await.unwrap();
}

#[test]
fn exported_methods_run_without_a_tokio_runtime_embedded() {
    let (w, cua) = world(false);
    let url = w.env.url.clone();
    on_foreign_thread("ffi-embedded", move || {
        // A plain FFI constructor too.
        let dir = tempfile::tempdir().unwrap();
        let other = Cua::embedded(CuaConfig {
            state_dir: Some(dir.path().join("s").display().to_string()),
            fleet_from_env: false,
            spaces_home: Some(dir.path().join("cua").display().to_string()),
            ..Default::default()
        })
        .unwrap();
        block_on(other.info()).unwrap();
        block_on(exercise(cua, url, true));
    });
    drop(w.rt);
}

#[test]
fn exported_methods_run_without_a_tokio_runtime_daemon() {
    let (w, cua) = world(true);
    let url = w.env.url.clone();
    on_foreign_thread("ffi-daemon", move || block_on(exercise(cua, url, false)));
    drop(w.rt);
}

// ------------------------------------------------------------------ static

/// Methods whose bodies need no reactor (they only lock a
/// `tokio::sync::Mutex`, which works on any executor).
const NO_REACTOR: &[&str] = &["me", "roster"];

/// `name -> bodies` of every `fn` in the SDK's native sources.
fn fn_bodies(src: &str, out: &mut BTreeMap<String, Vec<String>>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim_start();
        let is_fn = t.starts_with("pub async fn ")
            || t.starts_with("async fn ")
            || t.starts_with("pub(crate) async fn ")
            || t.starts_with("pub fn ")
            || t.starts_with("fn ")
            || t.starts_with("pub(crate) fn ");
        if !is_fn {
            i += 1;
            continue;
        }
        let name: String = t
            .split("fn ")
            .nth(1)
            .unwrap()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let indent = l.len() - t.len();
        let close = format!("{}}}", " ".repeat(indent));
        let mut body = String::new();
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim_end() != close {
            body.push_str(lines[j]);
            body.push('\n');
            j += 1;
        }
        out.entry(name).or_default().push(body);
        i = j + 1;
    }
}

/// `(name, line, body)` of the `pub async fn`s inside `#[uniffi::export]
/// impl` blocks.
fn exported_async(src: &str) -> Vec<(String, usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut in_export = false;
    let mut pending = false;
    for (n, l) in lines.iter().enumerate() {
        if l.starts_with("#[uniffi::export") {
            pending = true;
            continue;
        }
        if l.starts_with("impl ") {
            in_export = pending;
            pending = false;
            continue;
        }
        if l.starts_with('}') {
            in_export = false;
            continue;
        }
        let t = l.trim_start();
        if in_export && let Some(rest) = t.strip_prefix("pub async fn ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let close = format!("{}}}", " ".repeat(l.len() - t.len()));
            let body: Vec<&str> = lines[n + 1..]
                .iter()
                .take_while(|b| b.trim_end() != close)
                .copied()
                .collect();
            out.push((name, n + 1, body.join("\n")));
        }
    }
    out
}

fn body_uses_runtime(
    body: &str,
    fns: &BTreeMap<String, Vec<String>>,
    seen: &mut HashSet<String>,
) -> bool {
    if body.contains("run(") || body.contains("env_call!") || body.contains("fleet_call!") {
        return true;
    }
    // A delegator: it awaits a method (any same-named helper) that does.
    body.split(['.', ':']).skip(1).any(|seg| {
        let name: String = seg
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        seg[name.len()..].starts_with('(')
            && name != "clone"
            && (NO_REACTOR.contains(&name.as_str())
                || (seen.insert(name.clone())
                    && fns
                        .get(&name)
                        .is_some_and(|bs| bs.iter().any(|b| body_uses_runtime(b, fns, seen)))))
    })
}

#[test]
fn every_exported_async_method_goes_through_the_sdk_runtime() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/native");
    let mut fns = BTreeMap::new();
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().is_some_and(|e| e == "rs") {
            let s = std::fs::read_to_string(&p).unwrap();
            fn_bodies(&s, &mut fns);
            sources.push((p, s));
        }
    }
    let mut missing = Vec::new();
    let mut total = 0;
    for (p, s) in &sources {
        for (name, line, body) in exported_async(s) {
            total += 1;
            if !NO_REACTOR.contains(&name.as_str())
                && !body_uses_runtime(&body, &fns, &mut HashSet::new())
            {
                missing.push(format!("{}:{line} {name}", p.display()));
            }
        }
    }
    assert!(total > 100, "found only {total} exported async methods");
    assert!(
        missing.is_empty(),
        "exported async methods that bypass the SDK runtime (wrap them in `run`):\n{}",
        missing.join("\n")
    );
}
