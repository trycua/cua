//! Cua Cloud has ended: a cloud create fails with the one message, whatever
//! else the request carries, and listings say nothing about the cloud.

use cua_daemon::{CreateRequest, Error, Runtime, RuntimeConfig};

#[tokio::test]
async fn a_cloud_create_fails_with_the_ended_message() {
    let dirs = tempfile::tempdir().unwrap();
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        ..Default::default()
    })
    .unwrap();

    for req in [
        CreateRequest {
            location: Some("cloud".into()),
            image: "linux".into(),
            ..Default::default()
        },
        CreateRequest {
            location: Some("cloud".into()),
            ..Default::default()
        },
    ] {
        let e = rt.create(req).await.unwrap_err();
        assert!(
            matches!(&e, Error::Unsupported(m) if m == cua_sandbox_core::CLOUD_ENDED),
            "{e:?}"
        );
    }

    // A `pool:<name>` image named a cloud pool's template: the same message.
    let e = rt
        .create(CreateRequest {
            location: Some("local".into()),
            image: "pool:a-pool".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Error::Unsupported(m) if m == cua_sandbox_core::CLOUD_ENDED),
        "{e:?}"
    );

    // Listings say nothing about the cloud.
    let l = rt.list_filtered(None).await.unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert!(l.sandboxes.iter().all(|s| s.location != "cloud"));
}
