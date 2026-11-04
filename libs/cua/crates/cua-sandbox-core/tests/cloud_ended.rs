//! Cua Cloud has ended: every cloud entry point fails with the one message
//! (`CLOUD_ENDED`).

use cua_sandbox_core::{CLOUD_ENDED, CreateOptions, Error, ProviderKind, SandboxRef, Sandboxes};

fn sandboxes(dir: &std::path::Path) -> Sandboxes {
    Sandboxes::builder().state_dir(dir).build()
}

#[tokio::test]
async fn a_pool_image_fails_with_the_ended_message() {
    let dir = tempfile::tempdir().unwrap();
    for image in ["pool:my-pool", " pool:another "] {
        let e = sandboxes(dir.path())
            .create(CreateOptions::new(ProviderKind::Local, image))
            .await
            .unwrap_err();
        assert!(matches!(e, Error::CloudEnded), "{image}: {e:?}");
        assert_eq!(e.to_string(), CLOUD_ENDED);
    }
}

#[tokio::test]
async fn a_cloud_ref_fails_with_the_ended_message() {
    let dir = tempfile::tempdir().unwrap();
    let sbx = sandboxes(dir.path());
    let r = SandboxRef::parse("cloud:old").unwrap();
    assert!(matches!(sbx.connect_ref(&r).await, Err(Error::CloudEnded)));
}

#[test]
fn cloud_is_no_longer_a_listed_location() {
    assert!(
        cua_sandbox_core::placement::locations()
            .iter()
            .all(|c| c.name != "cloud")
    );
}
