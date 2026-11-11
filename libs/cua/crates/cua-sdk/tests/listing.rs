//! The default sandbox listing: Cua Cloud has ended, so cloud sandboxes are
//! never listed (quietly, with no warning). Nothing here reaches the
//! credential store (`account_from_session` is off).

use cua_sdk::{Cua, CuaConfig};

fn cua(dir: &tempfile::TempDir) -> std::sync::Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        account_from_session: false,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloud_sandboxes_are_not_listed_and_there_are_no_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let c = cua(&dir);
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    assert!(l.warnings.is_empty(), "{l:?}");
    assert!(l.sandboxes.iter().all(|s| s.location != "cloud"));
    assert!(c.sandboxes().list(None).await.is_ok());
    // Cua Cloud has ended: asking for cloud sandboxes lists none, quietly.
    let l = c
        .sandboxes()
        .list_with_warnings(Some("cloud".into()))
        .await
        .unwrap();
    assert!(l.sandboxes.is_empty());
    assert!(l.warnings.is_empty(), "{l:?}");
}
