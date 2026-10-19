//! `cua fleet pools ls|gc`: the managed Fleet pools behind
//! `cua sandbox create` without `--pool`.

use crate::{auth, util::line};
use cua_fleet::{ManagedPoolInfo, PoolManager};
use cua_sdk::CuaError;
use std::{io::Write, path::Path, time::Duration};

async fn manager(state_dir: Option<&str>) -> Result<PoolManager, CuaError> {
    let (client, _) = auth::fleet_client().await?;
    let mut cfg = cua_sandbox_core::settings::auto_pool_config();
    if let Some(d) = state_dir {
        cfg = cfg.with_state_dir(Path::new(d));
    }
    Ok(PoolManager::new(client, cfg))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn ago(t: Option<i64>) -> String {
    match t {
        Some(t) => format!(
            "{} ago",
            humantime::format_duration(Duration::from_secs((now() - t).max(0) as u64))
        ),
        None => "-".into(),
    }
}

fn row(p: &ManagedPoolInfo) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "managed": p.managed,
        "spec_hash": p.spec_hash,
        "image": p.image,
        "replicas": p.replicas,
        "ready_replicas": p.ready_replicas,
        "max_pool_size": p.max_pool_size,
        "claims": p.claims,
        "bound_claims": p.bound_claims,
        "last_used": p.last_used,
        "created": p.created,
        "expires_at": p.expires_at,
        "terminating": p.terminating,
    })
}

/// `cua fleet pools ls`.
pub async fn list(
    state_dir: Option<&str>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let pools = manager(state_dir)
        .await?
        .list()
        .await
        .map_err(auth::fleet_err)?;
    if json {
        line(
            out,
            serde_json::Value::Array(pools.iter().map(row).collect()).to_string(),
        );
        return Ok(0);
    }
    if pools.is_empty() {
        line(out, "no managed pools");
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<36} {:>9} {:>7} {:<18} IMAGE",
            "NAME", "REPLICAS", "CLAIMS", "LAST USED"
        ),
    );
    for p in &pools {
        let replicas = format!(
            "{}/{}",
            p.ready_replicas.unwrap_or(0),
            p.max_pool_size.map_or("-".into(), |m| m.to_string())
        );
        let mut name = p.name.clone();
        if p.terminating {
            name.push_str(" (deleting)");
        }
        line(
            out,
            format!(
                "{name:<36} {replicas:>9} {:>7} {:<18} {}",
                p.claims,
                ago(p.last_used),
                p.image.as_deref().unwrap_or("-")
            ),
        );
    }
    Ok(0)
}

/// `cua fleet pools gc`.
pub async fn gc(
    state_dir: Option<&str>,
    idle: &str,
    only: &[String],
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let idle = humantime::parse_duration(idle)
        .map_err(|e| CuaError::InvalidArgument(format!("--idle {idle:?}: {e}")))?;
    let mgr = manager(state_dir).await?;
    let r = if only.is_empty() {
        mgr.gc(idle).await
    } else {
        mgr.gc_pools(idle, only).await
    }
    .map_err(auth::fleet_err)?;
    if json {
        line(
            out,
            serde_json::json!({
                "deleted_pools": r.deleted_pools,
                "deleted_namespaces": r.deleted_namespaces,
                "deleted_claims": r.deleted_claims,
                "kept": r.kept,
                "errors": r.errors,
            })
            .to_string(),
        );
    } else {
        for p in &r.deleted_pools {
            line(out, format!("deleted pool {p}"));
        }
        for n in &r.deleted_namespaces {
            line(out, format!("deleted namespace {n}"));
        }
        for c in &r.deleted_claims {
            line(out, format!("deleted stuck claim {c}"));
        }
        line(
            out,
            format!(
                "{} deleted, {} kept",
                r.deleted_pools.len() + r.deleted_namespaces.len(),
                r.kept.len()
            ),
        );
        for e in &r.errors {
            line(out, format!("error: {e}"));
        }
    }
    Ok(if r.errors.is_empty() { 0 } else { 1 })
}

/// `cua fleet pool export NAME [--terraform]`: a pool read back as the
/// shared sandbox model (JSON), or as the Terraform `fleets_pool` block for
/// the same fields (sidecars as `sidecar {}` blocks).
pub async fn export(name: &str, terraform: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (client, _) = auth::fleet_client().await?;
    export_with(&client, name, terraform, out).await
}

/// [`export`] with a given client (tests use the fake Fleet).
pub async fn export_with(
    client: &cua_fleet::FleetClient,
    name: &str,
    terraform: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (spec, options, runtime) = client.export_pool(name).await.map_err(auth::fleet_err)?;
    if terraform {
        let _ = write!(
            out,
            "{}",
            cua_fleet::terraform_pool_block(name, &spec, &options, &runtime)
        );
        return Ok(0);
    }
    let secs = |d: Option<Duration>| d.map(|d| d.as_secs());
    line(
        out,
        serde_json::json!({
            "name": name,
            "runtime": cua_fleet::runtime_name(&runtime),
            "spec": spec,
            "options": {
                "replicas": options.replicas,
                "warm": options.warm,
                "min_pool_size": options.min_pool_size,
                "max_pool_size": options.max_pool_size,
                "idle_ttl_seconds": secs(options.idle_ttl),
                "ttl_policy": options.ttl_policy.map(|p| p.as_str()),
                "pool_ttl_seconds": secs(options.pool_ttl),
            },
        })
        .to_string(),
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_prints_the_pool_as_terraform_and_json() {
        let fake = cua_fleet::testing::FakeFleet::new();
        let fleet = fake.client();
        let spec = cua_fleet::SandboxSpec {
            command: Some(vec!["python".into(), "-m".into(), "srv".into()]),
            services: [("mcp".to_string(), 8765)].into(),
            cpu: Some(2),
            memory_mb: Some(4096),
            sidecars: vec![cua_fleet::Sidecar {
                name: "db".into(),
                args: Some(vec!["--save".into(), String::new()]),
                ports: vec![6379],
                cpu: Some("250m".into()),
                memory: Some("256Mi".into()),
                ..cua_fleet::Sidecar::new("redis:7-alpine")
            }],
            ..cua_fleet::SandboxSpec::new("ghcr.io/trycua/cua-e2e-x:1")
        };
        let options = cua_fleet::PoolOptions {
            runtime: Some(cua_fleet::RuntimeKind::Gvisor),
            warm: Some(true),
            idle_ttl: Some(Duration::from_secs(3600)),
            ..Default::default()
        };
        fleet.apply("cua-e2e-exp", &spec, &options).await.unwrap();
        let mut hcl = Vec::new();
        assert_eq!(
            export_with(&fleet, "cua-e2e-exp", true, &mut hcl)
                .await
                .unwrap(),
            0
        );
        let hcl = String::from_utf8(hcl).unwrap();
        for want in [
            "resource \"fleets_pool\" \"cua_e2e_exp\" {",
            "  cpu_cores = 2",
            "  memory = \"4096Mi\"",
            "  runtime = \"gvisor\"",
            "  command = [\"python\", \"-m\", \"srv\"]",
            "  process_mode = \"Run\"",
            "  idle_ttl_seconds = 3600",
            // Read back from the template, written as a provider block.
            "\n  sidecar {\n    name = \"db\"\n    image = \"redis:7-alpine\"\n    \
             args = [\"--save\", \"\"]\n    ports = [6379]\n    cpu = \"250m\"\n    \
             memory = \"256Mi\"\n  }\n",
            "    target_port = 8765",
            "    min_pool_size = 1",
        ] {
            assert!(hcl.contains(want), "missing {want:?} in\n{hcl}");
        }
        assert!(!hcl.contains('#'), "nothing is commented out:\n{hcl}");
        let mut js = Vec::new();
        export_with(&fleet, "cua-e2e-exp", false, &mut js)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&js).unwrap();
        assert_eq!(v["spec"]["command"][2], "srv");
        assert_eq!(v["options"]["idle_ttl_seconds"], 3600);
        assert_eq!(v["runtime"], "gvisor");
        assert!(
            export_with(&fleet, "cua-e2e-missing", true, &mut Vec::new())
                .await
                .is_err()
        );
    }
}
