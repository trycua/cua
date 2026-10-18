//! Host setup's auth and network failures: which stage stopped it, the
//! HTTP status behind it, and the transient relay failures it rides out.

use crate::service::FakeServiceManager;
use crate::testing::FakeRelay;
use crate::*;
use std::sync::Arc;

struct Fixture {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    driver: std::path::PathBuf,
    manager: Arc<FakeServiceManager>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let driver = dir.path().join("bundled-cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    Fixture {
        home: dir.path().join(".cua"),
        driver,
        manager: Arc::new(FakeServiceManager::default()),
        _dir: dir,
    }
}

impl Fixture {
    fn host(&self) -> Host {
        Host::new(&self.home).with_service_manager(self.manager.clone())
    }
    fn opts(&self, url: &str) -> SetupOptions {
        let mut o = SetupOptions::relay(url);
        o.name = Some("mini".into());
        o.driver_bin = Some(self.driver.clone());
        o
    }
}

#[tokio::test]
async fn no_account_stops_at_the_token_stage_before_the_relay_is_called() {
    let relay = FakeRelay::start().await;
    let f = fixture();
    let (stage, e) = f
        .host()
        .setup_staged(f.opts(&relay.url), &NoAccount)
        .await
        .unwrap_err();
    assert_eq!(stage, SetupStage::Token);
    assert!(matches!(e, Error::Unauthenticated(_)), "{e}");
    assert_eq!(e.http_status(), None);
    assert_eq!(relay.register_calls(), 0);
    assert!(!f.host().status().await.unwrap().configured);
}

#[tokio::test]
async fn a_refused_account_token_is_a_401_at_the_register_stage() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    let f = fixture();
    let (stage, e) = f
        .host()
        .setup_staged(f.opts(&relay.url), &StaticToken("expired".into()))
        .await
        .unwrap_err();
    assert_eq!(stage, SetupStage::Register);
    assert!(matches!(e, Error::Unauthenticated(_)), "{e}");
    assert_eq!(e.http_status(), Some(401));
    // Not retried: the relay answered, and a new token is the fix.
    assert_eq!(relay.register_calls(), 1);
    // With a valid token the same setup goes through.
    assert!(
        f.host()
            .setup(f.opts(&relay.url), &StaticToken("good".into()))
            .await
            .unwrap()
            .configured
    );
}

#[tokio::test]
async fn a_relay_briefly_unavailable_is_retried_until_it_registers() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    relay.fail_registrations(&[503, 502, 429]);
    let f = fixture();
    let status = f
        .host()
        .setup(f.opts(&relay.url), &StaticToken("good".into()))
        .await
        .unwrap();
    assert!(status.configured && status.service.running);
    assert_eq!(relay.register_calls(), 4);
}

#[tokio::test]
async fn a_relay_hiccup_does_not_make_the_host_need_attention() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    let f = fixture();
    f.host()
        .setup(f.opts(&relay.url), &StaticToken("good".into()))
        .await
        .unwrap();
    // One or two 503s from the proxy in front of the relay: read again.
    relay.fail_reads(&[503, 503]);
    let status = f.host().status().await.unwrap();
    assert_eq!(status.error, None);
    assert!(status.sharing);
    // A deploy (every read 503 for a while): the relay's last answer.
    relay.set_reads_down(true);
    let status = f.host().status().await.unwrap();
    assert_eq!(status.error, None);
    assert!(status.sharing);
    // A refusal is not a hiccup: it shows at once.
    relay.set_reads_down(false);
    relay.fail_reads(&[500]);
    let status = f.host().status().await.unwrap();
    assert!(status.error.unwrap().contains("HTTP 500"));
}

#[tokio::test]
async fn a_relay_down_with_no_answer_to_fall_back_on_needs_attention() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    relay.set_reads_down(true);
    let f = fixture();
    // Setup registers (a POST); the status read after it has no answer yet.
    let status = f
        .host()
        .setup(f.opts(&relay.url), &StaticToken("good".into()))
        .await
        .unwrap();
    assert_eq!(
        status.error.as_deref(),
        Some("relay: HTTP 503: no available server")
    );
}

#[test]
fn relay_failures_that_pass_on_their_own_are_transient() {
    for (e, transient) in [
        (Error::Relay("HTTP 503: no available server".into()), true),
        (Error::Relay("HTTP 502: bad gateway".into()), true),
        (Error::Relay("HTTP 504: timeout".into()), true),
        (Error::Relay("HTTP 429: slow down".into()), true),
        (
            Error::Relay("http://127.0.0.1:1: error sending request".into()),
            true,
        ),
        (Error::Relay("HTTP 500: boom".into()), false),
        (Error::Relay("bad relay response: EOF".into()), false),
        (Error::Unauthenticated("relay: no".into()), false),
        (Error::NotFound("relay: no such machine".into()), false),
    ] {
        assert_eq!(e.is_transient(), transient, "{e}");
    }
}

#[tokio::test]
async fn a_relay_that_stays_down_fails_at_register_with_its_status() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    relay.fail_registrations(&[503; 10]);
    let f = fixture();
    let (stage, e) = f
        .host()
        .setup_staged(f.opts(&relay.url), &StaticToken("good".into()))
        .await
        .unwrap_err();
    assert_eq!(stage, SetupStage::Register);
    assert!(matches!(e, Error::Relay(_)), "{e}");
    assert_eq!(e.http_status(), Some(503));
    assert_eq!(relay.register_calls(), 4);
}

#[tokio::test]
async fn a_500_may_have_registered_so_it_is_not_sent_again() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    relay.fail_registrations(&[500]);
    let f = fixture();
    let (stage, e) = f
        .host()
        .setup_staged(f.opts(&relay.url), &StaticToken("good".into()))
        .await
        .unwrap_err();
    assert_eq!((stage, e.http_status()), (SetupStage::Register, Some(500)));
    assert_eq!(relay.register_calls(), 1);
}

#[tokio::test]
async fn an_unreachable_relay_is_retried_then_fails_with_no_status() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let f = fixture();
    let (stage, e) = f
        .host()
        .setup_staged(f.opts(&url), &StaticToken("good".into()))
        .await
        .unwrap_err();
    assert_eq!(stage, SetupStage::Register);
    assert!(matches!(e, Error::Relay(_)), "{e}");
    assert_eq!(e.http_status(), None);
}

#[tokio::test]
async fn a_missing_driver_fails_at_the_download_stage() {
    let relay = FakeRelay::start().await;
    relay.add_account("good", "user-1", None);
    let f = fixture();
    let mut o = f.opts(&relay.url);
    o.driver_bin = Some(f.home.join("no-such-cua-spacesd"));
    let (stage, _) = f
        .host()
        .setup_staged(o, &StaticToken("good".into()))
        .await
        .unwrap_err();
    assert_eq!(stage, SetupStage::Download);
}

#[test]
fn http_status_reads_relay_and_download_errors() {
    for (e, want) in [
        (Error::Relay("HTTP 503: busy".into()), Some(503)),
        (
            Error::Download("https://x/a.tar.gz: HTTP 404 Not Found".into()),
            Some(404),
        ),
        (
            Error::Relay("http://127.0.0.1:9: error sending request".into()),
            None,
        ),
        (
            Error::Unauthenticated("relay: invalid account token".into()),
            Some(401),
        ),
        (
            Error::Unauthenticated("not signed in to cua.ai".into()),
            None,
        ),
        (Error::PermissionDenied("relay: no".into()), Some(403)),
        (Error::Download("archive: HTTP 9".into()), None),
        (Error::Service("launchctl bootstrap failed".into()), None),
    ] {
        assert_eq!(e.http_status(), want, "{e}");
    }
    assert_eq!(SetupStage::Register.as_str(), "register");
}

#[tokio::test]
async fn a_pairing_code_sets_up_the_machine_once_and_a_failed_download_does_not_spend_it() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "ada", Some("ada@example.com"));
    let code = RelayClient::new(&relay.url)
        .unwrap()
        .create_pairing_code("acct-token", &PairingCodeRequest::default())
        .await
        .unwrap()
        .code
        .unwrap();
    let token = PairingCodeToken::parse(&code).unwrap();

    // The driver is missing: setup stops at the download, before the relay
    // is asked, so the code is still good.
    let f = fixture();
    let mut opts = f.opts(&relay.url);
    opts.download_first = true;
    opts.driver_bin = Some(f.home.join("missing-spacesd"));
    let (stage, _) = f.host().setup_staged(opts, &token).await.unwrap_err();
    assert_eq!(stage, SetupStage::Download);
    assert_eq!(relay.register_calls(), 0);

    // With the driver: registered as the account that made the code.
    let mut opts = f.opts(&relay.url);
    opts.download_first = true;
    let status = f.host().setup(opts, &token).await.unwrap();
    assert!(status.configured);
    let id = f.host().config().unwrap().unwrap().machine_id.unwrap();
    assert_eq!(relay.machine(&id).unwrap().owner.id, "ada");

    // Spent: another machine cannot use it.
    let g = fixture();
    let mut opts = g.opts(&relay.url);
    opts.download_first = true;
    let (stage, e) = g.host().setup_staged(opts, &token).await.unwrap_err();
    assert_eq!(stage, SetupStage::Register);
    assert_eq!(e.http_status(), Some(401), "{e}");
}
