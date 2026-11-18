//! Local sandbox lifetimes: `keep_alive` on a local sandbox records when it
//! ends and what ends it, and `expired_local` lists the ones the daemon must
//! end. State files only (temp state dir, no runtime, no host processes).

use cua_sandbox_core::{Error, LocalExpiry, LocalState, SandboxState, Sandboxes};
use std::time::{Duration, SystemTime};

fn record(name: &str, status: &str) -> SandboxState {
    SandboxState::Local(LocalState {
        name: name.into(),
        runtime_type: "qemu".into(),
        host: "127.0.0.1".into(),
        api_port: 1,
        status: status.into(),
        ..Default::default()
    })
}

fn names(v: Vec<(String, LocalExpiry)>) -> Vec<String> {
    let mut n: Vec<_> = v.into_iter().map(|(n, _)| n).collect();
    n.sort();
    n
}

#[tokio::test]
async fn keep_alive_sets_a_lifetime_that_expires() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder().state_dir(dir.path()).build();
    sbx.state().save(&record("a", "running")).unwrap();
    sbx.state().save(&record("b", "running")).unwrap();
    sbx.state().save(&record("never", "running")).unwrap();

    sbx.keep_alive("a", Duration::from_secs(60)).await.unwrap();
    sbx.keep_alive("b", Duration::from_secs(120)).await.unwrap();
    let now = SystemTime::now();
    assert!(sbx.expired_local(now).is_empty());
    let (at, action) = sbx.local_expiry("a").unwrap();
    assert_eq!(action, LocalExpiry::Delete);
    assert!(at > now + Duration::from_secs(55) && at <= now + Duration::from_secs(61));
    assert!(sbx.local_expiry("never").is_none());

    // 90 s later only `a` is over; at 3 min both are.
    assert_eq!(
        names(sbx.expired_local(now + Duration::from_secs(90))),
        ["a"]
    );
    assert_eq!(
        names(sbx.expired_local(now + Duration::from_secs(180))),
        ["a", "b"]
    );

    // A later keep_alive replaces the lifetime, also with a shorter one.
    sbx.keep_alive("b", Duration::from_secs(0)).await.unwrap();
    assert_eq!(
        names(sbx.expired_local(SystemTime::now() + Duration::from_secs(1))),
        ["b"]
    );
}

#[tokio::test]
async fn suspend_lifetimes_end_once() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder().state_dir(dir.path()).build();
    sbx.state().save(&record("p", "running")).unwrap();
    sbx.set_local_expiry("p", Duration::ZERO, Some(LocalExpiry::Suspend))
        .unwrap();
    let later = SystemTime::now() + Duration::from_secs(1);
    assert_eq!(
        sbx.expired_local(later),
        [("p".to_string(), LocalExpiry::Suspend)]
    );
    // A keep_alive without an action keeps `suspend`.
    sbx.keep_alive("p", Duration::ZERO).await.unwrap();
    assert_eq!(sbx.local_expiry("p").unwrap().1, LocalExpiry::Suspend);
    // Once suspended it is not ended again.
    sbx.state().set_status("p", "suspended").unwrap();
    assert!(sbx.expired_local(later).is_empty());
    // Clearing removes the lifetime and keeps the record readable.
    sbx.clear_local_expiry("p").unwrap();
    assert!(sbx.local_expiry("p").is_none());
    assert!(sbx.state().load("p").is_some());
}

#[tokio::test]
async fn keep_alive_on_an_unknown_sandbox_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = Sandboxes::builder().state_dir(dir.path()).build();
    assert!(matches!(
        sbx.keep_alive("missing", Duration::from_secs(1)).await,
        Err(Error::NotFound(_))
    ));
}

#[test]
fn expiry_words() {
    assert_eq!(LocalExpiry::parse("kill"), Some(LocalExpiry::Delete));
    assert_eq!(LocalExpiry::parse("pause"), Some(LocalExpiry::Suspend));
    assert_eq!(LocalExpiry::parse("SUSPEND"), Some(LocalExpiry::Suspend));
    assert_eq!(LocalExpiry::parse("explode"), None);
}
