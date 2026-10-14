// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Language lanes: run an example in bench mode (examples/streaming
//! SCENARIO.md, "Benchmark JSONL") through `shims/<client>.sh` and turn its
//! per-frame JSONL into the same metrics as the native lane.

use std::collections::BTreeMap;
use std::io::BufRead as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::report::{put_dist, round3};
use crate::run::{children_cpu_s, Scenario};
use crate::space::Space;
use crate::video::unwrap_ms;

pub enum ShimOutcome {
    Ran(BTreeMap<String, f64>),
    /// The shim exited 3: prerequisites missing.
    Skipped(String),
}

const MAX_LINES: usize = 1_000_000;

pub async fn run_shim(
    shims: &Path,
    client: &str,
    space: &Space,
    scenario: Scenario,
    seconds: u64,
) -> Result<ShimOutcome, String> {
    let script = shims.join(format!("{client}.sh"));
    if !script.exists() {
        return Ok(ShimOutcome::Skipped(format!(
            "{} does not exist",
            script.display()
        )));
    }
    space
        .fixture(scenario.fixture_mode())
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let time_addr = space.time_addr;
    let (offset_ns, _) =
        tokio::task::spawn_blocking(move || crate::space::clock_offset(time_addr, 60))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    let space = space.clone();
    let shims = shims.to_path_buf();
    let client = client.to_owned();
    tokio::task::spawn_blocking(move || {
        wait_shim(&shims, &client, &space, scenario, seconds, offset_ns)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn wait_shim(
    shims: &Path,
    client: &str,
    space: &Space,
    scenario: Scenario,
    seconds: u64,
    offset_ns: i64,
) -> Result<ShimOutcome, String> {
    let script = shims.join(format!("{client}.sh"));
    let jsonl =
        std::env::temp_dir().join(format!("cua-bench-{client}-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&jsonl);
    let target = match scenario.target_title() {
        Some(title) => format!("window:{title}"),
        None => "display:primary".into(),
    };
    let log = std::env::temp_dir().join(format!("cua-bench-{client}-{}.log", std::process::id()));
    let out = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let cpu0 = children_cpu_s();
    let started = Instant::now();
    let mut child = Command::new("bash")
        .arg(&script)
        .env("CUA_ENV_URL", &space.url)
        .env("CUA_ENV_TOKEN", &space.token)
        .env("CUA_ENV_QUIC_ADDR", space.quic_addr.to_string())
        .env("CUA_BENCH_JSONL", &jsonl)
        .env("CUA_BENCH_TARGET", &target)
        .env("CUA_BENCH_SECONDS", seconds.to_string())
        .env("CUA_BENCH_AUDIO", "1")
        .env("CUA_HEADLESS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", script.display()))?;
    let limit = Duration::from_secs(seconds + 120);
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{client} shim timed out after {}s",
                    limit.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let wall = started.elapsed().as_secs_f64();
    let cpu = children_cpu_s() - cpu0;
    let stderr = std::fs::read(&log)
        .map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(64 * 1024)..]).into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    if status.code() == Some(3) {
        let reason = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("skipped")
            .trim()
            .to_owned();
        return Ok(ShimOutcome::Skipped(reason));
    }
    if !status.success() {
        let tail: String = stderr.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
        return Err(format!("{client} shim exited {status}: {tail}"));
    }
    let file = std::fs::File::open(&jsonl).map_err(|e| format!("no JSONL from {client}: {e}"))?;
    let mut open_ns = None;
    let mut frames: Vec<(i64, u64, Option<u64>)> = Vec::new(); // (unix_ns, bytes, tc)
    let mut audio_bytes = 0u64;
    let mut audio_packets = 0u64;
    let mut end: Option<serde_json::Value> = None;
    let mut keyframes = 0u64;
    for line in std::io::BufReader::new(file).lines().take(MAX_LINES) {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let t = value.get("t").and_then(|v| v.as_str()).unwrap_or_default();
        let ns = value.get("unix_ns").and_then(|v| v.as_i64()).unwrap_or(0);
        match t {
            "open" => open_ns = Some(ns),
            "frame" => {
                if value.get("key").and_then(|v| v.as_bool()) == Some(true) {
                    keyframes += 1;
                }
                frames.push((
                    ns,
                    value.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                    value.get("tc_ms").and_then(|v| v.as_u64()),
                ));
            }
            "audio" => {
                audio_packets += 1;
                audio_bytes += value.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
            }
            "end" => end = Some(value),
            _ => {}
        }
    }
    let _ = std::fs::remove_file(&jsonl);
    let mut m = BTreeMap::new();
    let Some(open_ns) = open_ns else {
        return Err(format!("{client}: no open record"));
    };
    let Some(&(first_ns, ..)) = frames.first() else {
        return Err(format!("{client}: no frames"));
    };
    let span = frames
        .last()
        .map(|f| (f.0 - open_ns) as f64 / 1e9)
        .unwrap_or(1.0)
        .max(seconds as f64 * 0.5);
    m.insert("ttff_ms".into(), round3((first_ns - open_ns) as f64 / 1e6));
    m.insert("frames_decoded".into(), frames.len() as f64);
    m.insert("keyframes".into(), keyframes as f64);
    m.insert("fps".into(), round3(frames.len() as f64 / span));
    let intervals: Vec<f64> = frames
        .windows(2)
        .map(|w| (w[1].0 - w[0].0) as f64 / 1e6)
        .collect();
    put_dist(&mut m, "interval_ms", &intervals);
    let mut g2g = Vec::new();
    for (ns, _, tc) in &frames {
        if let Some(tc) = tc {
            let guest_ms = (ns + offset_ns) / 1_000_000;
            // Shims may report the low 32 bits or the full value.
            let drawn = if *tc < (1 << 32) {
                unwrap_ms(*tc as u32, guest_ms)
            } else {
                *tc as i64
            };
            let latency = (guest_ms - drawn) as f64;
            if (-100.0..10_000.0).contains(&latency) {
                g2g.push(latency);
            }
        }
    }
    put_dist(&mut m, "g2g_ms", &g2g);
    let video_bytes: u64 = frames.iter().map(|f| f.1).sum();
    m.insert(
        "video_bytes_per_s".into(),
        round3(video_bytes as f64 / span),
    );
    m.insert(
        "audio_bytes_per_s".into(),
        round3(audio_bytes as f64 / span),
    );
    m.insert("audio_packets".into(), audio_packets as f64);
    // Whole shim process tree (runtime start-up included), and the shim's
    // own report of its streaming phase when it gives one.
    m.insert("client_cpu_pct".into(), round3(cpu / wall * 100.0));
    if let Some(end) = end {
        let user = end
            .get("cpu_user_s")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let sys = end.get("cpu_sys_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
        m.insert("client_self_cpu_s".into(), round3(user + sys));
    }
    m.insert("clock_offset_ms".into(), round3(offset_ns as f64 / 1e6));
    Ok(ShimOutcome::Ran(m))
}
