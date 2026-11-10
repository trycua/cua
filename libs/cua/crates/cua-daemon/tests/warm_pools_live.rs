//! Opt-in: the `warm-pools` experiment end to end on this computer's
//! container engine, measured the way Home uses it. A person asks for one
//! thing, then another a few seconds later, and so on: each ask is a create
//! of the Linux Space Home makes for an agent, then the coding app starts in
//! it. The test makes N such creates in a row, `CUA_WARM_LIVE_GAP_MS` apart,
//! and writes one JSON line for each:
//!
//! - `create_ms`: from the create call to a Space that answers with its own
//!   token (the claim, when a warm Space was ready);
//! - `apps_ms`: the agent stack check at the start of the agent (a no-op
//!   when the coding app is in the Space already);
//! - `agent_ms`: from the start of the coding app to an open session that
//!   accepts a message (no model is asked anything);
//! - `total_ms`: the three together, from the create call to "the agent is
//!   ready for the first message";
//! - `warm_ready`: warm Spaces the daemon held when the create began.
//!
//! `CUA_WARM_LIVE_MODE=warm` (default) turns the experiment on and waits for
//! the prepared warm Space first; `cold` leaves it off, so every create
//! starts a Space and installs the coding app, as without the experiment.
//!
//! ```text
//! CUA_HOME=$HOME/projects/.cua-work/warmpool/home CUA_WARM_LIVE=1 \
//!   CUA_WARM_LIVE_OUT=/some/dir/warm.jsonl \
//!   cargo test -p cua-daemon --test warm_pools_live -- --ignored --nocapture
//! ```
//!
//! Needs a throwaway `CUA_HOME` (never `~/.cua`). Image
//! `CUA_WARM_LIVE_IMAGE` (default: what Home creates), app
//! `CUA_WARM_LIVE_HARNESS` (default `claude-code`), count `CUA_WARM_LIVE_N`
//! (10), gap `CUA_WARM_LIVE_GAP_MS` (3000). Everything made is deleted on
//! every path. Run it under the no-UI sandbox profile on a desktop.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_agents::{Agents, RunStatus, runs::RunOptions};
use cua_daemon::{CreateRequest, Runtime, RuntimeConfig, local::VmmLocal};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};

async fn client(url: &str, token: &str) -> SpacesdClient {
    let o = ConnectOptions::parse(url)
        .unwrap()
        .token(token)
        .transport(TransportPreference::Native)
        .probe(false);
    SpacesdClient::connect(o).await.unwrap()
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Warm Spaces ready now and whether one of them has its apps installed.
fn pool_records(state: &std::path::Path) -> (usize, usize) {
    let mut all = 0;
    let mut prepared = 0;
    if let Ok(rd) = std::fs::read_dir(state.join(".pool")) {
        for e in rd.flatten() {
            if let Some(v) = std::fs::read(e.path())
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                && v.get("secret").is_some()
            {
                all += 1;
                if v["prepared"].as_bool() == Some(true) {
                    prepared += 1;
                }
            }
        }
    }
    (all, prepared)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs Spaces on this computer's container engine; set CUA_WARM_LIVE=1 and a throwaway CUA_HOME"]
async fn sequential_creates_start_the_agent_from_a_warm_home() {
    if std::env::var("CUA_WARM_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_WARM_LIVE=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .try_init();
    }
    let warm = std::env::var("CUA_WARM_LIVE_MODE").as_deref() != Ok("cold");
    let image = std::env::var("CUA_WARM_LIVE_IMAGE")
        .unwrap_or_else(|_| cua_daemon::local::warm_pools_image());
    // The apps measured, one after the other against the same prepared
    // Space (comma separated; default: the four most used).
    let harnesses: Vec<String> = std::env::var("CUA_WARM_LIVE_HARNESS")
        .unwrap_or_else(|_| "openai-codex,hermes,claude-code,opencode".into())
        .split(',')
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .collect();
    let n = env_u64("CUA_WARM_LIVE_N", 10) as usize;
    let gap = Duration::from_millis(env_u64("CUA_WARM_LIVE_GAP_MS", 3000));
    let out_path = std::env::var_os("CUA_WARM_LIVE_OUT").map(std::path::PathBuf::from);
    // The operator override that turns Warm start off, for this process only (a cold run).
    if !warm {
        // SAFETY: set before the runtime or any thread reads it.
        unsafe { std::env::set_var(cua_daemon::local::WARM_START_ENV, "0") };
    }
    // The apps the warm Space is prepared for: the daemon's default, or
    // `CUA_WARM_POOLS_HARNESSES` as given.
    let state = home.join("sandboxes");
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(state.clone()),
        vmm: Some(vmm.clone()),
        ..Default::default()
    })
    .unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    let pool_task = vmm
        .enable_pool(cua_sandbox_core::StateStore::new(state.clone()), rx)
        .expect("pool enabled once");
    let tag = format!("{:06x}", std::process::id() & 0xff_ffff);
    let names: Vec<String> = harnesses
        .iter()
        .flat_map(|h| {
            let (tag, h) = (tag.clone(), h.replace(['-', '_'], ""));
            (0..n).map(move |i| format!("wp-{tag}-{h}-{i}"))
        })
        .collect();
    let request = |name: &str| CreateRequest {
        location: Some("local".into()),
        name: Some(name.into()),
        image: image.clone(),
        ..Default::default()
    };
    let mut lines: Vec<serde_json::Value> = vec![];
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        if warm {
            // The first warm Space is made and prepared (the image is
            // pulled and the app installed once); creates start after it.
            let began = Instant::now();
            let deadline = Instant::now() + Duration::from_secs(env_u64("CUA_WARM_LIVE_WAIT_S", 2400));
            loop {
                let ready = vmm.pool().map(|p| p.status().members).unwrap_or(0);
                if ready >= 1 && pool_records(&state).1 >= 1 {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "no prepared warm Space within the wait: {:?}",
                    vmm.pool().map(|p| p.status())
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            eprintln!(
                "prepared warm Space ready after {:.0}s",
                began.elapsed().as_secs_f64()
            );
        } else {
            // Without the experiment the image is pulled once, outside the
            // timing, so the numbers are starts and not downloads.
            // (`CUA_WARM_LIVE_NO_PULL=1` keeps it in: the first create of a
            // fresh install, with the runtime and the image still to fetch.)
            if std::env::var_os("CUA_WARM_LIVE_NO_PULL").is_none() {
                let warmup = format!("wp-{tag}-pull");
                rt.create(request(&warmup)).await.unwrap();
                rt.delete(&warmup).await.unwrap();
            }
        }
        for (k, name) in names.iter().enumerate() {
            let (harness, i) = (&harnesses[k / n], k % n);
            if i > 0 {
                tokio::time::sleep(gap).await;
            }
            let warm_ready = vmm.pool().map(|p| p.status().members).unwrap_or(0);
            let t0 = Instant::now();
            let made = rt.create(request(name)).await.unwrap();
            let create_ms = t0.elapsed().as_millis() as u64;
            let sb = rt.handle(&made.id).await.unwrap();
            let token = sb.env_token().expect("a token");
            let c = client(&made.endpoints["env"], &token).await;
            let agents = Agents::new(c).await.unwrap();
            // What every start of the app does first: the stack check.
            let h = cua_agents::harness::harness(harness).expect("a harness");
            let t1 = Instant::now();
            agents.prewarm(h, true).await.unwrap();
            agents.ensure(h.installs, |_| {}).await.unwrap();
            let apps_ms = t1.elapsed().as_millis() as u64;
            let t2 = Instant::now();
            // A credential no model ever sees: no turn is taken.
            let env = h.placeholder_env();
            let started = agents
                .start(
                    harness,
                    "",
                    RunOptions {
                        env,
                        install: Some(true),
                        skills: Some(false),
                        idle_exit_secs: Some(900),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let mut ready_at = None;
            let mut failed: Option<String> = None;
            let deadline = Instant::now() + Duration::from_secs(900);
            while Instant::now() < deadline {
                if let Ok(st) = agents.status(&started.run_id).await {
                    if st.status == RunStatus::Idle && st.accepts_message {
                        ready_at = Some(t2.elapsed().as_millis() as u64);
                        break;
                    }
                    if st.status == RunStatus::Failed {
                        failed = Some(format!("{} {:?}", st.reason, st.error));
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if let Some(why) = failed {
                let line = serde_json::json!({
                    "harness": harness, "i": i, "error": why.chars().take(300).collect::<String>(),
                    "create_ms": create_ms, "mode": if warm { "warm" } else { "cold" },
                });
                eprintln!("{line}");
                lines.push(line);
                let _ = agents.stop(&started.run_id).await;
                let _ = rt.delete(name).await;
                continue;
            }
            let agent_ms = ready_at.expect("the agent session opened within 15 minutes");
            let line = serde_json::json!({
                "harness": harness, "i": i, "mode": if warm { "warm" } else { "cold" }, "gap_ms": gap.as_millis() as u64,
                "warm_ready": warm_ready, "create_ms": create_ms, "apps_ms": apps_ms,
                "agent_ms": agent_ms, "total_ms": t0.elapsed().as_millis() as u64,
            });
            eprintln!("{line}");
            lines.push(line);
            if std::env::var_os("CUA_WARM_LIVE_PROFILE").is_some() {
                // The runner's own steps, in the guest's clock: where the start went.
                if let Ok(page) = agents.events(&started.run_id, 0, 500).await {
                    let first = page.events.first().map(|e| e.ts_ms).unwrap_or(0);
                    let steps: Vec<String> = page
                        .events
                        .iter()
                        .map(|e| {
                            format!(
                                "{}+{}",
                                e.raw.get("type").and_then(|v| v.as_str()).unwrap_or(e.kind),
                                e.ts_ms.saturating_sub(first)
                            )
                        })
                        .collect();
                    eprintln!("profile {i}: {}", steps.join(" "));
                }
            }
            let _ = agents.stop(&started.run_id).await;
            if let Some(hold) = std::env::var("CUA_WARM_LIVE_HOLD_S").ok().and_then(|v| v.parse::<u64>().ok())
                && k + 1 == names.len()
            {
                // Left running for a person to look at (`docker exec`).
                eprintln!("holding {name} for {hold} s");
                tokio::time::sleep(Duration::from_secs(hold)).await;
            }
            let _ = rt.delete(name).await;
        }
        for name in &names {
            let _ = rt.delete(name).await;
        }
    }))
    .await;
    let _ = stop.send(true);
    let _ = pool_task.await;
    if let Some(p) = vmm.pool() {
        p.drain().await;
    }
    for name in &names {
        let _ = rt.delete(name).await;
    }
    if let Some(path) = out_path {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(path, text).unwrap();
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    let left = vmm.container().await.unwrap().pool_members().await.unwrap();
    assert!(left.is_empty(), "members left behind: {left:?}");
    use cua_sandbox_core::LocalRuntime as _;
    let listed = vmm.list().await.unwrap();
    assert!(
        listed.iter().all(|s| !names.contains(&s.name)),
        "Spaces left behind"
    );
}
