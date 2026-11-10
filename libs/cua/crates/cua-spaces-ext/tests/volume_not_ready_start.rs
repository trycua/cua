// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! CUA-4002: a persistent agent starts while Cua Volume is not answering
//! (sync with the other computer stalled): it starts anyway, in a folder of
//! its own, says why in plain words, and never saves that folder over the
//! home. Against the in-process spacesd mock and a store that refuses
//! every call.

use std::sync::Arc;

use cua_spaces::Spaces;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::AgentSpec;
use cua_spacesd_client::testing::{MockAuth, MockServer};

#[tokio::test]
async fn an_agent_starts_without_a_volume_that_does_not_answer() {
    let srv = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let store =
        cua_volume::backend::Unavailable("the other computer did not answer (sync stalled)".into());
    let drive = cua_volume::Drive::new(Arc::new(store), home.path().join("drive"));
    let spaces = cua_spaces_ext::register(Spaces::builder().home(home.path()), drive, None).build();
    let info = spaces
        .add(
            &format!("http://{}", srv.addr),
            None,
            Some("other-mac".into()),
        )
        .await
        .unwrap();
    let p = spaces.persistent();
    p.create(AgentSpec {
        name: "ada".into(),
        harness: "claude-code".into(),
        space: info.id.clone(),
        ..Default::default()
    })
    .unwrap();

    let d = p.start("ada", "fix the build").await.unwrap();
    assert!(d.started);
    assert!(d.restored.as_ref().is_some_and(|t| t.offline), "{d:?}");
    let notice = d.notice.clone().expect("a notice");
    assert!(
        notice.starts_with("Cua Volume isn't answering on this computer"),
        "{notice}"
    );
    assert!(notice.contains("ada started anyway"), "{notice}");
    assert!(!notice.contains("guest command failed"), "{notice}");
    // Its own folder, not the home's copy.
    assert!(
        srv.state
            .is_dir("/home/cua/cua-volume/agents/ada@offline/work")
    );
    assert!(!srv.state.is_dir("/home/cua/cua-volume/agents/ada"));
    let rec = p.get("ada").unwrap();
    assert!(rec.home_offline);
    // Saving never writes this run over the home.
    let saved = p.save("ada").await.unwrap();
    assert!(
        saved.offline && saved.files == 0 && saved.removed == 0,
        "{saved:?}"
    );
}
