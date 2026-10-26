//! Time to a usable Space: create (or fork) through `cua daemon`, then the
//! first command and the first screenshot inside it. "Ready" is the end of
//! the screenshot, the first moment an agent has both a shell and its eyes.
//!
//!     cargo run -p cua-sdk --example space_ready -- [options]
//!
//! Options (all optional):
//!   --image REF        default ghcr.io/trycua/linux:24.04-slim
//!   --kind K           container | vm | microvm (default: the daemon's)
//!   --runtime R        gvisor | qemu | firecracker | containerization | ...
//!   --n N              samples (default 10)
//!   --mode M           create | fork | restore (default create)
//!   --source NAME      fork/restore source (a running Space or a checkpoint)
//!   --checkpoint NAME  before sampling, checkpoint `--source` as NAME and
//!                      time it (n=1 per run); restore/fork then use NAME
//!   --cpus N --memory-mb M
//!   --no-shot          skip the screenshot (images without a desktop)
//!   --keep             keep the Spaces (default: delete each after timing)
//!   --gap-ms MS        pause between samples (default 0; lets a pool refill)
//!   --label TEXT       copied into every JSON line
//!
//! One JSON line per sample on stdout:
//! `{"mode","i","create_ms","exec_ms","shot_ms","ready_ms","ok",...}` and a
//! summary line (`"summary":true`) with n, p50 and p95 of `ready_ms`.
//! Connects to the running daemon (`cua daemon start`); run it with the
//! daemon's `CUA_HOME`.

use cua_sdk::{Cua, SandboxCreateOptions};
use std::time::Instant;

struct Args {
    image: String,
    kind: Option<String>,
    runtime: Option<String>,
    n: usize,
    mode: String,
    source: Option<String>,
    checkpoint: Option<String>,
    cpus: Option<u32>,
    memory_mb: Option<u64>,
    shot: bool,
    keep: bool,
    gap_ms: u64,
    label: String,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        image: "ghcr.io/trycua/linux:24.04-slim".into(),
        kind: None,
        runtime: None,
        n: 10,
        mode: "create".into(),
        source: None,
        checkpoint: None,
        cpus: None,
        memory_mb: None,
        shot: true,
        keep: false,
        gap_ms: 0,
        label: String::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--image" => a.image = v()?,
            "--kind" => a.kind = Some(v()?),
            "--runtime" => a.runtime = Some(v()?),
            "--n" => a.n = v()?.parse().map_err(|e| format!("--n: {e}"))?,
            "--mode" => a.mode = v()?,
            "--source" => a.source = Some(v()?),
            "--checkpoint" => a.checkpoint = Some(v()?),
            "--cpus" => a.cpus = Some(v()?.parse().map_err(|e| format!("--cpus: {e}"))?),
            "--memory-mb" => {
                a.memory_mb = Some(v()?.parse().map_err(|e| format!("--memory-mb: {e}"))?)
            }
            "--gap-ms" => a.gap_ms = v()?.parse().map_err(|e| format!("--gap-ms: {e}"))?,
            "--label" => a.label = v()?,
            "--no-shot" => a.shot = false,
            "--keep" => a.keep = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if a.mode != "create" && a.source.is_none() && a.checkpoint.is_none() {
        return Err(format!("--mode {} needs --source or --checkpoint", a.mode));
    }
    Ok(a)
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    // Nearest rank.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = parse().map_err(|e| format!("space_ready: {e}"))?;
    let cua = Cua::connect(None, None)?;
    let sbs = cua.sandboxes();
    let tag = format!("{:06x}", std::process::id() & 0xff_ffff);

    let mut fork_source = a.source.clone();
    if let (Some(ck), Some(src)) = (&a.checkpoint, &a.source) {
        let t = Instant::now();
        sbs.checkpoint(src.clone(), ck.clone(), false).await?;
        println!(
            "{}",
            serde_json::json!({"mode": "checkpoint", "label": a.label, "source": src,
                "checkpoint": ck, "checkpoint_ms": ms(t), "ok": true})
        );
        fork_source = Some(ck.clone());
    } else if let Some(ck) = &a.checkpoint {
        fork_source = Some(ck.clone());
    }

    let mut ready = Vec::new();
    let mut failed = 0usize;
    for i in 0..a.n {
        if i > 0 && a.gap_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(a.gap_ms)).await;
        }
        let name = format!("rdy-{tag}-{i}");
        let t0 = Instant::now();
        let created = match a.mode.as_str() {
            "create" => {
                let mut o = SandboxCreateOptions::new("local", a.image.clone());
                o.name = Some(name.clone());
                o.kind = a.kind.clone();
                o.runtime = a.runtime.clone();
                o.cpus = a.cpus;
                o.memory_mb = a.memory_mb;
                sbs.create(o).await
            }
            _ => {
                sbs.fork(fork_source.clone().unwrap_or_default(), name.clone())
                    .await
            }
        };
        let create_ms = ms(t0);
        let sb = match created {
            Ok(sb) => sb,
            Err(e) => {
                failed += 1;
                println!(
                    "{}",
                    serde_json::json!({"mode": a.mode, "label": a.label, "i": i, "ok": false,
                        "stage": "create", "create_ms": create_ms, "error": e.to_string()})
                );
                let _ = sbs.delete(name.clone()).await;
                continue;
            }
        };
        let mut line = serde_json::json!({"mode": a.mode, "label": a.label, "i": i,
            "name": name, "create_ms": create_ms});
        let outcome: Result<(), String> = async {
            let t1 = Instant::now();
            let env = sb.spacesd(Some(30_000)).await.map_err(|e| e.to_string())?;
            let out = env
                .sh("true".into(), Some(30_000))
                .await
                .map_err(|e| e.to_string())?;
            if out.exit.code != Some(0) {
                return Err(format!("first command exited {:?}", out.exit));
            }
            line["exec_ms"] = ms(t1).into();
            if a.shot {
                let t2 = Instant::now();
                let shot = env.screenshot(None).await.map_err(|e| e.to_string())?;
                if shot.image.is_empty() || shot.width == 0 {
                    return Err("empty screenshot".into());
                }
                line["shot_ms"] = ms(t2).into();
                line["shot_px"] = format!("{}x{}", shot.width, shot.height).into();
            }
            Ok(())
        }
        .await;
        let ready_ms = ms(t0);
        match outcome {
            Ok(()) => {
                line["ready_ms"] = ready_ms.into();
                line["ok"] = true.into();
                ready.push(ready_ms);
            }
            Err(e) => {
                failed += 1;
                line["ok"] = false.into();
                line["error"] = e.into();
            }
        }
        println!("{line}");
        if !a.keep {
            let td = Instant::now();
            if let Err(e) = sbs.delete(name.clone()).await {
                eprintln!("delete {name}: {e}");
            }
            eprintln!(
                "deleted {name} in {:.0} ms",
                td.elapsed().as_secs_f64() * 1e3
            );
        }
    }
    ready.sort_by(|x, y| x.partial_cmp(y).unwrap());
    println!(
        "{}",
        serde_json::json!({"summary": true, "mode": a.mode, "label": a.label,
            "image": a.image, "kind": a.kind, "runtime": a.runtime,
            "n": ready.len(), "failed": failed,
            "p50_ms": pct(&ready, 50.0), "p95_ms": pct(&ready, 95.0),
            "min_ms": ready.first(), "max_ms": ready.last()})
    );
    Ok(())
}
