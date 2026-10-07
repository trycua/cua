#[cfg(target_os = "macos")]
mod native {
    //! Native experiment transport for RFC #4771; no private focus restore hook.
    use cua_driver_sdk::CuaDriver;
    use serde_json::{json, Value};
    use std::{
        io::{self, BufRead, Write},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(
            mode: *const std::ffi::c_void,
            seconds: f64,
            return_after: bool,
        ) -> i32;
    }
    async fn serve() -> Result<(), Box<dyn std::error::Error>> {
        let driver = CuaDriver::create(None)?;
        let mut receipts = Vec::<(Value, Value)>::new();
        for line in io::stdin().lock().lines() {
            let req: Value = serde_json::from_str(&line?)?;
            let Some(id) = req.get("id") else { continue };
            let result = match req["method"].as_str().unwrap_or("") {
                "initialize" => {
                    let m = driver.metadata().await?;
                    json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"owned-supervision-host","version":m.driver_version},"_meta":{"driver_metadata":m}})
                }
                "tools/list" => serde_json::from_str::<Value>(&driver.list_tools_json().await?)?,
                "tools/call" => {
                    let r = driver
                        .call_tool(
                            req["params"]["name"].as_str().ok_or("name")?.into(),
                            req["params"]["arguments"].to_string(),
                        )
                        .await?;
                    serde_json::from_str(&r.raw_json)?
                }
                "research/restore_target" => {
                    let pid = req["params"]["pid"].as_i64().ok_or("pid")? as i32;
                    let w = req["params"]["window_id"].as_u64().ok_or("window")? as u32;
                    json!({"registered":platform_macos::windows::window_info_by_id(w).is_some_and(|window|window.pid==pid),"cocoa_front_matches":platform_macos::apps::frontmost_pid()==Some(pid),"native_front_matches":platform_macos::input::skylight::front_process_matches(pid,w)})
                }
                "research/start" => {
                    let args = req["params"]["arguments"].clone();
                    let r = driver
                        .call_tool("dispatch_set_value".into(), args.to_string())
                        .await?;
                    let result: Value = serde_json::from_str(&r.raw_json)?;
                    if result["isError"] == true {
                        return Err(format!("dispatch refused: {result}").into());
                    }
                    let receipt = result["structuredContent"]["receipt_id"].clone();
                    if receipt.is_null() {
                        return Err("missing owned receipt".into());
                    }
                    receipts.push((receipt.clone(), args["session"].clone()));
                    json!({"receipt":receipt,"status":if result["structuredContent"]["input_disposition"]=="attempted" {"dispatched"}else{"uncertain"},"completion_claim":false})
                }
                "research/fence" => {
                    let mut outcomes = serde_json::Map::new();
                    for (receipt, session) in &receipts {
                        let r = driver
                            .call_tool(
                                "fence_action_supervision".into(),
                                json!({"session":session,"receipt_id":receipt,"timeout_ms":3000})
                                    .to_string(),
                            )
                            .await?;
                        let v: Value = serde_json::from_str(&r.raw_json)?;
                        if v["isError"] == true
                            || v["structuredContent"]["supervision"]["state"] != "finished"
                        {
                            return Err(format!("supervision uncertain: {v}").into());
                        }
                        outcomes.insert(receipt.as_str().ok_or("receipt")?.to_owned(), v);
                    }
                    json!({"receipts":outcomes})
                }
                _ => json!({"error":"unsupported"}),
            };
            println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
            io::stdout().flush()?;
        }
        // The SDK runtime drain owns outstanding native observers after EOF.
        driver.shutdown().await?;
        Ok(())
    }
    pub fn run() {
        let _observer = platform_macos::focus_steal::FocusStealPreventer::shared();
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let worker = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            if let Err(e) = rt.block_on(serve()) {
                eprintln!("Owned experiment host: {e}");
            }
            flag.store(true, Ordering::SeqCst);
        });
        while !done.load(Ordering::SeqCst) {
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
            }
        }
        worker.join().unwrap();
    }
}
#[cfg(target_os = "macos")]
fn main() {
    native::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("owned native supervision is unsupported on this platform");
    std::process::exit(1);
}
