// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Modal through the sandbox layer, against an in-process stand-in for
//! `cua-modal-helper` (the same JSON protocol) and the fake relay: the
//! profile and environment reach every call, the sandbox joins the relay
//! through the join wrapper, runtimes validate, stop is refused, and delete
//! touches only this home's sandboxes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_byoc::modal::{HelperTransport, JOIN_WRAPPER, ModalApi};
use cua_byoc::{RelayAccess, Target};
use cua_contrib::image_config::{FixedConfigSource, ImageConfig};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE;
use cua_sandbox_core::placement::Runtime;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeModal {
    requests: Mutex<Vec<(Value, bool)>>,
    sandboxes: Mutex<BTreeMap<String, Value>>,
    relay: Mutex<Option<Arc<FakeRelay>>>,
}

#[async_trait]
impl HelperTransport for FakeModal {
    async fn call(&self, req: Value, profile_only: bool, _: Duration) -> cua_byoc::Result<Value> {
        self.requests
            .lock()
            .unwrap()
            .push((req.clone(), profile_only));
        let mut sbs = self.sandboxes.lock().unwrap();
        Ok(match req["op"].as_str().unwrap_or_default() {
            "check" if req["environment"] == "missing" => {
                json!({"error": {"kind": "not_found", "message": "environment \"missing\" not found"}})
            }
            "check" => {
                json!({"account": {"profile": req["profile"], "environment": req["environment"], "sandboxes": sbs.len()}})
            }
            "create" => {
                let id = format!("sb-{}", sbs.len() + 1);
                let sb = json!({"id": id, "name": req["name"], "status": "running", "tags": req["tags"]});
                sbs.insert(id, sb.clone());
                // The guest's driver joins the relay as its machine.
                if let (Some(relay), Some(m)) = (
                    self.relay.lock().unwrap().as_ref(),
                    req["env"]["CUA_ENV_MACHINE_ID"].as_str(),
                ) {
                    relay.set_online(m, true, "0.1.0");
                }
                json!({"sandbox": sb})
            }
            "get" => match sbs.get(req["id"].as_str().unwrap_or_default()) {
                Some(sb) => json!({"sandbox": sb}),
                None => json!({"error": {"kind": "not_found", "message": "Sandbox not found"}}),
            },
            "list" => {
                let want = req["tags"].as_object().cloned().unwrap_or_default();
                let all: Vec<Value> = sbs
                    .values()
                    .filter(|s| want.iter().all(|(k, v)| &s["tags"][k] == v))
                    .cloned()
                    .collect();
                json!({"sandboxes": all})
            }
            "delete" => match sbs.remove(req["id"].as_str().unwrap_or_default()) {
                Some(_) => json!({}),
                None => json!({"error": {"kind": "not_found", "message": "Sandbox not found"}}),
            },
            other => {
                json!({"error": {"kind": "invalid", "message": format!("unknown op {other}")}})
            }
        })
    }
}

async fn world() -> (tempfile::TempDir, Arc<FakeModal>, Sandboxes) {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = Arc::new(FakeModal::default());
    *fake.relay.lock().unwrap() = Some(relay.clone());
    let toml = dir.path().join("modal.toml");
    std::fs::write(
        &toml,
        "[cuaai]\ntoken_id = \"ak-secret\"\ntoken_secret = \"as-secret\"\nactive = true\n",
    )
    .unwrap();
    let api = ModalApi::with(
        fake.clone(),
        Arc::new(FixedConfigSource(ImageConfig {
            cmd: vec!["/opt/cua/desktop/entrypoint.sh".into()],
            ..Default::default()
        })),
        Some(toml),
    );
    let sandboxes = cua_byoc::install_with(
        Sandboxes::builder().state_dir(dir.path().join("sandboxes")),
        dir.path(),
        vec![Arc::new(api)],
        RelayAccess::new(
            relay.url.clone(),
            Arc::new(cua_host::relay::StaticToken("acct-token".into())),
        ),
    )
    .build();
    (dir, fake, sandboxes)
}

fn create(name: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, "registry.invalid/trycua/linux:24.04");
    o.contrib = Some("modal".into());
    o.name = Some(name.into());
    o
}

#[tokio::test]
async fn a_modal_sandbox_joins_the_relay_and_is_deleted_with_its_machine() {
    let (_dir, fake, sandboxes) = world().await;
    let clouds = sandboxes.clouds().unwrap();

    // The profile's environment must exist; nothing is connected otherwise.
    let err = clouds
        .connect(
            &Target {
                provider: "modal".into(),
                environment: Some("missing".into()),
                ..Default::default()
            },
            false,
            None,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");

    let c = clouds
        .connect(
            &Target {
                provider: "modal".into(),
                environment: Some("cua-byoc".into()),
                ..Default::default()
            },
            false,
            Some(48),
        )
        .await
        .unwrap();
    assert_eq!(c.provider.profile, "cuaai", "the active profile");
    assert_eq!(c.provider.label, "Modal \u{b7} cua-byoc");
    assert_eq!(
        c.provider.ttl_hours, 24,
        "a Modal sandbox lives at most 24 h"
    );
    assert!(c.provider.credentials.source.contains("profile cuaai"));
    // The profile's name is all Cua sends; the token stays in the file.
    let reqs = fake.requests.lock().unwrap().clone();
    assert!(
        reqs.iter()
            .all(|(r, profile_only)| r["profile"] == "cuaai" && *profile_only)
    );
    assert!(!format!("{reqs:?}").contains("ak-secret"));

    // --runtime microvm is Modal's VM runtime; anything else is refused by
    // the placement model before any call.
    let mut o = create("box");
    o.runtime = Runtime::Other("microvm".into());
    let sb = sandboxes.create(o).await.unwrap();
    let machine = sb.provider_details()[DETAIL_RELAY_MACHINE].clone();
    let req = fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|(r, _)| r["op"] == "create")
        .unwrap()
        .0
        .clone();
    assert_eq!(req["runtime"], "vm");
    assert_eq!(req["env"]["CUA_ENV_RUNTIME"], "container");
    assert_eq!(req["environment"], "cua-byoc");
    assert_eq!(req["timeout_secs"], 24 * 3600);
    let argv: Vec<&str> = req["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        argv,
        vec![
            "/bin/sh",
            "-c",
            JOIN_WRAPPER,
            "cua-join",
            "/opt/cua/desktop/entrypoint.sh"
        ]
    );
    assert_eq!(req["env"]["CUA_ENV_MACHINE_ID"], machine);
    assert_eq!(req["env"]["CUA_GUESTD_ARGS"], "join");
    assert_eq!(req["tags"]["cua-managed"], "true");
    assert!(req["tags"]["cua-created"].as_str().is_some());

    let mut bad = create("bad");
    bad.runtime = Runtime::Other("firecracker".into());
    let err = sandboxes.create(bad).await.unwrap_err();
    assert!(
        matches!(err, cua_sandbox_core::Error::InvalidPlacement(ref p) if p.valid.contains(&"microvm".to_string())),
        "{err}"
    );

    // Stop is refused with the reason; the sandbox keeps running.
    let err = sandboxes.suspend("box").await.unwrap_err();
    assert!(
        matches!(err, cua_sandbox_core::Error::InvalidArgument(_)),
        "{err}"
    );
    assert!(err.to_string().contains("cannot stop"), "{err}");

    // A sandbox of the environment that is not this home's is never touched.
    fake.sandboxes.lock().unwrap().insert(
        "sb-foreign".into(),
        json!({"id": "sb-foreign", "status": "running", "tags": {"team": "x"}}),
    );
    let swept = clouds.sweep(Some("modal"), false, true).await.unwrap();
    assert!(
        swept
            .resources
            .iter()
            .all(|i| i.resource.id != "sb-foreign")
    );
    assert!(fake.sandboxes.lock().unwrap().contains_key("sb-foreign"));
    assert!(
        !fake.sandboxes.lock().unwrap().contains_key("sb-1"),
        "--all deleted ours"
    );
    // Deleting the (already swept) sandbox by name is fine.
    sandboxes.delete("box").await.unwrap();
}
