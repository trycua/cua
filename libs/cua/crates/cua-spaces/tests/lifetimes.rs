//! Space lifetimes ([`Spaces::set_lifetime`]): recorded next to the
//! registry, listed once over, and ended by [`Spaces::reap_expired`] (the
//! daemon's loop). A Space added by an MCP URL on loopback stands in for any
//! registered Space: ending it forgets it. No VMs, no host processes.

use cua_sandbox_core::LocalExpiry;
use cua_sandbox_core::testing::McpTestServer;
use cua_spaces::{Error, Spaces};
use std::time::{Duration, SystemTime};

async fn registered(spaces: &Spaces, url: &str, name: &str) -> String {
    spaces
        .add(&format!("{url}/mcp"), None, Some(name.into()))
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_lifetime_ends_the_space_once_it_passes() {
    let server = McpTestServer::start("").await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let id = registered(&spaces, &server.url, "short").await;

    let at = spaces
        .set_lifetime(&id, Duration::from_secs(60), None)
        .unwrap();
    let now = SystemTime::now();
    assert!(at > now + Duration::from_secs(55) && at <= now + Duration::from_secs(61));
    let l = spaces.lifetime("short").unwrap();
    assert_eq!(l.action(), LocalExpiry::Delete);
    assert!(spaces.expired(now).is_empty());
    assert_eq!(
        spaces.expired(now + Duration::from_secs(120)),
        [(id.clone(), LocalExpiry::Delete)]
    );
    // Nothing has passed yet: the reaper leaves it.
    assert!(spaces.reap_expired().await.is_empty());
    assert_eq!(spaces.list().unwrap().len(), 1);

    // A later call replaces it, here with a lifetime already over.
    spaces.set_lifetime(&id, Duration::ZERO, None).unwrap();
    assert_eq!(spaces.reap_expired().await, std::slice::from_ref(&id));
    assert!(spaces.list().unwrap().is_empty(), "the Space is gone");
    assert!(spaces.lifetime(&id).is_none(), "and so is its lifetime");
}

#[tokio::test]
async fn the_record_survives_a_new_spaces_handle() {
    let server = McpTestServer::start("").await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let id = {
        let spaces = Spaces::builder().home(home.path()).build();
        let id = registered(&spaces, &server.url, "kept").await;
        spaces
            .set_lifetime(&id, Duration::from_secs(600), Some(LocalExpiry::Suspend))
            .unwrap();
        id
    };
    // What a daemon started later (or another process) reads.
    let again = Spaces::builder().home(home.path()).build();
    let l = again.lifetime(&id).unwrap();
    assert_eq!(l.action(), LocalExpiry::Suspend);
    // Without an action, a new lifetime keeps the old one's.
    again
        .set_lifetime(&id, Duration::from_secs(5), None)
        .unwrap();
    assert_eq!(again.lifetime(&id).unwrap().action(), LocalExpiry::Suspend);
    again.clear_lifetime(&id).unwrap();
    assert!(again.lifetime(&id).is_none());
}

#[tokio::test]
async fn a_space_that_cannot_be_turned_off_is_tried_again() {
    let server = McpTestServer::start("").await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let id = registered(&spaces, &server.url, "always-on").await;
    // A Space added by address cannot be turned off: the reaper keeps its
    // lifetime and retries instead of dropping it.
    spaces
        .set_lifetime(&id, Duration::ZERO, Some(LocalExpiry::Suspend))
        .unwrap();
    assert!(spaces.reap_expired().await.is_empty());
    assert!(spaces.lifetime(&id).is_some());
    assert_eq!(spaces.list().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_spaces_have_no_lifetime() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    assert!(matches!(
        spaces.set_lifetime("direct:127.0.0.1:9", Duration::from_secs(1), None),
        Err(Error::NotFound(_))
    ));
}
