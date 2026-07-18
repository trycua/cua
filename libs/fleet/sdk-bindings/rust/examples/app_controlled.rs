use cyclops_sdk_host::{
    Claim, Configuration, CreateClaimRequest, CreatePoolRequest, OAuthConfiguration, Pool,
    PoolSpec, PoolSpecService, PoolSpecServiceProtocol, PoolTemplate, Sandbox, Sdk,
};
use std::{
    env, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn run_agent(sdk: &mut Sdk, sandbox: Sandbox) -> Result<serde_json::Value, String> {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cyclops-sdk-rust-example","version":"0.1.0"}}}"#;
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut mcp = sdk
        .service_client(&sandbox, "mcp")
        .map_err(|error| error.to_string())?;
    loop {
        let response = mcp
            .post("/mcp")
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body(body)
            .send()
            .map_err(|error| error.to_string())?;
        if (200..300).contains(&response.status) {
            return Ok(
                serde_json::json!({"namespace": sandbox.namespace, "claim": sandbox.claim, "sandbox": sandbox.sandbox, "mcp_status": response.status}),
            );
        }
        if !matches!(response.status, 502..=504) || Instant::now() >= deadline {
            return Err(format!(
                "MCP initialize failed with HTTP {}: {}",
                response.status, response.body
            ));
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn main() -> Result<(), String> {
    let suffix = format!(
        "{:08x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos() as u32
    );
    let namespace =
        env::var("CYCLOPS_NAMESPACE").unwrap_or_else(|_| format!("sdk-example-{suffix}"));
    let mut sdk = Sdk::connect(Configuration {
        base_url: env::var("CUA_BASE_URL").unwrap_or_else(|_| "https://run.cua.ai".into()),
        oauth: OAuthConfiguration {
            token_url: env::var("CUA_TOKEN_URL").unwrap_or_else(|_| {
                "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token".into()
            }),
            client_id: required("CUA_CLIENT_ID")?,
            client_secret: required("CUA_CLIENT_SECRET")?,
        },
    })?;
    let mut pool: Option<Pool> = None;
    let mut claim: Option<Claim> = None;
    let result = (|| {
        let created_pool = sdk.create_pool(CreatePoolRequest {
            namespace: namespace.clone(),
            spec: PoolSpec::builder()
                .replicas(1)
                .services(vec![PoolSpecService::builder()
                    .name("mcp".into())
                    .target_port(3000)
                    .protocol(PoolSpecServiceProtocol::TCP)
                    .build()?])
                .template(
                    PoolTemplate::builder()
                        .container_disk_image(required("CUA_IMAGE")?)
                        .image_pull_secret(required("CUA_IMAGE_PULL_SECRET")?)
                        .cpu_cores(4)
                        .memory("4Gi".into())
                        .build()?,
                )
                .build()?,
        })?;
        pool = Some(created_pool.clone());
        let created_claim = sdk.create_claim(CreateClaimRequest {
            pool: created_pool,
            spec: None,
        })?;
        claim = Some(created_claim.clone());
        let sandbox = sdk.wait_claim(created_claim)?;
        let result = run_agent(&mut sdk, sandbox)?;
        println!("{result}");
        Ok(())
    })();
    let claim_cleanup = claim.map_or(Ok(()), |claim| sdk.delete_claim(claim));
    let pool_cleanup = pool.map_or(Ok(()), |pool| sdk.delete_pool(pool));
    sdk.close();
    result.and(claim_cleanup).and(pool_cleanup)
}

fn required(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} is required"))
}
