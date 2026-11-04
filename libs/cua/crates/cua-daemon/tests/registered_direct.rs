//! A Space added by address (`cua spaces add host:port`) is a direct
//! sandbox too: `cua sb ls` lists it and `cua sb` finds it by name, though
//! no state file records it. Registry entries only (no network).

use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_proto::daemon::v1 as dpb;
use cua_sandbox_core::ProviderKind;
use cua_spaces::registry::Credential;

fn space(id: &str, name: &str) -> dpb::Space {
    dpb::Space {
        id: id.into(),
        name: name.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn spaces_added_by_address_are_listed_and_resolve_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let d = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();
    let registry = d.spaces().registry();
    let token = |t: &str| Credential {
        token: Some(t.into()),
        ..Default::default()
    };
    registry
        .upsert(space("direct:127.0.0.1:43211", "studio"), token("t"))
        .unwrap();
    // Not sandboxes: a plain MCP endpoint, and a Space a host forwards.
    let mut mcp = token("bearer");
    mcp.service_urls
        .insert("mcp".into(), "http://127.0.0.1:8765/mcp".into());
    registry
        .upsert(space("direct:127.0.0.1:8765", "tools"), mcp)
        .unwrap();
    let routed = cua_spaces::id::routed_authority("m-4f2a9c", 40123);
    registry
        .upsert(space(&format!("direct:{routed}"), "provided"), token("t"))
        .unwrap();

    let listed = d.list(None).await.unwrap();
    let ids: Vec<&str> = listed.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["direct:127.0.0.1:43211"], "{listed:?}");
    let studio = &listed[0];
    assert_eq!(studio.name, "studio");
    assert_eq!(studio.location, "direct");
    assert_eq!(studio.provider, ProviderKind::Direct);
    assert!(d.list(Some(ProviderKind::Local)).await.unwrap().is_empty());

    assert_eq!(
        d.resolve("studio").await.unwrap().to_string(),
        "direct:127.0.0.1:43211"
    );
    assert!(d.resolve("tools").await.is_err());

    // Forgotten with the Space.
    registry.remove("direct:127.0.0.1:43211").unwrap();
    assert!(d.list(None).await.unwrap().is_empty());
}
