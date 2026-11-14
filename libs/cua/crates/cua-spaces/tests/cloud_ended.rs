//! Cua Cloud has ended: `create_space(on="cloud")` fails with the one
//! message.

use cua_sandbox_core::placement::On;
use cua_spaces::{SpaceCreate, Spaces};

#[tokio::test]
async fn creating_a_cloud_space_fails_with_the_ended_message() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some("linux".into()),
            ..Default::default()
        })
        .await
        .expect_err("a cloud create fails");
    assert_eq!(e.to_string(), cua_sandbox_core::CLOUD_ENDED);
}

fn ended(e: impl std::fmt::Display) -> bool {
    e.to_string() == cua_sandbox_core::CLOUD_ENDED
}

/// A `cloud:` Space an earlier version registered: every operation that
/// needed Cua Cloud answers with the one message, and `remove` still forgets
/// the stale record locally.
#[tokio::test]
async fn a_registered_cloud_space_answers_cloud_ended_and_can_be_forgotten() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    spaces
        .registry()
        .upsert(
            cua_proto::daemon::v1::Space {
                id: "cloud:old".into(),
                name: "old".into(),
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();

    // Local registry reads need no client.
    assert_eq!(
        spaces.resolve("cloud:old").unwrap().to_string(),
        "cloud:old"
    );
    assert_eq!(spaces.find("cloud:old").await.unwrap().id, "cloud:old");

    // Everything that needed the cloud says it ended.
    assert!(ended(spaces.space("cloud:old").await.unwrap_err()));
    assert!(ended(spaces.start("cloud:old").await.unwrap_err()));
    assert!(ended(spaces.stop("cloud:old").await.unwrap_err()));
    assert!(ended(spaces.delete("cloud:old").await.unwrap_err()));
    assert!(ended(
        spaces.add("cloud:old", None, None).await.unwrap_err()
    ));

    // The failed calls left the record alone; `remove` forgets it.
    assert!(spaces.registry().get("cloud:old").unwrap().is_some());
    let removed = spaces.remove("cloud:old").await.unwrap();
    assert_eq!(removed.id, "cloud:old");
    assert!(spaces.registry().get("cloud:old").unwrap().is_none());
}

#[tokio::test]
async fn a_cloud_create_in_the_background_still_fails_up_front() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            wait: Some(false),
            ..Default::default()
        })
        .await
        .expect_err("a cloud create fails");
    assert!(ended(e));
}
