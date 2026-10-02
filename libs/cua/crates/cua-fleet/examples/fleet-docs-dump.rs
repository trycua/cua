//! Prints, as JSON, the Fleet facts the docs.cua.ai Fleets reference is
//! generated from (`scripts/docs-generators/fleet.ts`):
//!
//! - `crds`: the `osgym.cua.ai` CustomResourceDefinitions (Sandbox,
//!   Template, WarmPool, Claim) exactly as `cyclops-sdk-schema` renders
//!   them for the Fleet operator;
//! - `config`: the default endpoints and the missing-credentials message;
//! - `secrets`: the Secrets the SDK writes, built by the same functions
//!   the SDK calls, with placeholder values;
//! - `extensions`: fields the SDK writes as JSON beyond the CRD schema;
//! - `examples`: request bodies built by the SDK's own spec types for a
//!   default pool of the cua Linux image.
//!
//! Needs no credentials and no network.
//!
//! ```sh
//! cargo run -q -p cua-fleet --example fleet-docs-dump
//! ```

use cua_fleet::{claim_secrets, parity, schema};
use serde_json::{Value, json};

fn main() {
    let crds = schema::generate::render_crds().expect("render CRDs");
    let crds: Vec<Value> = schema::generate::semantic_yaml_documents(&crds).expect("parse CRDs");

    let registry = "ghcr.io";
    let registry_secret = parity::registry_secret_name(registry, "octocat");
    let registry_body = parity::registry_secret_body(
        "my-pool",
        &registry_secret,
        registry,
        &cua_fleet::RegistryCredentials::new("octocat", "<token>"),
    )
    .expect("registry secret body");

    let extensions = json!([
        {
            "object": "Template",
            "field": "spec.vmTemplate.claimSecrets",
            "type": "boolean",
            "description": "Opts the template into per-claim Secrets: the Secret a claim names in `spec.secretRef` is delivered into the bound sandbox at `/run/cua/env-token`.",
        },
        {
            "object": "Claim",
            "field": "spec.secretRef.name",
            "type": "string",
            "description": format!("The claim's `{}<claim>` Secret. The template must set `claimSecrets`.", claim_secrets::CLAIM_SECRET_PREFIX),
        },
        {
            "object": "Template",
            "field": "spec.vmTemplate.env",
            "type": "map of string",
            "description": "Environment of the sandbox process.",
        },
        {
            "object": "Template",
            "field": "spec.vmTemplate.args",
            "type": "string[]",
            "description": "Arguments of the sandbox process.",
        },
        {
            "object": "Template",
            "field": "spec.vmTemplate.sidecars",
            "type": "object[]",
            "description": format!("Containers that run next to the sandbox, on every runtime (at most {}). They reach the sandbox at host `{}`.", parity::MAX_SIDECARS, parity::MAIN_CONTAINER_NAME),
        },
        {
            "object": "Template",
            "field": "spec.vmTemplate.processMode",
            "type": "\"Legacy\" | \"Run\"",
            "description": "`Legacy`: pod runtimes run `command`, `args` and `env`; KubeVirt ignores `command` and refuses `args` and `env`. `Run`: every runtime runs them (KubeVirt through cloud-init).",
        },
    ]);

    // The SDK's defaults: one replica, the `env` service on 3211.
    let spec = cua_fleet::PoolSpec::new("my-pool", "ghcr.io/trycua/linux:24.04-disk");
    let pool = spec.pool_request();
    let claim = schema::ClaimSpec {
        sandbox_template_ref: pool.spec.sandbox_template_ref.clone(),
        warmpool: None,
        bind_deadline: Some(schema::DEFAULT_CLAIM_BIND_DEADLINE_SECONDS),
        lifecycle: None,
        ttl_seconds_after_created: None,
        secret_ref: None,
    };
    let examples = json!({
        "pool": {
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxWarmPool",
            "metadata": {"namespace": pool.namespace, "name": pool.namespace},
            "spec": serde_json::to_value(&pool.spec).expect("pool spec"),
        },
        "template": spec.template_json().expect("template"),
        "claim": {
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxClaim",
            "metadata": {"namespace": pool.namespace, "name": "claim-1"},
            "spec": serde_json::to_value(&claim).expect("claim spec"),
        },
    });

    let out = json!({
        "crds": crds,
        "config": {
            "default_base_url": cua_fleet::DEFAULT_FLEET_BASE_URL,
            "default_token_url": cua_fleet::DEFAULT_TOKEN_URL,
            "missing_credentials": cua_fleet::MISSING_CREDENTIALS,
            "default_claim_bind_deadline_seconds": schema::DEFAULT_CLAIM_BIND_DEADLINE_SECONDS,
            "remote_builds_supported": parity::REMOTE_BUILDS_SUPPORTED,
        },
        "examples": examples,
        "extensions": extensions,
        "secrets": {
            "claim": {
                "prefix": claim_secrets::CLAIM_SECRET_PREFIX,
                "key": claim_secrets::ENV_TOKEN_KEY,
                "label": claim_secrets::CLAIM_SECRET_CLAIM_LABEL,
                "wait_seconds": claim_secrets::DEFAULT_WAIT.as_secs(),
                "example": claim_secrets::claim_secret_body(
                    "my-pool",
                    "claim-1",
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                ),
            },
            "registry": {
                "prefix": parity::REGISTRY_SECRET_PREFIX,
                "max_sidecars": parity::MAX_SIDECARS,
                "example": registry_body,
            },
        },
    });
    println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
}
