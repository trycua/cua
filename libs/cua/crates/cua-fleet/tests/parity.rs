#![allow(deprecated)] // exercises the deprecated `apply_pool` wrapper too
//! Sandbox parity against the fake Fleet: sidecars on every runtime
//! (trycua/cloud#7893), `cua-registry-*` pull Secrets (#7887), and remote
//! image builds.
//!
//! Remote builds are behind the `fleet-remote-builds` feature (Fleet's
//! builder does not run them yet): without it only their "not deployed yet"
//! test runs.

use base64::Engine as _;
use cua_fleet::{
    AcquireOpts, AutoPoolConfig, PoolManager, PoolSpec, PoolSpecKey, RegistryCredentials,
    RuntimeKind, Sidecar, testing::FakeFleet,
};
use serde_json::Value;
use std::time::Duration;

const IMAGE: &str = "docker.io/library/python@sha256:0123";

fn cfg(fake: &FakeFleet, home: &std::path::Path) -> AutoPoolConfig {
    AutoPoolConfig {
        home: home.to_path_buf(),
        idle_gc: None,
        heartbeat_every: Some(Duration::from_millis(20)),
        clock: fake.clock(),
        ..AutoPoolConfig::default()
    }
}

fn redis() -> Sidecar {
    Sidecar {
        ports: vec![6379],
        env: [("REDIS_ARGS".to_string(), "--save ''".to_string())].into(),
        ..Sidecar::new("redis:7-alpine")
    }
}

fn gvisor_key() -> PoolSpecKey {
    PoolSpecKey::new(IMAGE)
        .runtime(RuntimeKind::Gvisor)
        .resources(Some(2), Some(2048))
}

fn decode_secret(secret: &Value) -> Value {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(secret["data"][".dockerconfigjson"].as_str().unwrap())
        .unwrap();
    serde_json::from_slice(&raw).unwrap()
}

#[test]
fn spec_hash_changes_only_when_parity_fields_are_set() {
    let base = gvisor_key();
    let before = base.canonical();
    assert!(!before.contains("sidecars") && !before.contains("pull_secret"));
    let with_sidecar = PoolSpecKey {
        sidecars: vec![redis()],
        ..base.clone()
    };
    let with_secret = PoolSpecKey {
        pull_secret: Some("cua-registry-0123456789abcdef".into()),
        ..base.clone()
    };
    assert_ne!(base.spec_hash(), with_sidecar.spec_hash());
    assert_ne!(base.spec_hash(), with_secret.spec_hash());
    assert_ne!(with_sidecar.spec_hash(), with_secret.spec_hash());
    let other_port = PoolSpecKey {
        sidecars: vec![Sidecar {
            ports: vec![6380],
            ..redis()
        }],
        ..base.clone()
    };
    assert_ne!(with_sidecar.spec_hash(), other_port.spec_hash());
}

#[cfg(not(feature = "fleet-remote-builds"))]
#[tokio::test]
async fn remote_builds_fail_clearly_until_fleet_runs_them() {
    let fake = FakeFleet::new();
    let build = cua_fleet::builds::BuildSpec {
        from: "python:3.12-slim".into(),
        layers: vec![cua_image::spec::ImageLayer::Run {
            command: "true".into(),
        }],
        ..Default::default()
    };
    let err = fake
        .client()
        .build_image(&build, None, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not deployed yet"), "{err}");
    assert!(fake.all_namespaces().is_empty(), "nothing was created");
}

mod enabled {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn managed_pool_template_carries_sidecars_and_the_pull_secret() {
        let fake = FakeFleet::new();
        let home = tempfile::tempdir().unwrap();
        let mgr = PoolManager::new(fake.client_for_tenant("alice"), cfg(&fake, home.path()));
        let secret = cua_fleet::registry_secret_name("ghcr.io", "me");
        let key = PoolSpecKey {
            sidecars: vec![redis()],
            pull_secret: Some(secret.clone()),
            ..PoolSpecKey::new("ghcr.io/me/private@sha256:0123")
                .runtime(RuntimeKind::Gvisor)
                .services([("mcp", 8765u16)])
        };
        let creds = RegistryCredentials::new("me", "tok-1").for_registry("ghcr.io");
        let opts = AcquireOpts {
            registry_credentials: Some(creds.clone()),
            ..AcquireOpts::default()
        };
        let c = mgr.acquire(key.clone(), opts.clone()).await.unwrap();
        let pool = c.pool.clone();
        let t = fake.object("template", &c.pool, &c.pool).unwrap();
        let vm = &t["spec"]["vmTemplate"];
        assert_eq!(vm["imagePullSecret"], json!(secret));
        assert_eq!(
            vm["sidecars"],
            json!([{"name": "redis", "image": "redis:7-alpine",
                "env": {"REDIS_ARGS": "--save ''"}, "ports": [6379]}])
        );
        assert_eq!(vm["runtime"], "gvisor");
        let s = fake
            .object("secret", &c.pool, &secret)
            .expect("pull secret");
        assert_eq!(s["type"], "kubernetes.io/dockerconfigjson");
        assert_eq!(decode_secret(&s)["auths"]["ghcr.io"]["password"], "tok-1");
        // The secret was written before the template.
        let reqs = fake.requests();
        let secret_at = reqs
            .iter()
            .position(|r| r.method == "POST" && r.path.ends_with("/secrets"))
            .unwrap();
        let template_at = reqs
            .iter()
            .position(|r| r.method == "POST" && r.path.ends_with("/osgymsandboxtemplates"))
            .unwrap();
        assert!(secret_at < template_at);
        // No request or error carries the password outside the Secret body.
        for r in &reqs {
            if !r.path.ends_with("/secrets") {
                assert!(!format!("{r:?}").contains("tok-1"), "leaked in {}", r.path);
            }
        }
        c.release().await.unwrap();

        // A rotated password (same user) keeps the pool and rewrites the
        // secret (delete + create: the gateway admits no update).
        let rotated = AcquireOpts {
            registry_credentials: Some(
                RegistryCredentials::new("me", "tok-2").for_registry("ghcr.io"),
            ),
            ..AcquireOpts::default()
        };
        let c2 = mgr.acquire(key, rotated).await.unwrap();
        assert_eq!(c2.pool, pool, "same pool");
        let s = fake.object("secret", &c2.pool, &secret).unwrap();
        assert_eq!(decode_secret(&s)["auths"]["ghcr.io"]["password"], "tok-2");
        assert!(
            fake.requests()
                .iter()
                .any(|r| r.method == "DELETE" && r.path.ends_with(&format!("/secrets/{secret}")))
        );
        c2.release().await.unwrap();
    }

    #[tokio::test]
    async fn kubevirt_templates_carry_sidecars_and_bad_ones_are_refused() {
        let fake = FakeFleet::new();
        let home = tempfile::tempdir().unwrap();
        let mgr = PoolManager::new(fake.client_for_tenant("alice"), cfg(&fake, home.path()));
        // IMAGE is a KubeVirt containerDisk: its sidecars run in Fleet's
        // companion gVisor pod, addressed by name like on gVisor.
        let vm = PoolSpecKey {
            sidecars: vec![redis()],
            ..PoolSpecKey::new(IMAGE).services([("db", 6379u16)])
        };
        let c = mgr.acquire(vm, AcquireOpts::default()).await.unwrap();
        let t = fake.object("template", &c.pool, &c.pool).unwrap();
        let vmt = &t["spec"]["vmTemplate"];
        assert_eq!(vmt["runtime"], "kubevirt");
        assert_eq!(vmt["sidecars"][0]["name"], "redis");
        assert_eq!(vmt["services"][0]["name"], "db");
        c.release().await.unwrap();
        let before = fake.all_namespaces();

        // A pod sidecar may not take spacesd's port.
        let clash = PoolSpecKey {
            sidecars: vec![Sidecar {
                ports: vec![3211],
                ..redis()
            }],
            ..gvisor_key()
        };
        let err = mgr
            .acquire(clash, AcquireOpts::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("3211"), "{err}");
        // With sidecars, main / sidecars / sc are reserved service names.
        for reserved in cua_fleet::RESERVED_SERVICE_NAMES {
            let key = PoolSpecKey {
                sidecars: vec![redis()],
                ..gvisor_key().services([(reserved, 8080u16)])
            };
            let err = mgr
                .acquire(key, AcquireOpts::default())
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("reserved"), "{err}");
        }
        assert_eq!(fake.all_namespaces(), before, "nothing more was created");
    }

    #[tokio::test]
    async fn the_fake_admits_sidecars_like_fleet() {
        use cua_fleet::sdk::{HttpClient, HttpRequest};
        let fake = FakeFleet::new();
        fake.add_namespace("ns");
        let post = |vm: Value| -> HttpRequest {
            let body = json!({"apiVersion": "osgym.cua.ai/v1alpha1",
                "kind": "OSGymSandboxTemplate", "metadata": {"name": "t", "namespace": "ns"},
                "spec": {"vmTemplate": vm}});
            serde_json::from_value(json!({
                "method": "POST",
                "url": "https://fleet.test/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/ns/osgymsandboxtemplates",
                "headers": [],
                "body": serde_json::to_vec(&body).unwrap(),
            }))
            .unwrap()
        };
        let status = |r: HttpRequest| {
            let fake = fake.clone();
            async move { fake.execute(r).await.unwrap().status }
        };
        let reserved = json!({"containerDiskImage": "img",
            "sidecars": [{"name": "redis", "image": "redis:7"}],
            "services": [{"name": "sc", "targetPort": 80}]});
        assert_eq!(status(post(reserved)).await, 403);
        let main = json!({"containerDiskImage": "img",
            "sidecars": [{"name": "main", "image": "redis:7"}]});
        assert_eq!(status(post(main)).await, 403);
        // Without sidecars the names are free.
        let free = json!({"containerDiskImage": "img",
            "services": [{"name": "main", "targetPort": 80}]});
        assert_eq!(status(post(free)).await, 201);
    }

    #[tokio::test]
    async fn apply_pool_writes_sidecars_and_rolls_back_on_failure() {
        let fake = FakeFleet::new();
        let fleet = fake.client();
        let spec = PoolSpec {
            sidecars: vec![redis()],
            ..PoolSpec::new("cua-e2e-sc", IMAGE).runtime(RuntimeKind::Gvisor)
        };
        let h = fleet.apply_pool(&spec).await.unwrap();
        let t = fake.object("template", "cua-e2e-sc", "cua-e2e-sc").unwrap();
        assert_eq!(t["spec"]["vmTemplate"]["sidecars"][0]["name"], "redis");
        // Re-applying without sidecars removes them (explicit null).
        let plain = PoolSpec::new("cua-e2e-sc", IMAGE).runtime(RuntimeKind::Gvisor);
        let spec2 = PoolSpec {
            image_pull_secret: Some("cua-registry-abc".into()),
            ..plain
        };
        fleet.apply_pool(&spec2).await.unwrap();
        let t = fake.object("template", "cua-e2e-sc", "cua-e2e-sc").unwrap();
        assert!(t["spec"]["vmTemplate"]["sidecars"].is_null(), "{t}");
        assert_eq!(
            t["spec"]["vmTemplate"]["imagePullSecret"],
            "cua-registry-abc"
        );
        fleet.delete_pool(h).await.unwrap();
    }
}

#[cfg(feature = "fleet-remote-builds")]
mod remote_builds {
    use super::*;
    use cua_fleet::builds::{BuildFile, BuildSpec};
    use cua_image::spec::ImageLayer;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn remote_builds_are_cached_by_content_and_report_progress() {
        let fake = FakeFleet::new();
        fake.faults.lock().unwrap().build_reads = 2;
        let fleet = fake.client_for_tenant("alice");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("server.py");
        std::fs::write(&file, b"print('hi')\n").unwrap();
        let spec = BuildSpec {
            from: "python:3.12-slim".into(),
            layers: vec![ImageLayer::PipInstall {
                packages: vec!["mcp".into()],
            }],
            env: [("A".to_string(), "1".to_string())].into(),
            ports: vec![8765],
            files: vec![BuildFile {
                source: file.clone(),
                destination: "/srv/server.py".into(),
            }],
            timeout: Some(Duration::from_secs(30)),
        };
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&seen);
        let progress = move |m: &str| sink.lock().unwrap().push(m.to_string());
        let built = fleet
            .build_image(&spec, None, Some(&progress))
            .await
            .unwrap();
        assert!(!built.cached);
        assert!(built.reference.contains("@sha256:"), "{}", built.reference);
        assert!(built.name.starts_with("cua-b-"));
        assert!(built.namespace.starts_with("cua-build-"));
        let lines = seen.lock().unwrap().clone();
        assert!(lines.iter().any(|l| l.contains("Building")), "{lines:?}");
        let img = fake.object("image", &built.namespace, &built.name).unwrap();
        let recipe = &img["spec"]["recipe"];
        assert_eq!(recipe["kind"], "container");
        assert_eq!(recipe["from"], "python:3.12-slim");
        assert_eq!(recipe["layers"][0]["type"], "pip_install");
        assert_eq!(recipe["env"]["A"], "1");
        assert!(
            recipe["files"][0]["source"]["reference"]
                .as_str()
                .unwrap()
                .starts_with("uploads/tenant-")
        );
        assert_eq!(recipe["files"][0]["destination"], "/srv/server.py");
        // Only name + namespace (the gateway's image admission refuses
        // other metadata keys).
        let meta: Vec<&String> = img["metadata"].as_object().unwrap().keys().collect();
        assert_eq!(meta, vec!["name", "namespace"], "{meta:?}");

        // Identical spec, another process: the existing build is reused.
        let creates = |f: &FakeFleet| {
            f.requests()
                .iter()
                .filter(|r| r.method == "POST" && r.path.ends_with("/images"))
                .count()
        };
        assert_eq!(creates(&fake), 1);
        let again = fleet.build_image(&spec, None, None).await.unwrap();
        assert_eq!(again.reference, built.reference);
        assert_eq!(creates(&fake), 1, "no rebuild");
        // Different content builds anew.
        let changed = BuildSpec {
            layers: vec![ImageLayer::Run {
                command: "true".into(),
            }],
            ..spec.clone()
        };
        let other = fleet.build_image(&changed, None, None).await.unwrap();
        assert_ne!(other.name, built.name);
        assert_eq!(creates(&fake), 2);
    }

    #[tokio::test]
    async fn private_bases_use_a_pull_secret_and_failures_are_reported() {
        let fake = FakeFleet::new();
        let fleet = fake.client_for_tenant("bob");
        let spec = BuildSpec {
            from: "ghcr.io/me/base:1".into(),
            layers: vec![ImageLayer::Run {
                command: "true".into(),
            }],
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        };
        let creds = RegistryCredentials::new("me", "pw-1");
        let built = fleet.build_image(&spec, Some(&creds), None).await.unwrap();
        let img = fake.object("image", &built.namespace, &built.name).unwrap();
        let secret = img["spec"]["recipe"]["fromPullSecret"].as_str().unwrap();
        assert_eq!(secret, cua_fleet::registry_secret_name("ghcr.io", "me"));
        let s = fake.object("secret", &built.namespace, secret).unwrap();
        assert_eq!(decode_secret(&s)["auths"]["ghcr.io"]["username"], "me");

        fake.faults.lock().unwrap().fail_build = true;
        let failing = BuildSpec {
            layers: vec![ImageLayer::Run {
                command: "exit 1".into(),
            }],
            ..spec
        };
        let err = fleet
            .build_image(&failing, None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("failed") && err.contains("RUN exited 1"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_fleet_without_the_images_api_says_so() {
        let fake = FakeFleet::new();
        fake.faults.lock().unwrap().no_images_api = true;
        let spec = BuildSpec {
            from: "python:3.12-slim".into(),
            layers: vec![ImageLayer::Run {
                command: "true".into(),
            }],
            ..Default::default()
        };
        let err = fake
            .client()
            .build_image(&spec, None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not deployed yet"), "{err}");
    }
}
