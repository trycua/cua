//! Linking a signed-in install's usage data to its opaque account id
//! (CUA-1881): only with usage data on, only for an id the server minted,
//! never before the machine showed the linking notice, never from the
//! process that showed it, and undone by signing out, the opt-out or the
//! server no longer offering it.

use cua_telemetry::events::{self, Outcome};
use cua_telemetry::sink::MemorySink;
use cua_telemetry::{LinkState, NoticeMode, Telemetry, link, notice};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const ACCT: &str = "acct_0123456789abcdef0123456789abcdef";
const OTHER: &str = "acct_fedcba9876543210fedcba9876543210";

fn client(home: &Path, mode: NoticeMode) -> (Telemetry, Arc<MemorySink>) {
    let sink = Arc::new(MemorySink::new());
    let t = Telemetry::builder()
        .env(|k| (k == "CUA_TELEMETRY_FORBID_NETWORK").then(|| "1".into()))
        .home(home)
        .sink(sink.clone())
        .product("cli", "1.2.3")
        .notice_mode(mode)
        .foreground()
        .build();
    (t, sink)
}

fn send(t: &Telemetry) {
    t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t.flush(Duration::from_secs(1));
}

fn install_id(home: &Path) -> String {
    std::fs::read_to_string(home.join("telemetry").join("install_id"))
        .unwrap()
        .trim()
        .to_string()
}

fn named<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["event"] == name).collect()
}

/// An install that showed the old "anonymous" notice before this release.
fn existing_install(home: &Path) {
    let dir = home.join("telemetry");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(notice::MARKER), "1\n").unwrap();
}

#[test]
fn without_an_offer_events_stay_on_the_install_id() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    send(&t);
    let ev = sink.events();
    assert!(named(&ev, "$identify").is_empty());
    assert_eq!(ev[0]["distinct_id"], install_id(h.path()));
    assert_eq!(ev[0]["properties"]["$process_person_profile"], false);
    assert_eq!(t.link_state(), LinkState::NotOffered);
}

#[test]
fn a_new_install_links_once_signed_in() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    // The first-run notice says signed-in usage is linked.
    t.acknowledge_notice();
    send(&t);
    let anon = install_id(h.path());
    assert_eq!(t.offer_account(Some(ACCT)), LinkState::Linked);
    assert!(!t.link_notice_due());
    send(&t);
    send(&t);
    let ev = sink.events();
    let ids = named(&ev, "$identify");
    assert_eq!(ids.len(), 1, "one $identify: {ev:?}");
    assert_eq!(ids[0]["distinct_id"], ACCT);
    assert_eq!(ids[0]["properties"]["$anon_distinct_id"], anon.as_str());
    let deleted = named(&ev, "cua_sandbox_deleted");
    assert_eq!(deleted[0]["distinct_id"], anon.as_str());
    for e in &deleted[1..] {
        assert_eq!(e["distinct_id"], ACCT);
        assert_eq!(e["properties"]["$process_person_profile"], true);
    }
    assert_eq!(t.status().link, "linked");
}

#[test]
fn an_existing_install_waits_for_the_one_time_notice() {
    let h = tempfile::tempdir().unwrap();
    existing_install(h.path());
    let (t, sink) = client(h.path(), NoticeMode::External);
    assert_eq!(t.offer_account(Some(ACCT)), LinkState::NoticeDue);
    assert!(t.link_notice_due());
    send(&t);
    let ev = sink.events();
    assert!(
        named(&ev, "$identify").is_empty(),
        "never linked before the notice"
    );
    assert_eq!(ev[0]["distinct_id"], install_id(h.path()));

    // The app showed it and the person closed it.
    t.acknowledge_link_notice();
    assert!(!t.link_notice_due());
    send(&t);
    let ev = sink.events();
    assert_eq!(named(&ev, "$identify").len(), 1);
    assert_eq!(ev.last().unwrap()["distinct_id"], ACCT);
}

#[test]
fn the_cli_prints_the_notice_and_that_process_links_nothing() {
    let h = tempfile::tempdir().unwrap();
    existing_install(h.path());
    let (t, sink) = client(h.path(), NoticeMode::Stderr);
    t.offer_account(Some(ACCT));
    assert!(t.show_notice_if_needed(), "prints the linking notice");
    assert!(!t.show_notice_if_needed(), "once");
    send(&t);
    let ev = sink.events();
    assert!(named(&ev, "$identify").is_empty());
    assert_eq!(ev[0]["distinct_id"], install_id(h.path()));

    // The next command links.
    let (t2, sink2) = client(h.path(), NoticeMode::Stderr);
    assert!(!t2.show_notice_if_needed());
    send(&t2);
    let ev = sink2.events();
    assert_eq!(named(&ev, "$identify").len(), 1);
    assert_eq!(ev.last().unwrap()["distinct_id"], ACCT);
}

#[test]
fn the_opt_out_wins_and_unlinks() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    t.offer_account(Some(ACCT));
    send(&t);
    t.set_enabled(false).unwrap();
    assert_eq!(t.link_state(), LinkState::Off);
    assert_eq!(t.offer_account(Some(ACCT)), LinkState::Off);
    let dir = h.path().join("telemetry");
    assert!(!dir.join(link::ACCOUNT_FILE).exists());
    assert!(!dir.join(link::LINKED_FILE).exists());
    let before = sink.events().len();
    send(&t);
    assert_eq!(sink.events().len(), before, "nothing sent while off");

    let off = tempfile::tempdir().unwrap();
    let sink = Arc::new(MemorySink::new());
    let t = Telemetry::builder()
        .env(|k| match k {
            "CUA_TELEMETRY_FORBID_NETWORK" => Some("1".into()),
            "DO_NOT_TRACK" => Some("1".into()),
            _ => None,
        })
        .home(off.path())
        .sink(sink.clone())
        .foreground()
        .build();
    t.acknowledge_notice();
    assert_eq!(t.offer_account(Some(ACCT)), LinkState::Off);
    send(&t);
    assert!(sink.events().is_empty());
    assert!(
        !off.path()
            .join("telemetry")
            .join(link::ACCOUNT_FILE)
            .exists()
    );
}

#[test]
fn only_server_minted_ids_are_linked() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    for bad in [
        "alice@example.com",
        "f3c1a2b4-1111-2222-3333-444455556666",
        "acct_XYZ",
    ] {
        assert_eq!(t.offer_account(Some(bad)), LinkState::NotOffered, "{bad}");
    }
    send(&t);
    let ev = sink.events();
    assert!(named(&ev, "$identify").is_empty());
    for e in &ev {
        assert!(!e.to_string().contains("alice@example.com"));
    }
}

#[test]
fn the_server_turning_linking_off_unlinks() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    t.offer_account(Some(ACCT));
    send(&t);
    // `GET /v1/me` without analytics_id: SPACES_CP_LINK_USAGE is off.
    assert_eq!(t.offer_account(None), LinkState::NotOffered);
    send(&t);
    let ev = sink.events();
    assert_eq!(ev.last().unwrap()["distinct_id"], install_id(h.path()));
    assert_eq!(
        ev.last().unwrap()["properties"]["$process_person_profile"],
        false
    );
}

#[test]
fn signing_out_starts_over_and_accounts_never_merge() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    t.offer_account(Some(ACCT));
    send(&t);
    let first = install_id(h.path());
    t.forget_account().unwrap();
    send(&t);
    let second = install_id(h.path());
    assert_ne!(first, second, "a new install id after sign-out");
    assert_eq!(
        sink.events().last().unwrap()["distinct_id"],
        second.as_str()
    );

    // Another account signs in without a sign-out in between.
    t.offer_account(Some(ACCT));
    send(&t);
    let third = install_id(h.path());
    t.offer_account(Some(OTHER));
    send(&t);
    let fourth = install_id(h.path());
    assert_ne!(third, fourth);
    let ev = sink.events();
    let ids = named(&ev, "$identify");
    let last = ids.last().unwrap();
    assert_eq!(last["distinct_id"], OTHER);
    assert_eq!(last["properties"]["$anon_distinct_id"], fourth.as_str());
    for i in &ids {
        if i["distinct_id"] == OTHER {
            assert_ne!(i["properties"]["$anon_distinct_id"], third.as_str());
        }
    }
}

#[test]
fn a_spooled_identify_survives_a_failed_send() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    t.offer_account(Some(ACCT));
    sink.set_failing(true);
    send(&t);
    sink.set_failing(false);
    let (t2, sink2) = client(h.path(), NoticeMode::External);
    t2.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t2.flush(Duration::from_secs(1));
    assert_eq!(named(&sink2.events(), "$identify").len(), 1);
}

#[test]
fn a_signed_in_client_asks_again_at_most_once_per_period() {
    let h = tempfile::tempdir().unwrap();
    let (t, _) = client(h.path(), NoticeMode::External);
    t.acknowledge_notice();
    let day = Duration::from_secs(86_400);
    assert!(t.account_check_due(day), "never asked: due");
    t.mark_account_checked();
    assert!(!t.account_check_due(day), "just asked: not due");
    assert!(t.account_check_due(Duration::ZERO));
    // Signing out (or the opt-out) forgets it: the next client asks again.
    t.forget_account().unwrap();
    assert!(t.account_check_due(day));
    t.mark_account_checked();
    t.set_enabled(false).unwrap();
    assert!(!t.account_check_due(day), "never due while off");
    t.mark_account_checked();
    assert!(
        !h.path().join("telemetry").join(link::CHECKED_FILE).exists(),
        "nothing written while off"
    );
}
