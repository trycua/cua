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
use cua_byoc::modal::{HelperTransport, JOIN_WRAPPER, ModalApi, ModalToken};
use cua_byoc::{RelayAccess, Target};
use cua_contrib::image_config::{FixedConfigSource, ImageConfig};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::DETAIL_RELAY_MACHINE;
use cua_sandbox_core::placement::Runtime;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeModal {
    /// Calls signed in with a token: (request, token id, token secret).
    with_token: Mutex<Vec<(Value, String, String)>>,
    requests: Mutex<Vec<(Value, bool)>>,
    sandboxes: Mutex<BTreeMap<String, Value>>,
    relay: Mutex<Option<Arc<FakeRelay>>>,
    /// Answer this op with "unavailable" this many more times.
    unavailable: Mutex<(String, u32)>,
}

#[async_trait]
impl HelperTransport for FakeModal {
    async fn call_with_token(
        &self,
        req: Value,
        token: &ModalToken,
        d: Duration,
    ) -> cua_byoc::Result<Value> {
        self.with_token.lock().unwrap().push((
            req.clone(),
            token.id.clone(),
            token.secret.expose().to_string(),
        ));
        self.call(req, true, d).await
    }

    async fn call(&self, req: Value, profile_only: bool, _: Duration) -> cua_byoc::Result<Value> {
        self.requests
            .lock()
            .unwrap()
            .push((req.clone(), profile_only));
        {
            let mut u = self.unavailable.lock().unwrap();
            if u.1 > 0 && req["op"] == u.0.as_str() {
                u.1 -= 1;
                return Ok(
                    json!({"error": {"kind": "unavailable", "message": "rpc error: code = Unavailable"}}),
                );
            }
        }
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

    // `--kind microvm` is the same engine: Modal's VM runtime.
    let mut micro = create("micro");
    micro.kind = cua_sandbox_core::placement::Kind::MicroVm;
    sandboxes.create(micro).await.unwrap();
    let last_create = fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(r, _)| r["op"] == "create")
        .unwrap()
        .0
        .clone();
    assert_eq!(last_create["runtime"], "vm");

    // Firecracker is a local microVM engine, not Modal's.
    let mut bad = create("bad");
    bad.runtime = Runtime::Firecracker;
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

fn target() -> Target {
    Target {
        provider: "modal".into(),
        environment: Some("cua-byoc".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_throttled_modal_call_is_retried_a_few_times_but_a_create_never() {
    let (_dir, fake, sandboxes) = world().await;
    let clouds = sandboxes.clouds().unwrap();
    // Two "unavailable" answers, then it works.
    *fake.unavailable.lock().unwrap() = ("check".into(), 2);
    clouds.connect(&target(), false, None).await.unwrap();
    let checks = |f: &FakeModal| {
        f.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| r["op"] == "check")
            .count()
    };
    assert_eq!(checks(&fake), 3);
    // Always unavailable: gives up after the retries, with the reason.
    *fake.unavailable.lock().unwrap() = ("check".into(), 100);
    let before = checks(&fake);
    let e = clouds.test(&target()).await.unwrap();
    assert!(!e.ok, "{e:?}");
    assert_eq!(checks(&fake) - before, 4, "one try and three retries");
    // A create may have happened before the answer: never repeated.
    *fake.unavailable.lock().unwrap() = ("create".into(), 1);
    let before_create = fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(r, _)| r["op"] == "create")
        .count();
    assert!(sandboxes.create(create("retry-me")).await.is_err());
    let after_create = fake
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(r, _)| r["op"] == "create")
        .count();
    assert_eq!(after_create - before_create, 1);
}

#[cfg(feature = "federation")]
mod sealed_support;

/// Modal's token sealed to the account's devices: checked, sealed, and
/// handed to the helper in its environment only (never in a request), with
/// no profile; a Modal token given without sealing is refused.
#[cfg(feature = "federation")]
#[tokio::test]
async fn a_sealed_modal_token_signs_in_the_helper_and_nothing_else_sees_it() {
    use cua_byoc::CloudApi as _;
    use cua_byoc::federation::IssuerClient;
    use cua_byoc::sealed::PoolSealer;
    use cua_sandbox_core::byoc::{CloudManager, ConnectRequest};
    use sealed_support::{FakeIssuer, FakePool};

    const SECRET: &str = "as-SEALED-modal-secret";
    let home = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeModal::default());
    let api = Arc::new(ModalApi::with(
        fake.clone(),
        Arc::new(FixedConfigSource(ImageConfig::default())),
        None,
    ));
    let issuer = Arc::new(FakeIssuer::default());
    let pool = FakePool::new();
    let clouds = cua_byoc::Clouds::new(
        home.path(),
        vec![api.clone() as Arc<dyn cua_byoc::CloudApi>],
        Arc::new(cua_byoc::Store::new(home.path())),
    )
    .with_issuer(issuer.clone() as Arc<dyn IssuerClient>)
    .with_sealer(pool.join("laptop") as Arc<dyn PoolSealer>);
    let ask = |token: &str, budget: Option<f64>| ConnectRequest {
        target: Target {
            provider: "modal".into(),
            token: Some(token.into()),
            ..Default::default()
        },
        budget_usd_month: budget,
        ..Default::default()
    };

    // Without a budget nothing is sealed, and a Modal token is not kept
    // on the device either: refused, saying how.
    let e = clouds
        .connect_with(&ask(&format!("ak-1:{SECRET}"), None))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("sealed"), "{e}");
    // Not a Modal token.
    let e = clouds
        .connect_with(&ask("just-one-word", Some(30.0)))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("<token id>:<token secret>"), "{e}");

    let c = clouds
        .connect_with(&ask(&format!("ak-1:{SECRET}"), Some(30.0)))
        .await
        .unwrap();
    assert_eq!(
        (c.provider.status.as_str(), c.provider.auth.as_str()),
        ("connected", "sealed"),
        "{:?}",
        c.next
    );
    let server = issuer.only("modal");
    assert_eq!(server.identifiers["workspace"], "default");
    fake.with_token.lock().unwrap().clear();

    // A later call opens it in memory and signs the helper in with it.
    let conn = clouds.store().connection("modal").unwrap().unwrap();
    assert!(conn.profile.is_empty());
    let t = api.test(&conn).await;
    assert!(t.ok(), "{:?}", t.checks);
    let calls = fake.with_token.lock().unwrap().clone();
    assert!(!calls.is_empty());
    for (req, id, secret) in &calls {
        assert_eq!((id.as_str(), secret.as_str()), ("ak-1", SECRET));
        assert!(!req.to_string().contains(SECRET), "{req}");
        assert!(req.get("profile").is_none(), "{req}");
    }
    let status = clouds.status(Some("modal")).await.unwrap();
    assert_eq!(status.providers[0].holders, vec!["laptop".to_string()]);
    assert!(!serde_json::to_string(&status).unwrap().contains(SECRET));
    assert!(
        !format!(
            "{:?}",
            ModalToken::parse(&format!("ak-1:{SECRET}")).unwrap()
        )
        .contains(SECRET)
    );
    // Profile tokens read for `--sealed`: the one named, else the active one.
    let toml = "[a]\ntoken_id = \"ak-a\"\ntoken_secret = \"as-a\"\n[b]\ntoken_id = \"ak-b\"\ntoken_secret = \"as-b\"\nactive = true\n";
    assert_eq!(
        cua_byoc::modal::profile_token(toml, None).unwrap().id,
        "ak-b"
    );
    assert_eq!(
        cua_byoc::modal::profile_token(toml, Some("a")).unwrap().id,
        "ak-a"
    );
    assert!(cua_byoc::modal::profile_token(toml, Some("c")).is_none());
}
