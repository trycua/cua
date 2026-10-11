//! Live Fleet: the registry-secret admission path. Skipped unless
//! `CUA_E2E_FLEET=1` (and Fleet credentials).
//!
//! One `cua-e2e-regsec-<rand>` pool with zero replicas (nothing is pulled)
//! whose template pairs a public image and a sidecar with a
//! `cua-registry-*` pull Secret holding throwaway credentials:
//! - the gateway admits the labeled `kubernetes.io/dockerconfigjson` Secret;
//! - the template is admitted with that `imagePullSecret` and its sidecar,
//!   on gVisor and on KubeVirt, and reads back with both.
//!
//! The pools (and their namespaces and Secrets) are deleted whatever
//! happened.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 cargo test -p cua-fleet --test live_registry_secret -- --nocapture
//! ```

use cua_fleet::{FleetClient, PoolOptions, RegistryCredentials, RuntimeKind, SandboxSpec, Sidecar};

fn enabled() -> bool {
    std::env::var("CUA_E2E_FLEET").as_deref() == Ok("1")
}

fn creds() -> RegistryCredentials {
    // Throwaway values: never a real credential.
    RegistryCredentials::new("cua-e2e", format!("cua-e2e-{:016x}", rand::random::<u64>()))
        .for_registry("ghcr.io")
}

fn spec(image: &str) -> SandboxSpec {
    SandboxSpec {
        registry_secret: Some(cua_fleet::registry_secret_name("ghcr.io", "cua-e2e")),
        sidecars: vec![Sidecar {
            name: "db".into(),
            ports: vec![6379],
            ..Sidecar::new("redis:7-alpine")
        }],
        services: [("db".to_string(), 6379)].into(),
        ..SandboxSpec::new(image)
    }
}

async fn run(
    fleet: &FleetClient,
    name: &str,
    runtime: RuntimeKind,
    image: &str,
) -> Result<(), String> {
    let options = PoolOptions {
        runtime: Some(runtime.clone()),
        replicas: Some(0),
        ..Default::default()
    };
    let spec = spec(image);
    let secret = spec.registry_secret.clone().unwrap();
    fleet
        .apply_with_credentials(name, &spec, &options, Some(&creds()))
        .await
        .map_err(|e| format!("apply {runtime:?}: {e}"))?;
    let (back, rt, raw) = fleet.pool_template(name).await.map_err(|e| e.to_string())?;
    let vm = &raw["spec"]["vmTemplate"];
    println!(
        "{name}: runtime={} imagePullSecret={} sidecars={}",
        vm["runtime"], vm["imagePullSecret"], vm["sidecars"]
    );
    if rt != runtime {
        return Err(format!("runtime {rt:?}, want {runtime:?}"));
    }
    if back.registry_secret.as_deref() != Some(secret.as_str()) {
        return Err(format!("pull secret not kept: {}", vm["imagePullSecret"]));
    }
    if back.sidecars.len() != 1 || back.sidecars[0].name != "db" {
        return Err(format!("sidecars not kept: {}", vm["sidecars"]));
    }
    Ok(())
}

async fn cleanup(fleet: &FleetClient, name: &str) {
    let _ = fleet
        .delete_registry_secret(name, &cua_fleet::registry_secret_name("ghcr.io", "cua-e2e"))
        .await;
    match fleet.get_pool(name).await {
        Ok(mut h) => {
            h.template = fleet
                .sdk()
                .get_template(name.to_string(), name.to_string())
                .await
                .ok();
            match fleet.delete_pool(h).await {
                Ok(()) => println!("deleted {name}"),
                Err(e) => eprintln!("CLEANUP FAILED for {name}: {e}"),
            }
        }
        Err(e) => eprintln!("cleanup: pool {name}: {e}"),
    }
}

#[tokio::test]
async fn live_registry_secret_and_sidecars_are_admitted_on_both_runtimes() {
    if !enabled() {
        eprintln!("skipped: set CUA_E2E_FLEET=1");
        return;
    }
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let tag = format!("{:08x}", rand::random::<u32>());
    let mut results = vec![];
    for (runtime, image, short) in [
        (
            RuntimeKind::Gvisor,
            "docker.io/library/python:3.12-slim",
            "gv",
        ),
        (RuntimeKind::Kubevirt, "ghcr.io/trycua/linux:24.04", "kv"),
    ] {
        let name = format!("cua-e2e-regsec-{short}-{tag}");
        let r = run(&fleet, &name, runtime, image).await;
        cleanup(&fleet, &name).await;
        results.push((name, r));
    }
    for (name, r) in &results {
        println!("{name}: {}", if r.is_ok() { "ok" } else { "FAILED" });
    }
    for (_, r) in results {
        r.unwrap();
    }
}
