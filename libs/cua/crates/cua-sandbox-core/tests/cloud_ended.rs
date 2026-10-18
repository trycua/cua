//! Cua Cloud has ended: every cloud entry point fails with the one message
//! (`CLOUD_ENDED`) and never reaches Fleet.

use cua_sandbox_core::{CLOUD_ENDED, CreateOptions, Error, ProviderKind, SandboxRef, Sandboxes};

fn sandboxes(dir: &std::path::Path) -> Sandboxes {
    Sandboxes::builder().state_dir(dir).build()
}

#[tokio::test]
async fn creating_in_the_cloud_fails_with_the_ended_message() {
    let dir = tempfile::tempdir().unwrap();
    let e = sandboxes(dir.path())
        .create(CreateOptions::new(ProviderKind::Fleet, "linux"))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::CloudEnded), "{e:?}");
    assert_eq!(e.to_string(), CLOUD_ENDED);
}

#[tokio::test]
async fn an_old_cloud_record_is_not_listed_and_fails_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = sandboxes(dir.path());
    sbx.state().save_fleet_claim("old", "cua-auto-x").unwrap();
    assert!(sbx.list().await.unwrap().iter().all(|i| i.name != "old"));
    assert!(matches!(sbx.get("old").await, Err(Error::CloudEnded)));
    assert!(matches!(sbx.connect("old").await, Err(Error::CloudEnded)));
    assert!(matches!(sbx.delete("old").await, Err(Error::CloudEnded)));
    assert!(matches!(sbx.suspend("old").await, Err(Error::CloudEnded)));
    assert!(matches!(sbx.resume("old").await, Err(Error::CloudEnded)));
    let r = SandboxRef::parse("cloud:old").unwrap();
    assert!(matches!(sbx.connect_ref(&r).await, Err(Error::CloudEnded)));
    assert!(matches!(
        sbx.connect_cloud("other", None).await,
        Err(Error::CloudEnded)
    ));
    assert!(matches!(sbx.pools(), Err(Error::CloudEnded)));
}

#[test]
fn cloud_is_no_longer_a_listed_location() {
    assert!(
        cua_sandbox_core::placement::locations()
            .iter()
            .all(|c| c.name != "cloud")
    );
}
