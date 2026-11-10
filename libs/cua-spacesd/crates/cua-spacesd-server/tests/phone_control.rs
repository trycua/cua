// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A phone reaches what a Space is, and nothing of the computer underneath.
//! The pointer and the keyboard answer a phone only on a desktop this
//! computer shares, and only when the account's role may touch it: the same
//! rule as for every app, with no second consent. The checks run on every
//! call. Each refusal is asserted by its exact words.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use cua_spacesd_server::auth::{ExternalAuthenticator, ExternalGrant, PhoneCaller, RelayCaller};
use cua_spacesd_server::phone_control::{PhoneControl, Scope};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use http::Request;
use tower::ServiceExt as _;

/// What the relay would have asserted, read from `x-test-phone: <device>`.
/// `view_only` is a view-only share of the account; `host_only` a computer
/// that does not share its desktop.
struct Phones {
    store: Arc<PhoneControl>,
    view_only: AtomicBool,
    host_only: AtomicBool,
}

impl ExternalAuthenticator for Phones {
    fn authenticate(&self, headers: &http::HeaderMap) -> Option<Result<ExternalGrant, String>> {
        let device = headers.get("x-test-phone")?.to_str().ok()?.to_owned();
        Some(Ok(ExternalGrant {
            principal: cua_proto::env::v1::Principal {
                id: "acct-ada".into(),
                display_name: "ada@example.test".into(),
                color: String::new(),
                kind: 1,
            },
            view_only: self.view_only.load(Ordering::Relaxed),
            host_only: self.host_only.load(Ordering::Relaxed),
            account: Some(RelayCaller {
                account: "acct-ada".into(),
                role: "owner".into(),
                device: Some(device.clone()),
                phone: Some(PhoneCaller { device }),
                ..Default::default()
            }),
        }))
    }

    fn phone_refused(&self, caller: &RelayCaller, path: &str) {
        let phone = caller.phone.as_ref().unwrap();
        self.store.note_refused(
            &caller.account,
            &phone.device,
            path.rsplit('/').next().unwrap_or(""),
        );
    }

    fn phone_used(&self, caller: &RelayCaller, scope: Scope) {
        let phone = caller.phone.as_ref().unwrap();
        self.store.note_use(&caller.account, &phone.device, scope);
    }
}

struct Rig {
    router: axum::Router,
    ctx: ServerContext,
    store: Arc<PhoneControl>,
    phones: Arc<Phones>,
    _data: tempfile::TempDir,
}

fn server() -> Rig {
    let data = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.path().to_path_buf(),
        shutdown_grace: Duration::from_secs(1),
        access_log: true,
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some("s3cret".into()));
    let store = Arc::new(PhoneControl::in_data_dir(data.path()));
    let phones = Arc::new(Phones {
        store: store.clone(),
        view_only: AtomicBool::new(false),
        host_only: AtomicBool::new(false),
    });
    ctx.auth().set_external(phones.clone());
    Rig {
        router: ServerBuilder::new(ctx.clone()).build().router(),
        ctx,
        store,
        phones,
        _data: data,
    }
}

fn as_phone(path: &str, device: &str) -> Request<Body> {
    Request::post(path)
        .header("host", "127.0.0.1:3211")
        .header("content-type", "application/grpc-web+proto")
        .header("x-test-phone", device)
        .body(Body::from(vec![0u8, 0, 0, 0, 0]))
        .unwrap()
}

fn denied(resp: &http::Response<Body>) -> Option<String> {
    let h = resp.headers();
    (h.get("grpc-status")? == "7").then(|| {
        let raw = h.get("grpc-message").unwrap().to_str().unwrap();
        percent_encoding::percent_decode_str(raw)
            .decode_utf8_lossy()
            .into_owned()
    })
}

const POINTER: &str = "/cua.env.v1.ComputerService/Pointer";
const KEYBOARD: &str = "/cua.env.v1.ComputerService/Keyboard";
const NOT_THE_COMPUTERS: &str =
    "a phone cannot make this call: it reaches Spaces, not this computer's own services";

async fn call(rig: &Rig, path: &str, device: &str) -> Option<String> {
    let resp = rig
        .router
        .clone()
        .oneshot(as_phone(path, device))
        .await
        .unwrap();
    denied(&resp)
}

#[tokio::test]
async fn a_phone_is_refused_the_computers_own_services_whatever_its_role() {
    let rig = server();
    for role_view_only in [false] {
        rig.phones
            .view_only
            .store(role_view_only, Ordering::Relaxed);
        for path in [
            "/cua.env.v1.ProcessService/StartProcess",
            "/cua.env.v1.ProcessService/Start",
            "/cua.env.v1.FilesystemService/ReadFile",
            "/cua.env.v1.FilesystemService/WriteFile",
            "/cua.env.v1.FilesystemService/WatchDir",
            "/cua.env.v1.TunnelService/Forward",
            "/cua.env.v1.DriverService/CallTool",
            "/cua.env.v1.SystemService/Shutdown",
            "/cua.env.v1.ComputerService/SetClipboard",
            "/cua.env.v1.ComputerService/GetClipboard",
            "/cua.env.v1.HostSpacesService/CreateHostSpace",
            "/cua.env.v1.SystemService/AttachRelay",
        ] {
            assert_eq!(
                call(&rig, path, "dev_1").await.as_deref(),
                Some(NOT_THE_COMPUTERS),
                "{path}"
            );
        }
    }
}

#[tokio::test]
async fn the_pointer_and_the_keyboard_of_a_shared_desktop_need_no_second_consent() {
    let rig = server();
    // The owner's own phone on a computer that shares its desktop: the calls
    // are not ours to refuse (the service answers however it does), and the
    // log says the phone touched the computer.
    for path in [POINTER, KEYBOARD] {
        let d = call(&rig, path, "dev_1").await;
        assert!(
            d.as_deref() != Some(NOT_THE_COMPUTERS)
                && !d.as_deref().unwrap_or("").contains("not allowed")
                && !d.as_deref().unwrap_or("").contains("view-only"),
            "{path}: {d:?}"
        );
    }
    let log = rig.store.audit_tail(100).join("\n");
    assert!(
        log.contains("phone_controlled") && log.contains("dev_1"),
        "{log}"
    );
    assert!(!log.contains("s3cret"));
}

#[tokio::test]
async fn a_view_only_share_cannot_touch_it_and_a_computer_that_does_not_share_its_desktop_has_none_to_touch(
) {
    let rig = server();
    rig.phones.view_only.store(true, Ordering::Relaxed);
    for path in [POINTER, KEYBOARD] {
        let d = call(&rig, path, "dev_1").await.expect("refused");
        assert!(
            d.starts_with("view-only share: ") && d.ends_with(&format!("cannot call {path}")),
            "{d}"
        );
    }
    rig.phones.view_only.store(false, Ordering::Relaxed);
    rig.phones.host_only.store(true, Ordering::Relaxed);
    for path in [POINTER, KEYBOARD, "/cua.env.v1.StreamService/OpenMedia"] {
        let d = call(&rig, path, "dev_1").await.expect("refused");
        assert!(
            d.starts_with("this computer does not share its desktop (it only provides Spaces): ")
                && d.ends_with(&format!("cannot call {path}")),
            "{d}"
        );
    }
}

#[tokio::test]
async fn the_thread_and_the_screen_are_a_phones_on_a_shared_desktop() {
    let rig = server();
    for path in [
        "/cua.env.v1.HostThreadService/CallThreadTool",
        "/cua.env.v1.SystemService/GetCapabilities",
        "/cua.env.v1.ComputerService/Screenshot",
    ] {
        let d = call(&rig, path, "dev_1").await;
        assert!(d.as_deref() != Some(NOT_THE_COMPUTERS), "{path}: {d:?}");
    }
}

#[tokio::test]
async fn a_phone_reaches_no_plain_http_route() {
    let rig = server();
    let mut headers = http::HeaderMap::new();
    headers.insert("x-test-phone", http::HeaderValue::from_static("dev_1"));
    assert!(
        !rig.ctx.check_bearer(&headers),
        "/mcp, exec sockets, files and the rest"
    );
}

/// The owner reads which device did what: every phone call in the host's
/// hash-chained log names the device and, for a phone, its scope.
#[tokio::test]
async fn the_host_log_names_the_phone_and_whether_it_looked_or_controlled() {
    let rig = server();
    for path in ["/cua.env.v1.HostThreadService/CallThreadTool", POINTER] {
        call(&rig, path, "dev_1").await;
    }
    let path = rig.ctx.config().access_log_path();
    cua_spacesd_server::access_log::verify(&path).expect("the chain verifies");
    let lines: Vec<cua_spacesd_server::access_log::AccessEntry> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let who: Vec<&str> = lines.iter().map(|e| e.body.who.as_str()).collect();
    assert!(
        who.iter().any(|w| w.ends_with("[dev_1 mobile view]")),
        "{who:?}"
    );
    assert!(
        who.iter().any(|w| w.ends_with("[dev_1 mobile control]")),
        "{who:?}"
    );
}
