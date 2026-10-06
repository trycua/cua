//! Temporary qualification host; identical bytes for each isolated candidate.
use cua_driver_sdk::CuaDriver;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
async fn run(driver: &CuaDriver) -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str().unwrap_or("") {
            "initialize" => {
                let metadata = driver.metadata().await?;
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"ax-batch-qualification","version":metadata.driver_version},"_meta":{"driver_metadata":metadata}})
            }
            "tools/list" => serde_json::from_str::<Value>(&driver.list_tools_json().await?)?,
            "tools/call" => {
                let receipt = driver
                    .call_tool(
                        request["params"]["name"]
                            .as_str()
                            .ok_or("missing tool")?
                            .into(),
                        request["params"]["arguments"].to_string(),
                    )
                    .await?;
                serde_json::from_str(&receipt.raw_json)?
            }
            "ping" => json!({}),
            _ => json!({"error":"unsupported method"}),
        };
        println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
        io::stdout().flush()?;
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let driver = CuaDriver::create(None)?;
    let outcome = run(&driver).await;
    let shutdown = driver.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}
