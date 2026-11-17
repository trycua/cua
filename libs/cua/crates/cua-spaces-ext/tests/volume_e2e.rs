// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in e2e (`CUA_VOLUME_E2E=1`): Cua Volume mounted in real Spaces,
//! through the daemon's own runtime (the Spaces registry, the drive and its
//! services, local Docker and Lume). Run through
//! `tests/e2e/run-volume-e2e.sh`, which builds the images and cleans up.
//!
//! - `CUA_VOLUME_E2E_LINUX_IMAGE`: a Linux image with this branch's
//!   cua-spacesd, fuse3, `/volume` and the volume helper (the script builds
//!   one on the published slim image). Runs under gVisor when the engine has
//!   it.
//! - `CUA_VOLUME_E2E_MACOS_IMAGE` (optional): a Lume image; with
//!   `CUA_VOLUME_E2E_MACOS_SPACESD`, this branch's macOS cua-spacesd is
//!   installed into the guest first (published images predate the volume).
//!   The VM gets at most 6 GiB.
//! - `ANTHROPIC_API_KEY` (optional): a persistent Claude Code agent is asked
//!   to use the volume through a shell, and the host checks the file. Without
//!   it the agent's run still starts with its home on the mount, and fails at
//!   its first model call.
//!
//! Everything lives in a throwaway cua home under `CUA_VOLUME_E2E_HOME`
//! (default: the system temp dir); Spaces are deleted at the end, pass or
//! fail. Timings are printed as `timing <what> <ms>`.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::{Runtime, RuntimeConfig};
use cua_keyvault::broker::FakePresence;
use cua_spaces::{Space, SpaceCreate, SpaceCreated, Spaces};
use cua_spaces_ext::SpacesDrive as _;
use cua_spaces_ext::daemon::CuaSpacesDaemon;
use serde_json::{Value, json};

fn enabled() -> bool {
    std::env::var("CUA_VOLUME_E2E").as_deref() == Ok("1")
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.is_empty())
}

fn timing(what: &str, t0: Instant) -> u128 {
    let ms = t0.elapsed().as_millis();
    eprintln!("timing {what} {ms}");
    ms
}

async fn sh(space: &Space, cmd: &str) -> (bool, String, String) {
    let out = space
        .bash(cmd, Duration::from_secs(300))
        .await
        .unwrap_or_else(|e| panic!("{cmd}: {e}"));
    (out.success(), out.stdout, out.stderr)
}

async fn ok(space: &Space, cmd: &str) -> String {
    let (good, out, err) = sh(space, cmd).await;
    assert!(good, "{cmd}: {out}{err}");
    out
}

/// Polls `f` every 100 ms until it returns `Some`, for at most `limit`.
async fn until<T, F, Fut>(what: &str, limit: Duration, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let t0 = Instant::now();
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(t0.elapsed() < limit, "{what}: not within {limit:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn read(spaces: &Spaces, path: &str) -> Option<Vec<u8>> {
    spaces
        .drive()
        .session(cua_volume::Context::user())
        .read(path, None)
        .await
        .ok()
        .map(|(b, _)| b)
}

async fn write(spaces: &Spaces, path: &str, body: &[u8]) {
    spaces
        .drive()
        .session(cua_volume::Context::user())
        .write(path, body.to_vec(), cua_volume::Condition::None)
        .await
        .unwrap();
}

/// A Space from `image`, its volume mounted (as the daemon does at start).
async fn create(spaces: &Spaces, image: &str, memory_mb: Option<u64>) -> (String, u128) {
    let t0 = Instant::now();
    let created = spaces
        .create(SpaceCreate {
            image: Some(image.into()),
            on: Some(cua_sandbox_core::placement::On::Local),
            memory_mb,
            // A local test image is not a known Cua image: say it runs
            // cua-spacesd, so create waits for it.
            spacesd: Some(true),
            wait: Some(true),
            // A first macOS pull is about 23 GB.
            timeout: Some(Duration::from_secs(3600)),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = match created {
        SpaceCreated::Ready { info, .. } => info.id,
        SpaceCreated::Starting(p) => panic!("{} is still starting", p.id),
    };
    let ready = timing(&format!("{image} ready"), t0);
    (id, ready)
}

/// The volume the daemon mounted at start, once it is up.
async fn mounted(spaces: &Spaces, id: &str) -> cua_spaces_ext::VolumeInfo {
    let t0 = Instant::now();
    let v = until(
        "the volume mounts at Space start",
        Duration::from_secs(120),
        || async { spaces.volume(id).await },
    )
    .await;
    timing(&format!("{id} volume mounted after ready"), t0);
    v
}

/// Host, guest and back; what the Space may and may not do.
async fn round_trip(spaces: &Spaces, id: &str, tag: &str) -> String {
    let space = spaces.space(id).await.unwrap();
    let v = mounted(spaces, id).await;
    let mp = v.mount_path.clone();
    assert_eq!(
        v.principal,
        format!("space:{}", cua_volume::path::space_folder_name(id))
    );
    let q = |p: &str| format!("'{}'", format!("{mp}/{p}").replace('\'', "'\\''"));

    // The Space sees public/ and its own folder, nothing else.
    let root = ok(&space, &format!("ls -1 {}", q(""))).await;
    assert_eq!(
        root.split_whitespace().collect::<Vec<_>>(),
        ["public", "spaces"],
        "{root}"
    );
    let folder = ok(&space, &format!("ls -1 {}", q("spaces"))).await;
    let folder = folder.trim().to_string();
    assert_eq!(folder, cua_volume::path::space_folder_name(id));

    // Host -> guest.
    let body = format!("from the host to {tag}");
    let t0 = Instant::now();
    write(spaces, &format!("public/{tag}.txt"), body.as_bytes()).await;
    let cat = format!("cat {} 2>/dev/null", q(&format!("public/{tag}.txt")));
    until(
        "the guest reads the host's write",
        Duration::from_secs(15),
        || async {
            let (good, out, _) = sh(&space, &cat).await;
            (good && out == body).then_some(())
        },
    )
    .await;
    timing(&format!("{tag} host write visible in the guest"), t0);

    // Guest -> host.
    let t0 = Instant::now();
    ok(
        &space,
        &format!(
            "printf 'from the guest' > {}",
            q(&format!("spaces/{folder}/from-guest.txt"))
        ),
    )
    .await;
    let key = format!("spaces/{folder}/from-guest.txt");
    until(
        "the host reads the guest's write",
        Duration::from_secs(15),
        || async { read(spaces, &key).await.filter(|b| b == b"from the guest") },
    )
    .await;
    timing(&format!("{tag} guest write stored on the host"), t0);

    // Refused: public/ is read-only, other areas do not exist here.
    let (good, _, err) = sh(&space, &format!("echo no > {}", q("public/nope.txt"))).await;
    assert!(!good, "a Space wrote to public/");
    assert!(
        ["Permission denied", "Read-only", "Operation not permitted"]
            .iter()
            .any(|m| err.contains(m)),
        "{err}"
    );
    assert!(read(spaces, "public/nope.txt").await.is_none());
    let (good, _, _) = sh(&space, &format!("mkdir {}", q("agents"))).await;
    assert!(!good, "a Space created agents/");

    // Throughput through the guest mount: 64 MiB out and back.
    let big = q(&format!("spaces/{folder}/big.bin"));
    let t0 = Instant::now();
    ok(
        &space,
        &format!("dd if=/dev/urandom of={big} bs=1048576 count=64 2>/dev/null"),
    )
    .await;
    let w = timing(&format!("{tag} write 64 MiB in the guest"), t0);
    let t0 = Instant::now();
    ok(
        &space,
        &format!("dd if={big} of=/dev/null bs=1048576 2>/dev/null"),
    )
    .await;
    let r = timing(&format!("{tag} read 64 MiB in the guest"), t0);
    eprintln!(
        "timing {tag} MiB/s write {} read {}",
        64_000 / w.max(1),
        64_000 / r.max(1)
    );
    let key = format!("spaces/{folder}/big.bin");
    until("the 64 MiB file lands", Duration::from_secs(60), || async {
        read(spaces, &key).await.filter(|b| b.len() == 64 << 20)
    })
    .await;
    ok(&space, &format!("rm {big}")).await;

    // The sync view reports the Space's mount.
    let st = cua_spaces_ext::drive_tools::sync_status_for(spaces, &cua_volume::Context::user())
        .await
        .unwrap();
    assert!(
        st["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["space"] == json!(id) && x["mount_path"] == json!(mp)),
        "{st}"
    );
    folder
}

/// A persistent agent's home is its folder on the mount while it runs.
async fn persistent_agent(spaces: &Spaces, id: &str, folder: &str) {
    let server = cua_spaces::mcp::McpServer::new(spaces.clone());
    let call = |name: &'static str, a: Value| {
        let server = &server;
        async move {
            let o = Box::pin(server.call(name, a)).await;
            let text = o.content[0]["text"].as_str().unwrap_or("").to_string();
            let v = serde_json::from_str(&text).unwrap_or(Value::String(text));
            assert!(!o.is_error, "{name}: {v}");
            v
        }
    };
    let key = env("ANTHROPIC_API_KEY");
    let mut agent_env = serde_json::Map::new();
    agent_env.insert(
        "ANTHROPIC_API_KEY".into(),
        json!(key.clone().unwrap_or_else(|| "not-a-key".into())),
    );
    call(
        "persistent_agent_create",
        json!({"name": "vol-ada", "space": id, "agent": "claude-code", "env": agent_env}),
    )
    .await;
    let prompt = format!(
        "Use a shell command (not the drive tools) to write the text volume-ok to the \
         file spaces/{folder}/proof.txt on Cua Volume, then stop."
    );
    let t0 = Instant::now();
    let sent = call(
        "persistent_agent_send",
        json!({"name": "vol-ada", "text": prompt}),
    )
    .await;
    timing("agent run started with its home on the mount", t0);
    assert_eq!(sent["restored"]["mounted"], json!(true), "{sent}");
    let v = spaces.volume(id).await.expect("mounted");
    assert_eq!(v.principal, "agent:vol-ada");
    let space = spaces.space(id).await.unwrap();
    let home = format!("{}/agents/vol-ada", v.mount_path);
    ok(&space, &format!("test -d '{home}/work'")).await;
    let marker = until(
        "the host sees the agent's home",
        Duration::from_secs(15),
        || async {
            spaces
                .drive()
                .session(cua_volume::Context::user())
                .ls("agents/vol-ada/")
                .await
                .ok()
                .filter(|e| e.iter().any(|x| x.name == "work"))
        },
    )
    .await;
    eprintln!(
        "agent home on the host: {:?}",
        marker.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    // Its view on the mount: its home, and no other agent's.
    let root = ok(&space, &format!("ls -1 '{}/agents'", v.mount_path)).await;
    assert_eq!(root.trim(), "vol-ada");
    if key.is_some() {
        let proof = format!("spaces/{folder}/proof.txt");
        let t0 = Instant::now();
        let got = until(
            "the agent writes through the mount",
            Duration::from_secs(600),
            || async { read(spaces, &proof).await },
        )
        .await;
        timing("agent task done", t0);
        assert!(String::from_utf8_lossy(&got).contains("volume-ok"));
        // It used the mount: the guest's skills include the volume's.
        ok(&space, "ls ~/.claude/skills/cua-volume/SKILL.md").await;
    } else {
        eprintln!("no ANTHROPIC_API_KEY: the agent's model step is not exercised");
    }
    call("persistent_agent_remove", json!({"name": "vol-ada"})).await;
    let v = spaces.volume(id).await.expect("still mounted");
    assert_eq!(v.principal, format!("space:{folder}"));
    let root = ok(&space, &format!("ls -1 '{}'", v.mount_path)).await;
    assert!(!root.contains("agents"), "{root}");
}

/// Replaces the guest's cua-spacesd with this branch's and reconnects.
async fn upgrade_macos_spacesd(spaces: &Spaces, id: &str, bin: &Path) {
    let space = spaces.space(id).await.unwrap();
    let t0 = Instant::now();
    space
        .upload(bin, Some("/tmp/cua-spacesd.new"))
        .await
        .unwrap();
    ok(
        &space,
        "chmod 755 /tmp/cua-spacesd.new && \
         { sudo -n true 2>/dev/null && S='sudo -n' || S=\"sudo -S -p ''\"; } && \
         printf 'lume\\n' | $S cp /tmp/cua-spacesd.new \
         '/Applications/Cua Spacesd.app/Contents/MacOS/cua-spacesd' && \
         (nohup sh -c 'sleep 1; launchctl kickstart -k gui/$(id -u)/com.trycua.spacesd' \
         >/dev/null 2>&1 &)",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    spaces.forget_connection(id).await.unwrap();
    until(
        "the new driver answers",
        Duration::from_secs(120),
        || async {
            let s = spaces.space(id).await.ok()?;
            if s.supports(cua_spaces::volume::VOLUME_FEATURE) {
                Some(())
            } else {
                let _ = spaces.forget_connection(id).await;
                None
            }
        },
    )
    .await;
    timing("macOS guest driver upgraded", t0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cua_volume_in_linux_and_macos_spaces() {
    if !enabled() {
        eprintln!("skipped: set CUA_VOLUME_E2E=1 (run tests/e2e/run-volume-e2e.sh)");
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "cua_spaces=info,cua_spacesd_client=warn".into()),
        )
        .with_test_writer()
        .try_init();
    let linux = env("CUA_VOLUME_E2E_LINUX_IMAGE").expect("CUA_VOLUME_E2E_LINUX_IMAGE");
    let base = env("CUA_VOLUME_E2E_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&base).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("volume-e2e-")
        .tempdir_in(&base)
        .unwrap();
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().join("cua")),
        vmm: Some(Arc::new(cua_daemon::local::VmmLocal::new(
            Default::default(),
        ))),
        // The Cua Spaces extension, with a test presence gate: nothing
        // reaches the OS key store.
        extensions: vec![Arc::new(CuaSpacesDaemon::with_test_presence(Arc::new(
            FakePresence::new(true),
        )))],
        ..Default::default()
    })
    .unwrap();
    let spaces = rt.spaces().clone();
    let macos = env("CUA_VOLUME_E2E_MACOS_IMAGE");
    let macos_bin = env("CUA_VOLUME_E2E_MACOS_SPACESD");
    let run = {
        let spaces = spaces.clone();
        async move {
            let (a, _) = create(&spaces, &linux, Some(4096)).await;
            let fa = round_trip(&spaces, &a, "linux").await;
            persistent_agent(&spaces, &a, &fa).await;
            if let Some(image) = macos {
                let (b, _) = create(&spaces, &image, Some(6144)).await;
                if let Some(bin) = macos_bin {
                    upgrade_macos_spacesd(&spaces, &b, Path::new(&bin)).await;
                }
                let fb = round_trip(&spaces, &b, "macos").await;
                // Across Spaces: public/ is shared; each Space's folder is
                // its own.
                let sa = spaces.space(&a).await.unwrap();
                let sb = spaces.space(&b).await.unwrap();
                let va = spaces.volume(&a).await.unwrap();
                let vb = spaces.volume(&b).await.unwrap();
                ok(&sa, &format!("cat '{}/public/macos.txt'", va.mount_path)).await;
                ok(&sb, &format!("cat '{}/public/linux.txt'", vb.mount_path)).await;
                let listed = ok(&sb, &format!("ls -1 '{}/spaces'", vb.mount_path)).await;
                assert_eq!(listed.trim(), fb, "B sees only its own folder");
                let (good, _, _) = sh(
                    &sb,
                    &format!("cat '{}/spaces/{fa}/from-guest.txt'", vb.mount_path),
                )
                .await;
                assert!(!good, "B read A's folder");
            }
        }
    };
    let outcome = tokio::spawn(run).await;
    // Delete what was created, pass or fail: deleting a Space unmounts its
    // volume first.
    for s in spaces.list().unwrap_or_default() {
        if s.id.starts_with("local:") {
            let t0 = Instant::now();
            let r = spaces.delete(&s.id).await;
            timing(&format!("{} deleted ({r:?})", s.id), t0);
        }
    }
    assert!(
        spaces.volumes().await.is_empty(),
        "a volume outlived its Space"
    );
    spaces.stop_drive_service().await;
    drop(rt);
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
