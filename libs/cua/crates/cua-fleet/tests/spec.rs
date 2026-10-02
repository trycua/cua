//! The shared sandbox model against the fake control plane: `apply` is the
//! one pool writer, `check_pool_spec` refuses a named pool whose template
//! differs (with a readable diff), `apply_pool_template` reconciles it, and
//! `export_pool` reads a pool back into the model (and Terraform).

use cua_fleet::{
    Error, PoolOptions, ReadinessProbe, RuntimeKind, SandboxSpec, TtlPolicy, terraform_pool_block,
    testing::FakeFleet,
};
use std::time::Duration;

const IMAGE: &str = "ghcr.io/trycua/cua-e2e-plain:1";

fn spec() -> SandboxSpec {
    SandboxSpec {
        command: Some(vec!["python".into(), "-m".into(), "srv".into()]),
        services: [("mcp".to_string(), 8765)].into(),
        readiness: Some(ReadinessProbe::Http {
            port: 8765,
            path: "/health".into(),
        }),
        cpu: Some(2),
        memory_mb: Some(2048),
        ..SandboxSpec::new(IMAGE)
    }
}

fn gvisor() -> PoolOptions {
    PoolOptions {
        runtime: Some(RuntimeKind::Gvisor),
        warm: Some(true),
        max_pool_size: Some(3),
        idle_ttl: Some(Duration::from_secs(3600)),
        ttl_policy: Some(TtlPolicy::Retain),
        claim_ttl: Some(Duration::from_secs(600)),
        ..Default::default()
    }
}

#[tokio::test]
async fn apply_writes_the_pool_and_template_from_one_model() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let h = fleet
        .apply("cua-e2e-spec", &spec(), &gvisor())
        .await
        .unwrap();
    assert_eq!(h.runtime, Some(RuntimeKind::Gvisor));
    assert_eq!(h.claim_ttl, Some(Duration::from_secs(600)));
    let p = fake.object("pool", "cua-e2e-spec", "cua-e2e-spec").unwrap();
    assert_eq!(
        p["spec"]["autoscaling"],
        serde_json::json!({"minPoolSize": 1, "initialPoolSize": 1, "maxPoolSize": 3})
    );
    assert_eq!(p["spec"]["idleTtlSeconds"], 3600);
    assert_eq!(p["spec"]["ttlPolicy"], "Retain");
    let t = fake
        .object("template", "cua-e2e-spec", "cua-e2e-spec")
        .unwrap();
    let vm = &t["spec"]["vmTemplate"];
    assert_eq!(vm["command"][2], "srv");
    assert_eq!(vm["runtime"], "gvisor");
    assert_eq!(vm["probes"]["readinessProbe"]["httpGet"]["port"], 8765);
    assert_eq!(vm["memory"], "2048Mi");

    // Reading it back gives the same model.
    let (s, o, rt) = fleet.export_pool("cua-e2e-spec").await.unwrap();
    assert_eq!(rt, RuntimeKind::Gvisor);
    assert_eq!(s.command, spec().command);
    assert_eq!(s.readiness, spec().readiness);
    assert_eq!(o.warm, Some(true));
    assert_eq!(o.idle_ttl, Some(Duration::from_secs(3600)));
    let hcl = terraform_pool_block("cua-e2e-spec", &s, &o, &rt);
    assert!(hcl.contains("container_disk_image = \"ghcr.io/trycua/cua-e2e-plain:1\""));
    assert!(hcl.contains("readiness_probe_json = jsonencode("), "{hcl}");
    assert!(hcl.contains("\n  idle_ttl_seconds = 3600\n"), "{hcl}");
    assert!(hcl.contains("max_pool_size = 3"), "{hcl}");
    fleet.delete_pool(h).await.unwrap();
}

#[tokio::test]
async fn a_named_pool_with_a_different_template_is_a_mismatch() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    fleet.apply("cua-e2e-mm", &spec(), &gvisor()).await.unwrap();

    // Same fields (and unset ones) match.
    fleet.check_pool_spec("cua-e2e-mm", &spec()).await.unwrap();
    fleet
        .check_pool_spec("cua-e2e-mm", &SandboxSpec::default())
        .await
        .unwrap();
    let only_command = SandboxSpec {
        command: spec().command,
        ..Default::default()
    };
    fleet
        .check_pool_spec("cua-e2e-mm", &only_command)
        .await
        .unwrap();

    let other = SandboxSpec {
        command: Some(vec!["node".into(), "srv.js".into()]),
        cpu: Some(4),
        ..Default::default()
    };
    let err = fleet
        .check_pool_spec("cua-e2e-mm", &other)
        .await
        .unwrap_err();
    let Error::PoolSpecMismatch { pool, diffs } = &err else {
        panic!("{err:?}")
    };
    assert_eq!(pool, "cua-e2e-mm");
    let fields: Vec<&str> = diffs.iter().map(|d| d.field.as_str()).collect();
    assert_eq!(fields, ["command", "cpu"]);
    let msg = err.to_string();
    assert!(
        msg.contains(
            "command: pool has [\"python\", \"-m\", \"srv\"], requested [\"node\", \"srv.js\"]"
        ),
        "{msg}"
    );
    assert!(msg.contains("cpu: pool has 2, requested 4"), "{msg}");
    assert!(msg.contains("apply=True"), "{msg}");

    // A different image is a mismatch too.
    let err = fleet
        .check_pool_spec("cua-e2e-mm", &SandboxSpec::new("ghcr.io/trycua/other:2"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PoolSpecMismatch { .. }), "{err:?}");

    // apply=True reconciles only the given fields and keeps the rest.
    let before = fake.object("pool", "cua-e2e-mm", "cua-e2e-mm").unwrap();
    fleet
        .apply_pool_template("cua-e2e-mm", &other, None)
        .await
        .unwrap();
    fleet.check_pool_spec("cua-e2e-mm", &other).await.unwrap();
    let t = fake.object("template", "cua-e2e-mm", "cua-e2e-mm").unwrap();
    let vm = &t["spec"]["vmTemplate"];
    assert_eq!(vm["command"][0], "node");
    assert_eq!(vm["cpuCores"], 4);
    assert_eq!(vm["services"][0]["name"], "mcp", "kept");
    assert_eq!(vm["memory"], "2048Mi", "kept");
    assert_eq!(
        fake.object("pool", "cua-e2e-mm", "cua-e2e-mm").unwrap()["spec"],
        before["spec"],
        "the pool's capacity is untouched"
    );
    // Nothing to do: no write.
    let n = fake.requests().len();
    fleet
        .apply_pool_template("cua-e2e-mm", &other, None)
        .await
        .unwrap();
    assert!(
        !fake.requests()[n..]
            .iter()
            .any(|r| r.method == "PATCH" || r.method == "POST"),
        "no-op apply writes nothing"
    );
}

#[tokio::test]
async fn a_spec_a_runtime_cannot_run_fails_before_anything_is_written() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let kv = PoolOptions {
        runtime: Some(RuntimeKind::Kubevirt),
        ..Default::default()
    };
    let with_command = SandboxSpec {
        command: Some(vec!["x".into()]),
        ..SandboxSpec::new(IMAGE)
    };
    // An explicit Legacy cannot run a command on KubeVirt.
    let legacy = SandboxSpec {
        process_mode: Some(cua_fleet::ProcessMode::Legacy),
        ..with_command
    };
    let err = fleet.apply("cua-e2e-kv", &legacy, &kv).await.unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    assert!(fake.all_namespaces().is_empty());
    let bad = SandboxSpec {
        env: [("1BAD".to_string(), "x".to_string())].into(),
        ..SandboxSpec::new(IMAGE)
    };
    assert!(fleet.apply("cua-e2e-kv", &bad, &gvisor()).await.is_err());
    assert!(fake.all_namespaces().is_empty());
}

#[tokio::test]
async fn a_pool_can_reference_a_registry_secret_end_to_end() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    let creds = cua_fleet::RegistryCredentials::new("me", "hunter2");
    let secret = cua_fleet::registry_secret_name("ghcr.io", "me");
    let s = SandboxSpec {
        registry_secret: Some(secret.clone()),
        ..SandboxSpec::new("ghcr.io/me/private:1")
    };
    fleet
        .apply_with_credentials("cua-e2e-reg", &s, &gvisor(), Some(&creds))
        .await
        .unwrap();
    // The Secret carries the label the gateway requires (403 without it).
    let sec = fake.object("secret", "cua-e2e-reg", &secret).unwrap();
    assert_eq!(sec["metadata"]["labels"]["cua.ai/registry-secret"], "true");
    assert_eq!(sec["type"], "kubernetes.io/dockerconfigjson");
    let t = fake
        .object("template", "cua-e2e-reg", "cua-e2e-reg")
        .unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["imagePullSecret"], secret);
    // The password never appears outside the Secret POST.
    for r in fake.requests() {
        if !r.path.ends_with("/secrets") {
            let body = r.body.map(|b| b.to_string()).unwrap_or_default();
            assert!(!body.contains("hunter2"), "{} {}", r.method, r.path);
        }
    }
    // And the mismatch check sees it.
    fleet.check_pool_spec("cua-e2e-reg", &s).await.unwrap();
}

#[tokio::test]
async fn the_fake_refuses_a_registry_secret_without_the_label() {
    use cua_fleet::sdk::{HttpClient, HttpRequest};
    let fake = FakeFleet::new();
    let body = cua_fleet::parity::registry_secret_body(
        "ns",
        "cua-registry-x",
        "ghcr.io",
        &cua_fleet::RegistryCredentials::new("me", "pw"),
    )
    .unwrap();
    let post = |body: serde_json::Value| -> HttpRequest {
        serde_json::from_value(serde_json::json!({
            "method": "POST",
            "url": "https://fleet.test/api/k8s/api/v1/namespaces/ns/secrets",
            "headers": [],
            "body": serde_json::to_vec(&body).unwrap(),
        }))
        .unwrap()
    };
    assert_eq!(fake.execute(post(body.clone())).await.unwrap().status, 201);
    let mut unlabeled = body;
    unlabeled["metadata"]["name"] = "cua-registry-y".into();
    unlabeled["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("labels");
    assert_eq!(fake.execute(post(unlabeled)).await.unwrap().status, 403);
}
