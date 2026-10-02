// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Space in your cloud is a cloud sandbox plus its relay machine: Spaces
//! creates it through the sandbox layer, shows it as `relay:<machine>` with
//! its provider and place, stops, starts and deletes it there. Another
//! device of the account sees it from the relay metadata and deletes it
//! permanently only through the device that created it (or is told where);
//! removing it from the list keeps it.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_byoc::model::{self, Tier};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{RelayAccess, Target};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::Sandboxes;
use cua_sandbox_core::byoc::meta;
use cua_sandbox_core::placement::On;
use cua_spaces::host_spaces::{HostCaller, HostSpacesServer};
use cua_spaces::relay::{RelayAccount, StaticToken};
use cua_spaces::{SpaceCreate, Spaces};

struct World {
    _dir: tempfile::TempDir,
    relay: Arc<FakeRelay>,
    fake: Arc<FakeProvider>,
    sandboxes: Sandboxes,
    spaces: Spaces,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("aws", Tier::Vm);
    {
        let relay = relay.clone();
        fake.on_boot(Arc::new(move |spec, _| {
            relay.set_online(&spec.tags[model::tag::MACHINE], true, "0.1.0");
        }));
    }
    let token = Arc::new(StaticToken("acct-token".into()));
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![fake.clone()],
        RelayAccess::new(relay.url.clone(), token.clone()),
    )
    .build();
    sandboxes
        .clouds()
        .unwrap()
        .connect(
            &Target {
                provider: "aws".into(),
                ..Default::default()
            },
            false,
            None,
        )
        .await
        .unwrap();
    let spaces = Spaces::builder()
        .home(dir.path())
        .sandboxes(sandboxes.clone())
        .relay(RelayAccount::new(relay.url.clone(), token))
        .build();
    World {
        _dir: dir,
        relay,
        fake,
        sandboxes,
        spaces,
    }
}

async fn create(w: &World, name: &str) -> cua_spaces::SpaceInfo {
    w.spaces
        .create(SpaceCreate {
            on: Some(On::Provider("aws".into())),
            image: Some("registry.invalid/trycua/linux:24.04".into()),
            name: Some(name.into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap()
}

fn row(w: &World, id: &str) -> cua_spaces::SpaceInfo {
    w.spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .unwrap()
}

#[tokio::test]
async fn a_space_in_your_cloud_is_a_cloud_sandbox_on_the_relay() {
    let w = world().await;
    let info = create(&w, "Research box").await;
    assert!(info.id.starts_with("relay:cloud-"), "{}", info.id);
    assert_eq!(info.name, "research-box");
    assert_eq!(
        w.spaces.cloud_sandbox_of(&info.id).unwrap().as_deref(),
        Some("research-box")
    );
    let machine = info.id.trim_start_matches("relay:");

    // Its relay machine says what it is (no secrets), for other devices.
    let m = w.relay.machine(machine).unwrap();
    assert_eq!(m.meta[meta::PROVIDER], "aws");
    assert_eq!(m.meta[meta::PLACE], "Fake \u{b7} fake-1");
    assert_eq!(m.meta[meta::SANDBOX], "research-box");
    assert_eq!(m.meta[meta::OWNER].len(), 16);
    assert!(!m.meta.contains_key(meta::HOST), "this home is not a host");
    assert!(!m.meta.values().any(|v| v.contains("cmt_")));

    // Lists name the cloud it runs in, and that it is deleted here.
    let r = row(&w, &info.id);
    assert_eq!(
        (
            r.cloud.as_str(),
            r.cloud_place.as_str(),
            r.cloud_delete.as_str()
        ),
        ("aws", "Fake \u{b7} fake-1", "here")
    );
    assert_eq!(r.host_name, "Fake \u{b7} fake-1");
    assert!(r.host.is_empty());
    // A cloud VM stops and starts (stop_space / start_space).
    assert_eq!(r.power, "stop");

    let off = w.spaces.stop(&info.id).await.unwrap();
    assert_eq!(
        (off.state.as_str(), off.power.as_str()),
        ("stopped", "stop")
    );
    assert!(off.message.starts_with("Stopped relay:cloud-"), "{off:?}");
    let on = w.spaces.start(&info.id).await.unwrap();
    assert_eq!(on.state, "running");

    // A Space added by address cannot be turned off.
    let err = w.spaces.stop("direct:127.0.0.1:9").await.unwrap_err();
    assert_eq!(err.tag(), "wrong_provider");

    let msg = w.spaces.delete(&info.id).await.unwrap();
    assert!(msg.contains("in your cloud"), "{msg}");
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(w.relay.machine(machine).is_none());
    assert!(
        w.sandboxes
            .clouds()
            .unwrap()
            .status(None)
            .await
            .unwrap()
            .resources
            .is_empty()
    );
}

/// What another device of the account created: shown from its relay
/// metadata, deleted permanently only by that device.
#[tokio::test]
async fn a_cloud_space_another_device_created_is_deleted_there() {
    let w = world().await;
    let access = RelayAccess::new(
        w.relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    );
    let theirs = |host: Option<&str>| {
        let mut m = BTreeMap::from([
            (meta::PROVIDER.to_string(), "gcp".to_string()),
            (
                meta::PLACE.to_string(),
                "Google Cloud \u{b7} us-central1".to_string(),
            ),
            (meta::SANDBOX.to_string(), "theirs".to_string()),
            (meta::DEVICE.to_string(), "Studio Mac".to_string()),
        ]);
        if let Some(h) = host {
            m.insert(meta::HOST.to_string(), h.to_string());
        }
        m
    };
    // No host to ask: only there.
    access
        .register_with("cloud-0000000000000e01", "theirs", theirs(None))
        .await
        .unwrap();
    // A host that can be asked, but is offline.
    access
        .register("studio-mac-host1", "Studio Mac")
        .await
        .unwrap();
    access
        .register_with(
            "cloud-0000000000000e02",
            "theirs-2",
            theirs(Some("studio-mac-host1")),
        )
        .await
        .unwrap();
    w.spaces.relay_machines().await.unwrap();

    let r = row(&w, "relay:cloud-0000000000000e01");
    assert_eq!(
        (
            r.cloud.as_str(),
            r.cloud_place.as_str(),
            r.cloud_delete.as_str()
        ),
        ("gcp", "Google Cloud \u{b7} us-central1", "elsewhere")
    );
    assert_eq!(r.host_name, "Google Cloud \u{b7} us-central1");
    let err = w
        .spaces
        .delete("relay:cloud-0000000000000e01")
        .await
        .unwrap_err();
    assert_eq!(err.tag(), "host_capability_missing", "{err}");
    assert!(err.to_string().contains("created on Studio Mac"), "{err}");
    assert!(
        err.to_string().contains("remove it from this list"),
        "{err}"
    );
    assert!(w.relay.machine("cloud-0000000000000e01").is_some(), "kept");

    let r = row(&w, "relay:cloud-0000000000000e02");
    assert_eq!(r.cloud_delete, "host:studio-mac-host1");
    let err = w
        .spaces
        .delete("relay:cloud-0000000000000e02")
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Studio Mac, which created it, is offline"),
        "{err}"
    );
    assert!(w.relay.machine("cloud-0000000000000e02").is_some(), "kept");

    // Remove from list keeps the cloud resources and the relay machine.
    w.spaces
        .remove("relay:cloud-0000000000000e01")
        .await
        .unwrap();
    assert!(w.relay.machine("cloud-0000000000000e01").is_some());
    // Nothing was ever asked of this device's cloud.
    assert!(
        !w.fake
            .calls()
            .iter()
            .any(|c| c.starts_with("delete") || c.starts_with("provision"))
    );
}

/// The device that created it, asked through the relay
/// (`HostSpacesService.DeleteCloudSpace`): the owner only, through its own
/// records, audited.
#[tokio::test]
async fn the_creating_host_deletes_it_for_the_owner() {
    let w = world().await;
    let info = create(&w, "on-host").await;
    let machine = info.id.trim_start_matches("relay:").to_string();
    let host = HostSpacesServer::new(w.spaces.clone());

    let editor = HostCaller {
        account: "user-2".into(),
        role: "shared".into(),
        via: "relay".into(),
        ..Default::default()
    };
    let err = host.delete_cloud(&editor, &machine).await.unwrap_err();
    assert_eq!(err.tag(), "permission_denied", "{err}");
    assert!(w.relay.machine(&machine).is_some());

    let err = host
        .delete_cloud(&HostCaller::local(), "cloud-00000000000000ff")
        .await
        .unwrap_err();
    assert_eq!(err.tag(), "not_found", "{err}");

    let owner = HostCaller {
        account: "user-1".into(),
        role: "owner".into(),
        via: "relay".into(),
        ..Default::default()
    };
    let msg = host.delete_cloud(&owner, &machine).await.unwrap();
    assert!(
        msg.starts_with(&format!("Deleted relay:{machine}")),
        "{msg}"
    );
    assert!(w.fake.account.lock().unwrap().resources.is_empty());
    assert!(w.relay.machine(&machine).is_none());
}
