// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Full relay account flow against Docker (run through
//! `tests/e2e/run-relay-account-e2e.sh`): a fake OIDC issuer, `cua-relay` in
//! account mode, and a linux host joined in account mode with no
//! published ports. The script signs in with `cua auth login` (temp HOME,
//! file credential store) and exports:
//!
//! - `CUA_RELAY_E2E_URL`            relay base URL (published on loopback)
//! - `CUA_RELAY_E2E_CREDENTIALS`    the credentials.json `cua auth login` wrote
//! - `CUA_RELAY_E2E_MACHINE`        the host's machine id
//! - `CUA_RELAY_E2E_MACHINE_TOKEN`  the host's machine token (host-side
//!   "stop sharing", the call `cua host stop` makes)
//!
//! Without them the test is skipped.

use cua_spaces::Spaces;
use cua_spaces::presence::Identity;
use cua_spaces::relay::{RelayAccount, StaticToken};
use cua_spaces::stream::{FrameSink, StreamEvent, StreamOptions, StreamTarget, VideoFrame};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
struct Frames {
    frames: AtomicU64,
    keyframes: AtomicU64,
    closed: AtomicU64,
}

impl FrameSink for Frames {
    fn on_frame(&self, f: VideoFrame) {
        self.frames.fetch_add(1, Ordering::SeqCst);
        if f.keyframe {
            self.keyframes.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn on_event(&self, e: StreamEvent) {
        if matches!(e, StreamEvent::Closed { .. }) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
async fn relay_account_flow() {
    cua_spaces_ext::stream::register();
    let (Some(url), Some(creds), Some(machine), Some(machine_token)) = (
        env("CUA_RELAY_E2E_URL"),
        env("CUA_RELAY_E2E_CREDENTIALS"),
        env("CUA_RELAY_E2E_MACHINE"),
        env("CUA_RELAY_E2E_MACHINE_TOKEN"),
    ) else {
        eprintln!("skipped: run tests/e2e/run-relay-account-e2e.sh");
        return;
    };
    // Signed in by `cua auth login` against the fake issuer.
    let creds: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&creds).expect("credentials")).unwrap();
    let token = creds["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned();

    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .download_dir(home.path().join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(30))
        .relay(RelayAccount::new(url.clone(), Arc::new(StaticToken(token))))
        .build();

    // 1. The account's machines include the host, online.
    let mut listed = None;
    for _ in 0..60 {
        let machines = spaces.relay_machines().await.expect("relay_machines");
        if let Some(m) = machines.into_iter().find(|m| m.id == machine && m.online) {
            listed = Some(m);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let m = listed.expect("the host never appeared online in relay_machines()");
    assert_eq!(m.role, "owner");
    assert!(
        spaces
            .list()
            .unwrap()
            .iter()
            .any(|s| s.id == format!("relay:{machine}")),
        "list() includes the relay machine"
    );

    // 2. Connect and run a command (no env token anywhere on this side).
    let space = spaces
        .space(&format!("relay:{machine}"))
        .await
        .expect("connect through the relay");
    let out = space
        .bash("echo relay-ok; id -un", Duration::from_secs(60))
        .await
        .unwrap();
    assert!(out.success(), "{}", out.render());
    assert!(out.stdout.starts_with("relay-ok"), "{}", out.stdout);

    // 3. Desktop stream through the relay: a keyframe arrives.
    let frames = Arc::new(Frames::default());
    let session = space
        .stream_session(
            StreamTarget::Display(None),
            StreamOptions {
                max_fps: 5,
                max_dimension: 800,
                ..Default::default()
            },
            frames.clone(),
            None,
        )
        .await
        .expect("open desktop stream");
    for _ in 0..300 {
        if frames.keyframes.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        frames.keyframes.load(Ordering::SeqCst) > 0,
        "no keyframe through the relay"
    );

    // 4. Presence shows the account user: the relay-asserted identity wins
    //    over the id the client asks for.
    let presence = space
        .join_presence(
            Identity {
                id: "someone-else".into(),
                display_name: "Impostor".into(),
                ..Default::default()
            },
            Duration::from_secs(20),
        )
        .await
        .expect("join presence");
    let me = presence.me().clone();
    assert_eq!(
        me.principal_id, "ada",
        "presence principal is not the account user: {me:?}"
    );
    assert_ne!(me.display_name, "Impostor", "{me:?}");
    let relay = cua_host::relay::RelayClient::new(&url).unwrap();
    let with_clients = relay.machine(&machine_token, &machine).await.unwrap();
    assert!(
        with_clients.clients.iter().any(|c| c.id == "ada"),
        "relay presence: {:?}",
        with_clients.clients
    );

    // 5. Host-side stop sharing: the open stream is cut and new calls fail.
    let stopped = relay.stop_sharing(&machine_token, &machine).await.unwrap();
    assert!(!stopped.sharing);
    for _ in 0..100 {
        if frames.closed.load(Ordering::SeqCst) > 0 || session.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        frames.closed.load(Ordering::SeqCst) > 0 || session.is_closed(),
        "desktop stream survived stop-sharing"
    );
    let refused = space.bash("echo still-here", Duration::from_secs(20)).await;
    assert!(
        refused.is_err(),
        "command ran after stop-sharing: {refused:?}"
    );
    let _ = spaces.forget_connection(&format!("relay:{machine}")).await;
    let reconnect = spaces.space(&format!("relay:{machine}")).await;
    assert!(reconnect.is_err(), "reconnected after stop-sharing");
    drop(presence);
    // Leave the machine shareable for reruns.
    relay.start_sharing(&machine_token, &machine).await.unwrap();
}
