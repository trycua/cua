// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every provider against the elastic pool's contract
//! ([`cua_byoc::conformance`]): the scripted fake (both tiers) end to end,
//! and AWS, Google Cloud, Hetzner and Modal's capabilities and host types.
//! Each provider's own test file runs the lifecycle checks against its fake
//! API too.

use cua_byoc::api::CloudApi;
use cua_byoc::conformance::{CreateCall, lifecycle_checks, static_checks};
use cua_byoc::model::{Connection, Tier};
use cua_byoc::testing::FakeProvider;

fn conn(provider: &str, region: &str) -> Connection {
    Connection {
        provider: provider.into(),
        region: region.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_fake_cloud_conforms_as_hosts_and_as_spaces() {
    for tier in [Tier::Vm, Tier::Sandbox] {
        let fake = FakeProvider::new("fake", tier);
        let c = conn("fake", "fake-1");
        let bad = static_checks(fake.as_ref(), &c).await;
        assert!(bad.is_empty(), "{tier:?}: {bad:#?}");
        let f2 = fake.clone();
        let seen = move || {
            f2.account
                .lock()
                .unwrap()
                .specs
                .iter()
                .map(|s| CreateCall {
                    machine_type: s.machine_type.clone(),
                    region: s.region.clone(),
                    user_data: s.user_data.clone(),
                    tags: s.tags.clone(),
                    ssh_key: false,
                })
                .collect()
        };
        let bad = lifecycle_checks(fake.as_ref(), &c, "owner-1", &seen).await;
        assert!(bad.is_empty(), "{tier:?}: {bad:#?}");
    }
}

#[cfg(feature = "aws")]
#[tokio::test]
async fn aws_declares_a_conforming_offer() {
    let api = cua_byoc::aws::Aws::new();
    let bad = static_checks(&api, &conn("aws", "us-west-2")).await;
    assert!(bad.is_empty(), "{bad:#?}");
    let types = api.host_types(&conn("aws", "us-west-2")).await.unwrap();
    assert!(types.iter().any(|t| t.kvm), "a bare-metal type runs VMs");
}

#[cfg(feature = "gcp")]
#[tokio::test]
async fn gcp_declares_a_conforming_offer() {
    let c = Connection {
        provider: "gcp".into(),
        region: "us-central1".into(),
        zone: "us-central1-a".into(),
        project: "p".into(),
        ..Default::default()
    };
    let bad = static_checks(&cua_byoc::gcp::Gcp::new(), &c).await;
    assert!(bad.is_empty(), "{bad:#?}");
}

#[cfg(feature = "modal")]
#[tokio::test]
async fn modal_supplies_spaces_not_hosts() {
    let api = cua_byoc::modal::ModalApi::from_env();
    let c = conn("modal", "");
    let bad = static_checks(&api, &c).await;
    assert!(bad.is_empty(), "{bad:#?}");
    assert_eq!(
        api.capabilities(&c).supplies,
        cua_byoc::model::Supplies::Spaces
    );
    let bad = lifecycle_checks(&api, &c, "owner-1", &Vec::new).await;
    assert!(bad.is_empty(), "{bad:#?}");
}
