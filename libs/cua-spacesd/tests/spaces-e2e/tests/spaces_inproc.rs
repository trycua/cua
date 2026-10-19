// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spaces primitives against a real cua-spacesd server core, in-process
//! (see `common` for the host-safety rules these tests follow).

mod common;

use common::{TOKEN, added, driver, spaces, with_isolated_cua_home};
use cua_spaces::agents::RunOptions;
use cua_spaces::files::SendFileOptions;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn an_os_override_is_the_os_the_space_reports() {
    // The docs fixture stands in for a Linux Space on any host.
    let d = cua_spaces_e2e::driver_with(|c| c.os_override = Some("linux".into())).await;
    let reg = tempfile::tempdir().unwrap();
    let info = spaces(reg.path())
        .add(&d.url, Some(TOKEN.into()), None)
        .await
        .unwrap();
    assert_eq!(info.os, "linux");
}

#[tokio::test]
async fn add_persists_the_space_and_keeps_the_token_out_of_spaces_json() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let info = s.add(&d.url, Some(TOKEN.into()), None).await.unwrap();
    assert!(info.id.starts_with("direct:127.0.0.1:"), "{}", info.id);
    assert!(info.features.contains(&"driver".to_string()));
    assert!(info.features.contains(&"teleport.firefox".to_string()));
    // The OS comes from the GetCapabilities handshake (the in-process
    // driver reports the host's).
    let host_os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    assert_eq!(info.os, host_os);
    let json = std::fs::read_to_string(reg.path().join("spaces.json")).unwrap();
    assert!(!json.contains(TOKEN));
    let creds = std::fs::read_to_string(reg.path().join("spaces-credentials.json")).unwrap();
    assert!(creds.contains(TOKEN));

    // A fresh runtime over the same registry reconnects with the stored token.
    let again = spaces(reg.path());
    assert_eq!(again.list().unwrap().len(), 1);
    assert_eq!(
        again.list().unwrap()[0].os,
        host_os,
        "persisted in the registry"
    );
    let space = again.space(&info.id).await.unwrap();
    assert!(space.supports("driver"));
    // Names resolve too.
    assert_eq!(again.resolve(&info.name).unwrap().to_string(), info.id);

    // Remove forgets the Space and its token.
    again.remove(&info.id).await.unwrap();
    assert!(again.list().unwrap().is_empty());
    let creds = std::fs::read_to_string(reg.path().join("spaces-credentials.json")).unwrap();
    assert!(!creds.contains(TOKEN));
}

#[tokio::test]
async fn a_wrong_token_and_a_non_driver_are_refused_distinctly() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let e = s.add(&d.url, Some("wrong".into()), None).await.unwrap_err();
    assert_eq!(e.tag(), "unauthenticated", "{e}");

    // A plain HTTP server is not a Space.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Bounded: answers a handful of probes, then stops.
        for _ in 0..8 {
            let Ok((mut c, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = tokio::time::timeout(Duration::from_secs(2), c.read(&mut buf)).await;
            let _ = c
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        }
    });
    let e = s
        .add(&format!("http://{addr}"), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "spacesd_not_available", "{e}");
    assert!(s.list().unwrap().is_empty());
}

#[tokio::test]
async fn bash_write_upload_and_download_round_trip() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let space = added(&s, &d).await;

    let out = space
        .bash("echo hi; echo err >&2; exit 3", Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(out.render(), "hi\n[stderr]\nerr\n[exit 3]");

    let target = d.home.join("notes/a.txt");
    let w = space
        .write(target.to_str().unwrap(), "it's \"quoted\" $(not run)\n")
        .await
        .unwrap();
    assert_eq!(w.bytes, "it's \"quoted\" $(not run)\n".len() as u64);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "it's \"quoted\" $(not run)\n"
    );

    // Upload a folder (with an empty dir and a 3 MiB file: several chunks).
    let src = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    std::fs::create_dir_all(src.path().join("proj/empty")).unwrap();
    std::fs::write(src.path().join("proj/big.bin"), &big).unwrap();
    std::fs::write(src.path().join("proj/.gitignore"), "*.bin\n").unwrap();
    let up = space.upload(&src.path().join("proj"), None).await.unwrap();
    assert_eq!(up.kind, "folder");
    assert_eq!(
        up.files, 2,
        "upload sends everything, ignore files included"
    );
    assert_eq!(std::fs::read(d.home.join("proj/big.bin")).unwrap(), big);
    assert!(d.home.join("proj/empty").is_dir());

    // Download it back, file and folder.
    let dest = tempfile::tempdir().unwrap();
    let f = space
        .download(d.home.join("proj/big.bin").to_str().unwrap(), dest.path())
        .await
        .unwrap();
    assert_eq!(f.sha256.as_deref(), Some(sha(&big).as_str()));
    let dir = space
        .download(d.home.join("proj").to_str().unwrap(), dest.path())
        .await
        .unwrap();
    assert_eq!((dir.kind, dir.files), ("folder", 2));
    assert_eq!(
        std::fs::read(dest.path().join("proj/big.bin")).unwrap(),
        big
    );
    assert!(dest.path().join("proj/empty").is_dir());
}

#[tokio::test]
async fn send_file_lands_in_downloads_verified_and_honors_ignore_files() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let space = added(&s, &d).await;

    let src = tempfile::tempdir().unwrap();
    let root = src.path().join("app");
    for (rel, body) in [
        (".gitignore", "target/\n*.log\n!keep.log\n"),
        ("src/main.rs", "fn main() {}\n"),
        ("target/debug/app", "binary"),
        ("run.log", "noise"),
        ("keep.log", "kept"),
        (".git/HEAD", "ref: x"),
        (".dockerignore", "secret.env\n"),
        ("secret.env", "TOKEN=1"),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let r = space
        .send_file(
            &root,
            SendFileOptions {
                subdir: "inbox".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(r.verified);
    assert_eq!(r.kind, "folder");
    let landed = d.downloads.join("inbox/app");
    assert_eq!(
        std::fs::read_to_string(landed.join("src/main.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert!(landed.join("keep.log").exists());
    for gone in ["target", "run.log", ".git", "secret.env"] {
        assert!(!landed.join(gone).exists(), "{gone} must be skipped");
    }
    assert_eq!(
        r.skipped_by_ignorefiles,
        vec![".git/", "run.log", "secret.env", "target/"]
    );
    for f in &r.files {
        let bytes = std::fs::read(&f.path).unwrap();
        assert_eq!(sha(&bytes), f.sha256, "{}", f.path);
    }

    // A single file: respect_ignorefiles never applies to it.
    let one = src.path().join("run.log");
    std::fs::write(&one, "explicit").unwrap();
    let r = space
        .send_file(&one, SendFileOptions::default())
        .await
        .unwrap();
    assert_eq!(r.kind, "file");
    assert_eq!(
        std::fs::read_to_string(d.downloads.join("run.log")).unwrap(),
        "explicit"
    );
}

#[tokio::test]
async fn driver_tools_and_capability_gates() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let space = added(&s, &d).await;

    let (tools, _) = space.list_tools(None).await.unwrap();
    assert!(tools.iter().any(|t| t.name == "get_screen_size"));
    let r = space
        .call_tool(Some("mcp"), "get_screen_size", Default::default(), None)
        .await
        .unwrap();
    assert!(!r.is_error);
    assert_eq!(r.content[0]["text"], "1280x800");
    assert_eq!(
        space.list_tools(Some("blender")).await.unwrap_err().tag(),
        "not_found"
    );

    // No desktop provider in this server: streams are refused up front,
    // naming the feature and quoting the driver's limitation.
    let e = space
        .open_stream(
            cua_spaces::stream::StreamTarget::Display(None),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "capability_missing");
    assert!(e.to_string().contains("desktop_stream"), "{e}");
    assert_eq!(
        space
            .join_presence(Default::default(), Duration::from_secs(5))
            .await
            .unwrap_err()
            .tag(),
        "capability_missing"
    );
}

#[tokio::test]
async fn teleport_moves_a_synthetic_firefox_profile() {
    use cua_spaces_ext::teleport::providers::{ExportRegistry, FakeHost, FirefoxProvider};
    use cua_spaces_ext::teleport::{AppSessions, ImportOptions, SpaceTeleport as _, TeleportScope};

    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let space = added(&s, &d).await;

    // A generated profile in a temp dir, read through a FakeHost: never the
    // real Firefox profile, never the real keychain.
    let host_home = tempfile::tempdir().unwrap();
    let profile = host_home.path().join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        profile.join("prefs.js"),
        "user_pref(\"cua.test.marker\", \"teleported\");\n",
    )
    .unwrap();
    std::fs::write(profile.join("cookies.sqlite"), b"not-a-real-db").unwrap();
    std::fs::write(profile.join("places.sqlite"), b"history").unwrap();
    let host = Arc::new(FakeHost::new().with_home(host_home.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        FirefoxProvider::new()
            .with_host(host.clone())
            .with_profile_dir(&profile),
    ));
    let sessions = Arc::new(AppSessions::from_registry(registry));

    let manifest = sessions.manifest("firefox", TeleportScope::Full).unwrap();
    assert!(!manifest.items.is_empty());
    let id = space.id().to_string();
    // Cookies (sign-ins) are sensitive and opt-in, never default-checked:
    // the default set alone never needs `acknowledge_sensitive`.
    let cookies_item = manifest
        .items
        .iter()
        .find(|i| i.relative_path.ends_with("cookies.sqlite"))
        .expect("the fixture's cookies.sqlite is in the manifest");
    assert!(cookies_item.is_sensitive, "{cookies_item:?}");
    assert!(!cookies_item.is_checked_by_default, "{cookies_item:?}");
    assert!(
        manifest.default_selection().iter().all(|i| !i.is_sensitive),
        "no sensitive item is ever ticked by default"
    );
    let cookies_path = cookies_item.relative_path.clone();
    manifest.approving_default(&id, false).unwrap();
    // Explicitly selecting it without acknowledging needs consent.
    assert_eq!(
        manifest
            .approving(&id, std::slice::from_ref(&cookies_path), false)
            .unwrap_err()
            .tag(),
        "teleport_refused"
    );
    let approval = manifest.approving(&id, &[cookies_path], true).unwrap();

    // `Space::teleport` always routes through the caller's Cua Keyvault now
    // (never a direct, non-Keyvault upload); this harness (`common::spaces`)
    // wires no Keyvault extension and serves no `keyvault.sock`, so the only
    // thing to verify here is that it fails closed rather than silently
    // falling back to uploading the bundle itself -- full signed-in
    // delivery through a real Keyvault is
    // `cua-spaces-ext/tests/spaces_daemon_boundary.rs`'s job. Isolate
    // `$CUA_HOME` first: the Keyvault client reaches the real, per-OS-user
    // socket unless overridden (never the developer's own).
    with_isolated_cua_home(reg.path(), async {
        let err = space
            .teleport(sessions.clone(), &approval, ImportOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.tag(), "host_capability_missing", "{err:?}");

        // An approval for another Space is refused before ever reaching the
        // Keyvault.
        let other = manifest
            .approving_default("space://direct/elsewhere:1", true)
            .unwrap();
        assert_eq!(
            space
                .teleport(sessions, &other, ImportOptions::default())
                .await
                .unwrap_err()
                .tag(),
            "teleport_refused"
        );
    })
    .await;
}

#[tokio::test]
async fn hotspot_egress_goes_through_this_host() {
    use cua_spaces::hotspot::{Dialer, HotspotOptions};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let space = added(&s, &d).await;

    // The "internet" this host can reach: one loopback HTTP server.
    let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_port = web.local_addr().unwrap().port();
    tokio::spawn(async move {
        for _ in 0..4 {
            let Ok((mut c, _)) = web.accept().await else {
                return;
            };
            let mut buf = [0u8; 2048];
            let _ = c.read(&mut buf).await;
            let _ = c
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 12\r\nconnection: close\r\n\r\nvia-hotspot!")
                .await;
        }
    });
    // Host-safe dialer: only that loopback port.
    let dialer: Dialer = Arc::new(move |host: String, port: u16| {
        Box::pin(async move {
            if port != web_port {
                return Err(std::io::Error::other(format!(
                    "test dialer refuses {host}:{port}"
                )));
            }
            tokio::net::TcpStream::connect(("127.0.0.1", port)).await
        })
    });
    let socks = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let hotspot = space
        .start_hotspot(HotspotOptions {
            socks_port: u32::from(socks),
            set_system_proxy: false,
            bypass: vec![],
            dialer,
        })
        .await
        .unwrap();
    assert!(hotspot.socks_address().ends_with(&format!(":{socks}")));

    // Speak SOCKS5 to the guest-side listener, as a guest app would.
    let mut ok = false;
    for _ in 0..20 {
        if let Ok(mut c) = tokio::net::TcpStream::connect(("127.0.0.1", socks)).await {
            c.write_all(&[5, 1, 0]).await.unwrap();
            let mut hello = [0u8; 2];
            c.read_exact(&mut hello).await.unwrap();
            let mut req = vec![5, 1, 0, 3, 9];
            req.extend_from_slice(b"127.0.0.1");
            req.extend_from_slice(&web_port.to_be_bytes());
            c.write_all(&req).await.unwrap();
            let mut reply = [0u8; 10];
            c.read_exact(&mut reply).await.unwrap();
            if reply[1] != 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            c.write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
                .await
                .unwrap();
            let mut body = Vec::new();
            let _ =
                tokio::time::timeout(Duration::from_secs(5), c.take(4096).read_to_end(&mut body))
                    .await;
            ok = String::from_utf8_lossy(&body).contains("via-hotspot!");
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok, "egress through the hotspot");
    let status = hotspot.status().await.unwrap();
    assert!(status.served_here);
    hotspot.stop().await.unwrap();
    assert_eq!(space.hotspot_status().await.unwrap().state, "stopped");
}

/// Agent runs speak the Agent Client Protocol through cua-agents' runner, so
/// a fake CLI can no longer stand in for a model. This covers what needs no
/// model: the runner attaches to the Space, the roster reads, and runs that
/// do not exist are NotFound (never "done"). Real harness runs against a
/// scripted mock provider: `cargo test -p cua-agents --test e2e_live`
/// (libs/cua/crates/cua-agents/tests/e2e/run-agents-e2e.sh).
#[tokio::test]
async fn the_agent_runner_attaches_and_reports_the_truth_without_a_model() {
    let d = driver().await;
    let reg = tempfile::tempdir().unwrap();
    let s = spaces(reg.path());
    let space = added(&s, &d).await;
    let agents = space.agents().await.unwrap();
    // The runner reads the confined guest home, never the real one.
    assert_eq!(
        std::path::Path::new(agents.home()),
        d.home.as_path(),
        "{}",
        agents.home()
    );
    assert!(agents.list().await.unwrap().is_empty());
    let ghost = agents.status("run-00000000").await.unwrap_err();
    assert!(
        matches!(
            cua_spaces::Error::from(ghost),
            cua_spaces::Error::NotFound(_)
        ),
        "an unknown run is NotFound"
    );
    let bad = agents.status("../etc").await.unwrap_err();
    assert!(
        matches!(
            cua_spaces::Error::from(bad),
            cua_spaces::Error::InvalidArgument(_)
        ),
        "a run id cannot name a path"
    );
    let send = agents
        .send("run-00000000", "hello", vec![])
        .await
        .unwrap_err();
    assert!(matches!(
        cua_spaces::Error::from(send),
        cua_spaces::Error::NotFound(_)
    ));
    // An unknown harness is refused before anything is written.
    assert!(
        agents
            .start("no-such-agent", "hi", RunOptions::default())
            .await
            .is_err()
    );
    assert!(agents.list().await.unwrap().is_empty());
}
