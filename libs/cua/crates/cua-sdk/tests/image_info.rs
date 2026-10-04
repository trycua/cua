//! The exported `Sandbox::image_info` / `SandboxInfo.image_info` of a
//! direct sandbox (no network, nothing started): nothing was resolved.

use cua_daemon::{Runtime, RuntimeConfig, fixtures};
use cua_sdk::Cua;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_sandboxes_report_no_image() {
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    runtime.mark_share_host();
    let cua = Cua::from_runtime(runtime);
    let sandboxes = cua.sandboxes();
    let env = fixtures::start_env(None, None).await;
    let direct = sandboxes
        .connect_url(env.url.clone(), None, None)
        .await
        .unwrap();
    assert_eq!(direct.image_info(), None);
    assert_eq!(direct.info().image_info, None);
}
