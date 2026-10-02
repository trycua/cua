//! `Space.thumbnail` (the SDK face of the shared thumbnail cache) against
//! the in-process mock spacesd: the first call captures, a call within
//! `max_age_ms` is answered from the cache without touching the guest, a
//! zero `max_age_ms` captures again, and a new runtime on the same cua home
//! still has it. No host effects.

use cua_sdk::{Cua, CuaConfig};
use cua_spacesd_client::testing::MockServer;

fn cua(dir: &tempfile::TempDir) -> std::sync::Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dir.path().join("pools").display().to_string()),
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumbnails_come_from_the_cache_within_max_age() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start(Default::default()).await;
    let shots = || server.state.observed.screenshots.lock().unwrap().len();
    let c = cua(&dir);
    let info = c
        .spaces()
        .add(server.url(), None, Some("lab".into()))
        .await
        .unwrap();
    let space = c.spaces().space(info.id.clone()).await.unwrap();

    let first = space.thumbnail(None).await.unwrap();
    assert!(!first.image.is_empty() && first.captured_at_ms > 0);
    assert_eq!(shots(), 1, "nothing cached: captured");
    // A small JPEG of the primary display was asked for.
    {
        let asked = server.state.observed.screenshots.lock().unwrap();
        assert_eq!(
            asked[0].format,
            cua_spacesd_client::pb::ImageFormat::Jpeg as i32
        );
        assert!(asked[0].max_dimension > 0 && asked[0].max_dimension <= 480);
    }

    let cached = space.thumbnail(Some(60_000)).await.unwrap();
    assert_eq!(cached, first);
    assert_eq!(shots(), 1, "within max_age: no capture");

    let fresh = space.thumbnail(Some(0)).await.unwrap();
    assert_eq!(shots(), 2, "max_age 0: captured again");
    assert!(fresh.captured_at_ms >= first.captured_at_ms);

    // Another runtime on the same home (another client): the cache.
    let other = cua(&dir).spaces().space(info.id).await.unwrap();
    let shared = other.thumbnail(None).await.unwrap();
    assert_eq!(shared.image, fresh.image);
    assert_eq!(shots(), 2);
}
