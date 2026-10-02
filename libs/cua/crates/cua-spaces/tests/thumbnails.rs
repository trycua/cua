//! The Spaces runtime's thumbnail cache, as the daemon serves it
//! (`SpaceService.GetSpaceThumbnail`): cached answers within `max_age`,
//! the older image when a fresh capture fails, persistence across a
//! restart of the runtime, and the background pass's eviction and
//! interest gate. Hermetic: a temp cua home, no Space ever answers.

use cua_spaces::Spaces;
use cua_spaces::thumbnails::Thumbnail;
use std::time::{Duration, Instant, SystemTime};

const ID: &str = "direct:127.0.0.1:9";

fn spaces(home: &std::path::Path) -> Spaces {
    Spaces::builder()
        .home(home)
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .state_dir(home.join("sandboxes"))
                .build(),
        )
        .build()
}

fn shot(age: Duration) -> Thumbnail {
    Thumbnail {
        image: vec![0xff, 0xd8, 0xff, 0xd9],
        format: "jpeg".into(),
        width: 320,
        height: 200,
        captured_at: SystemTime::now() - age,
    }
}

#[tokio::test]
async fn cached_within_max_age_else_the_older_one_when_capture_fails() {
    let home = tempfile::tempdir().unwrap();
    let s = spaces(home.path());
    // Nothing cached and nothing answers: the capture's error.
    assert!(s.thumbnail(ID, None).await.is_err());

    let cached = shot(Duration::from_secs(30));
    s.thumbnails().put(ID, cached.clone());
    // Younger than max_age (or any age): at once, no capture.
    assert_eq!(s.thumbnail(ID, None).await.unwrap(), cached);
    assert_eq!(
        s.thumbnail(ID, Some(Duration::from_secs(60)))
            .await
            .unwrap(),
        cached
    );
    // Older than max_age: a capture is tried; it fails (the Space is not
    // registered), so the older image comes back with its timestamp.
    let stale = s.thumbnail(ID, Some(Duration::ZERO)).await.unwrap();
    assert_eq!(stale.captured_at, cached.captured_at);
    // Asking marked interest.
    assert!(s.thumbnails().interested(Instant::now()));
}

#[tokio::test]
async fn the_cache_survives_a_restart_under_the_cua_home() {
    let home = tempfile::tempdir().unwrap();
    let t = shot(Duration::from_secs(5));
    spaces(home.path()).thumbnails().put(ID, t.clone());
    let dir = home.path().join("cache").join("thumbnails");
    assert!(dir.is_dir(), "kept under <cua home>/cache/thumbnails");
    let again = spaces(home.path()).thumbnail(ID, None).await.unwrap();
    assert_eq!(
        (&again.image, again.width, again.height),
        (&t.image, t.width, t.height)
    );
    // Kept to the millisecond.
    let ms = |t: &Thumbnail| {
        t.captured_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    };
    assert_eq!(ms(&again), ms(&t));
}

#[tokio::test]
async fn the_background_pass_needs_interest_and_evicts_gone_spaces() {
    let home = tempfile::tempdir().unwrap();
    let s = spaces(home.path());
    s.thumbnails().put(ID, shot(Duration::from_secs(5)));
    // Nobody asked: nothing happens (no guest load, nothing evicted).
    assert_eq!(s.refresh_thumbnails().await, 0);
    assert!(s.thumbnails().get(ID).is_some());
    // Someone asked: the Space is not in the registry, so its image goes.
    s.thumbnails().note_interest(Instant::now());
    assert_eq!(s.refresh_thumbnails().await, 0);
    assert!(s.thumbnails().get(ID).is_none());
    assert!(
        spaces(home.path()).thumbnails().get(ID).is_none(),
        "on disk too"
    );
}
