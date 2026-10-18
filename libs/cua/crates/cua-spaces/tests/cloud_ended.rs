//! Cua Cloud has ended: `create_space(on="cloud")` fails with the one
//! message and never reaches Fleet.

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
