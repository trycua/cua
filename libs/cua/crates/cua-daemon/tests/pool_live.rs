//! Opt-in: the local warm pool end to end on this machine's container engine
//! (gVisor on the built-in Linux runtime, a Linux dockerd, ...). A cold
//! create teaches the pool its shape; two concurrent creates of that shape
//! then claim one member each. Each claimed Space runs a command and takes a
//! screenshot with its own token; the members' pool secrets and the other
//! Space's token are refused; deletes leave nothing behind.
//!
//! ```text
//! CUA_HOME=/tmp/some-throwaway-home CUA_POOL_LIVE=1 \
//!   cargo test -p cua-daemon --test pool_live -- --ignored --nocapture
//! ```
//!
//! Needs a throwaway `CUA_HOME` (never `~/.cua`); image
//! `CUA_POOL_LIVE_IMAGE` (default `ghcr.io/trycua/linux:24.04-slim`).
//! Three Spaces and two members run at most; everything is deleted on every
//! path.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::{CreateRequest, Runtime, RuntimeConfig, local::VmmLocal};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};

async fn client(url: &str, token: &str) -> SpacesdClient {
    let o = ConnectOptions::parse(url)
        .unwrap()
        .token(token)
        .transport(TransportPreference::Native)
        .probe(false);
    SpacesdClient::connect(o).await.unwrap()
}

fn request(name: &str, image: &str) -> CreateRequest {
    CreateRequest {
        location: Some("local".into()),
        name: Some(name.into()),
        image: image.into(),
        ..Default::default()
    }
}

async fn wait_members(vmm: &VmmLocal, n: u32, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        let st = vmm.pool().expect("pool on").status();
        if st.members >= n {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the pool did not reach {n} members within {within:?} ({st:?})"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs Spaces on this machine's container engine; set CUA_POOL_LIVE=1 and a throwaway CUA_HOME"]
async fn claimed_spaces_are_their_own() {
    if std::env::var("CUA_POOL_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_POOL_LIVE=1");
        return;
    }
    let home = std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home");
    let home = std::path::PathBuf::from(home);
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_POOL_LIVE_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    // An explicit pool of two (`pool.warm`) for the image's default shape.
    // SAFETY: set before any thread reads the pool settings.
    unsafe { std::env::set_var("CUA_POOL_WARM", format!("{image}=2")) };
    let state = home.join("sandboxes");
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(state.clone()),
        vmm: Some(vmm.clone()),
        ..Default::default()
    })
    .unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    let pool_task = vmm
        .enable_pool(cua_sandbox_core::StateStore::new(state.clone()), rx)
        .expect("pool enabled once");

    let tag = format!("{:06x}", std::process::id() & 0xff_ffff);
    let names: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|s| format!("pl-{tag}-{s}"))
        .collect();
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        rt.create(request(&names[0], &image)).await.unwrap();
        wait_members(&vmm, 2, Duration::from_secs(300)).await;
        // The members' pool secrets, as recorded (0600) for adoption.
        let secrets: Vec<String> = std::fs::read_dir(state.join(".pool"))
            .unwrap()
            .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
            .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .filter_map(|v| v["secret"].as_str().map(str::to_string))
            .collect();
        assert_eq!(secrets.len(), 2);
        let t = Instant::now();
        let (b, c) = tokio::join!(
            rt.create(request(&names[1], &image)),
            rt.create(request(&names[2], &image)),
        );
        let (b, c) = (b.unwrap(), c.unwrap());
        eprintln!("two warm creates in {:?}", t.elapsed());
        assert_eq!(
            vmm.pool().unwrap().status().members,
            0,
            "both creates claimed a member"
        );
        assert_ne!(
            b.endpoints["env"], c.endpoints["env"],
            "never one member twice"
        );
        let mut tokens = BTreeMap::new();
        for r in [&b, &c] {
            let sb = rt.handle(&r.id).await.unwrap();
            tokens.insert(r.id.clone(), sb.env_token().expect("a token"));
        }
        for r in [&b, &c] {
            let url = &r.endpoints["env"];
            let own = client(url, &tokens[&r.id]).await;
            let out = own
                .run(cua_spacesd_client::Command::shell("echo ok"))
                .await
                .unwrap();
            assert!(out.success());
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
            let shot = own.screenshot(Default::default()).await.unwrap();
            assert!(shot.width > 0 && !shot.image.is_empty());
            for s in &secrets {
                let e = client(url, s).await.list_processes(false).await;
                assert!(
                    matches!(e, Err(cua_spacesd_client::Error::Unauthenticated(_))),
                    "a pool secret still opens {}: {e:?}",
                    r.id
                );
            }
            let other = tokens.iter().find(|(k, _)| **k != r.id).unwrap().1;
            let e = client(url, other).await.list_processes(false).await;
            assert!(
                matches!(e, Err(cua_spacesd_client::Error::Unauthenticated(_))),
                "another Space's token opens {}: {e:?}",
                r.id
            );
        }
        // A claimed Space restarted (its container stopped and started, as
        // the built-in runtime's power control does) comes back with its
        // own token; the pool secret still opens nothing.
        {
            use cua_vmm::Runtime as _;
            let engine = vmm.container().await.unwrap();
            engine.stop(&b.name).await.unwrap();
            engine.resume(&b.name).await.unwrap();
            let url = &b.endpoints["env"];
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                let ok = tokio::time::timeout(Duration::from_secs(5), async {
                    let c = client(url, &tokens[&b.id]).await;
                    c.list_processes(false).await
                })
                .await;
                if matches!(ok, Ok(Ok(_))) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "the restarted Space does not take its own token: {ok:?}"
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            for s in &secrets {
                let e = client(url, s).await.list_processes(false).await;
                assert!(
                    matches!(e, Err(cua_spacesd_client::Error::Unauthenticated(_))),
                    "a pool secret opens the restarted Space: {e:?}"
                );
            }
            eprintln!("{} restarted with its own token", b.name);
        }
        for n in &names {
            rt.delete(n).await.unwrap();
        }
    }))
    .await;
    let _ = stop.send(true);
    let _ = pool_task.await;
    vmm.pool().unwrap().drain().await;
    for n in &names {
        let _ = rt.delete(n).await;
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    let left = vmm.container().await.unwrap().pool_members().await.unwrap();
    assert!(left.is_empty(), "members left behind: {left:?}");
    use cua_sandbox_core::LocalRuntime as _;
    let listed = vmm.list().await.unwrap();
    assert!(
        listed.iter().all(|s| !names.contains(&s.name)),
        "Spaces left behind"
    );
}

/// A pool by name (`create_pool`), and a create that names it the way the
/// CLI does (`cua sb create --pool NAME`: no image, kind and runtime
/// `auto`): it takes the pool's shape and claims its ready Space. Run the
/// live tests one at a time (`--test-threads=1`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs Spaces on this machine's container engine; set CUA_POOL_LIVE=1 and a throwaway CUA_HOME"]
async fn a_create_from_a_named_pool_claims_its_space() {
    if std::env::var("CUA_POOL_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_POOL_LIVE=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_POOL_LIVE_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    let state = home.join("sandboxes");
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(state.clone()),
        vmm: Some(vmm.clone()),
        ..Default::default()
    })
    .unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    let pool_task = vmm
        .enable_pool(cua_sandbox_core::StateStore::new(state.clone()), rx)
        .expect("pool enabled once");
    let tag = format!("{:06x}", std::process::id() & 0xff_ffff);
    let pool_name = format!("live-{tag}");
    let space = format!("pn-{tag}");
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        // A shape of its own (1 CPU, 2 GB), so no other pool shares it.
        let spec = CreateRequest {
            location: Some("local".into()),
            image: image.clone(),
            kind: Some("container".into()),
            runtime: Some("gvisor".into()),
            cpus: Some(1),
            memory_mb: Some(2048),
            ..Default::default()
        };
        rt.create_pool(Some(&pool_name), spec, 1, 1, None)
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(300);
        while rt.get_pool(&pool_name).unwrap().ready < 1 {
            assert!(Instant::now() < deadline, "the pool never got ready");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let created = rt
            .create(CreateRequest {
                location: Some("local".into()),
                name: Some(space.clone()),
                pool: Some(pool_name.clone()),
                kind: Some("auto".into()),
                runtime: Some("auto".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let p = rt.get_pool(&pool_name).unwrap();
        assert_eq!(
            p.ready, 0,
            "the create claimed the pool's ready Space: {p:?}"
        );
        let sb = rt.handle(&created.id).await.unwrap();
        let token = sb.env_token().expect("a token");
        let out = client(&created.endpoints["env"], &token)
            .await
            .run(cua_spacesd_client::Command::shell("echo ok"))
            .await
            .unwrap();
        assert!(out.success());
        // Another image than the pool's is refused.
        let other = rt
            .create(CreateRequest {
                location: Some("local".into()),
                name: Some(format!("{space}-x")),
                image: "python:3.12-slim".into(),
                pool: Some(pool_name.clone()),
                ..Default::default()
            })
            .await;
        assert!(other.is_err(), "another image from the pool: {other:?}");
        rt.delete(&space).await.unwrap();
        assert!(rt.delete_pool(&pool_name).unwrap());
    }))
    .await;
    let _ = stop.send(true);
    let _ = pool_task.await;
    vmm.pool().unwrap().drain().await;
    let _ = rt.delete(&space).await;
    let _ = rt.delete_pool(&pool_name);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    let left = vmm.container().await.unwrap().pool_members().await.unwrap();
    assert!(left.is_empty(), "members left behind: {left:?}");
}

/// Stopping the daemon takes its warm-pool members with it: none keeps
/// running (and holding memory) once `wait` returned.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs Spaces on this machine's container engine; set CUA_POOL_LIVE=1 and a throwaway CUA_HOME"]
async fn stopping_the_daemon_removes_its_pool_members() {
    if std::env::var("CUA_POOL_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_POOL_LIVE=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_POOL_LIVE_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    // SAFETY: set before any thread reads the pool settings.
    unsafe { std::env::set_var("CUA_POOL_WARM", format!("{image}=1")) };
    // The built-in Linux runtime VM is small (4 GiB): the default 15% budget
    // is below one member's estimate, so no member would ever start.
    unsafe { std::env::set_var("CUA_POOL_MEMORY_PERCENT", "50") };
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        vmm: Some(vmm.clone()),
        ..Default::default()
    })
    .unwrap();
    let client_rt = rt.clone();
    let handle = cua_daemon::server::start(
        rt,
        cua_daemon::server::ServerConfig {
            socket_path: Some(home.join("cua.sock")),
            loopback: None,
            token: "t".into(),
            discovery_path: Some(home.join("daemon.json")),
            bridge_ticket_ttl: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();
    // A cold create teaches the pool its shape; its members follow.
    let cold = format!("pl-stop-{:06x}", std::process::id() & 0xff_ffff);
    client_rt.create(request(&cold, &image)).await.unwrap();
    wait_members(&vmm, 1, Duration::from_secs(300)).await;
    client_rt.delete(&cold).await.unwrap();
    let engine = vmm.container().await.unwrap();
    assert!(!engine.pool_members().await.unwrap().is_empty());
    handle.shutdown();
    handle.wait().await;
    let left = engine.pool_members().await.unwrap();
    if !left.is_empty() {
        // Leave nothing behind even when the assertion fails.
        for m in &left {
            use cua_vmm::Runtime as _;
            let _ = engine.delete(&m.name).await;
        }
    }
    assert!(left.is_empty(), "pool members left running: {left:?}");
}
