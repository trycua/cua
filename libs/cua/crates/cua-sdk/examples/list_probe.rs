//! What an app does to keep its Space list fresh, through `cua daemon`,
//! timed: read the list, then reach every Space on it at once, each with the
//! app's own deadline, poll after poll. For checking the list at scale on
//! real computers (some off, slow or unresponsive).
//!
//!     cargo run -p cua-sdk --example list_probe -- [options]
//!
//! Options (all optional):
//!   --polls N         list reads (default 6)
//!   --interval-ms MS  pause between reads (default 10000, the app's poll)
//!   --deadline-ms MS  the app's bound on one Space's connect (default 4000)
//!   --label TEXT      copied into every JSON line
//!
//! One JSON line per read on stdout: `{"poll","list_ms","spaces","answered",
//! "answered_max_ms","failed","failed_max_ms",...}`. Connects to the running
//! daemon; run it with the daemon's `CUA_HOME`.

use cua_sdk::{Cua, CuaConfig};
use std::time::{Duration, Instant};

fn arg(name: &str, default: u64) -> u64 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn label() -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--label")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let polls = arg("--polls", 6);
    let interval = Duration::from_millis(arg("--interval-ms", 10_000));
    let deadline = Duration::from_millis(arg("--deadline-ms", 4_000));
    let label = label();
    let cua = Cua::auto(CuaConfig::default()).expect("cua daemon");
    let spaces = cua.spaces();
    for poll in 0..polls {
        let t = Instant::now();
        let list = match spaces.list().await {
            Ok(l) => l,
            Err(e) => {
                println!("{}", serde_json::json!({"label": label, "poll": poll, "error": e.to_string()}));
                tokio::time::sleep(interval).await;
                continue;
            }
        };
        let list_ms = t.elapsed().as_millis();
        let start = Instant::now();
        let mut tasks = Vec::new();
        for info in list.iter().filter(|i| !["stopped", "suspended"].contains(&i.power_state.as_str())) {
            let spaces = spaces.clone();
            let id = info.id.clone();
            tasks.push(tokio::spawn(async move {
                let r = tokio::time::timeout(deadline, spaces.space(id.clone())).await;
                let ok = matches!(r, Ok(Ok(_)));
                let why = match r {
                    Ok(Ok(_)) => String::new(),
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => "deadline".into(),
                };
                (id, ok, start.elapsed().as_millis(), why)
            }));
        }
        let (mut ok_n, mut ok_max, mut bad_n, mut bad_max) = (0, 0, 0, 0);
        let mut reasons = std::collections::BTreeMap::<String, u32>::new();
        for task in tasks {
            let (_id, ok, ms, why) = task.await.unwrap();
            if ok {
                ok_n += 1;
                ok_max = ok_max.max(ms);
            } else {
                bad_n += 1;
                bad_max = bad_max.max(ms);
                let key: String = why.chars().take(60).collect();
                *reasons.entry(key).or_default() += 1;
            }
        }
        println!(
            "{}",
            serde_json::json!({"label": label, "poll": poll, "list_ms": list_ms, "spaces": list.len(),
                "answered": ok_n, "answered_max_ms": ok_max, "failed": bad_n, "failed_max_ms": bad_max,
                "reasons": reasons})
        );
        if poll + 1 < polls {
            tokio::time::sleep(interval).await;
        }
    }
}
