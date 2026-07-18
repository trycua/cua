use cyclops_sdk_host::{
    Claim, Configuration, CreateClaimRequest, CreatePoolRequest, Pool, Sandbox, Sdk, SdkError,
    ServiceHttpResponse,
};

#[test]
fn typed_resource_api_only() {
    let _: fn(Configuration) -> Result<Sdk, String> = Sdk::connect;
    let _: fn(&mut Sdk, CreatePoolRequest) -> Result<Pool, String> = Sdk::create_pool;
    let _: fn(&mut Sdk, &str) -> Result<Vec<Pool>, String> = Sdk::list_pools;
    let _: fn(&mut Sdk, Pool) -> Result<Pool, String> = Sdk::get_pool;
    let _: fn(&mut Sdk, Pool) -> Result<Pool, String> = Sdk::update_pool;
    let _: fn(&mut Sdk, Pool) -> Result<(), String> = Sdk::delete_pool;
    let _: fn(&mut Sdk, CreateClaimRequest) -> Result<Claim, String> = Sdk::create_claim;
    let _: fn(&mut Sdk, &str) -> Result<Vec<Claim>, String> = Sdk::list_claims;
    let _: fn(&mut Sdk, Claim) -> Result<Claim, String> = Sdk::get_claim;
    let _: fn(&mut Sdk, Claim) -> Result<Claim, String> = Sdk::update_claim;
    let _: fn(&mut Sdk, Claim) -> Result<(), String> = Sdk::delete_claim;
    let _: fn(&mut Sdk, Claim) -> Result<cyclops_sdk_host::Sandbox, String> = Sdk::wait_claim;
}

fn service_http_contract(sdk: &mut Sdk, sandbox: &Sandbox) -> Result<(), SdkError> {
    let mut service = sdk.service_client(sandbox, "mcp")?;
    let _: ServiceHttpResponse = service
        .post("/mcp")
        .header("content-type", "application/json")
        .body("{}")
        .send()?;
    Ok(())
}

#[test]
fn service_http_is_sdk_owned() {
    let _: fn(&mut Sdk, &Sandbox) -> Result<(), SdkError> = service_http_contract;
}
