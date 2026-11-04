//! RL-rollout benchmark through the SDK (the path a trainer takes): sandbox
//! start, the trainer-to-sandbox channel, checkpoint/fork and a
//! rollout-shaped step loop. Connects to the running `cua daemon`; exec and
//! files then go straight to each sandbox's cua-spacesd.
//!
//!     cargo run --release -p cua-sdk --example rollout_bench -- <cmd> [options]
//!
//! Commands (one JSON line per sample on stdout, a `"summary":true` line last):
//!   create     --prefix P --n N [--conc C] [--kind K] [--runtime R] [--image I]
//!              [--cpus N] [--memory-mb M] [--keep] [--headless] [--profile P] [--wait-tcp PORT]
//!              (--headless: `SandboxCreateOptions.headless`, also for `pool`)
//!              time to the first successful exec (create + attach + `true`)
//!   fork       --source S --prefix P --n N [--conc C] [--keep]
//!   fork-many  --source S --prefix P --n N [--conc C (per host)] [--on O]
//!              one Sandboxes.fork_many batch; per fork: ready ms and first exec
//!              (create --forkable makes the source a fork source; checkpoint --pin)
//!   checkpoint --source S --name CK
//!   channel    --name S [--iters K]   exec RTT, upload/download 1 MB and
//!              100 MB, streaming stdout throughput
//!   probe      --prefix P [--iters K] exec RTT on every running sandbox
//!              whose name starts with P (concurrently, one in flight each)
//!   rollout    --prefix P --steps S [--payload-bytes B] [--n N]   every
//!              sandbox (or the first N) runs S steps of: exec a short
//!              command, write a file, read the reward
//!   pool       --name POOL --n SIZE --prefix P [--kind K] [--runtime R]
//!              fills a warm pool of SIZE, then times SIZE/2 serial claims
//!              and a batch of SIZE/2 claims (create from the pool to first exec)
//!   delete     --prefix P [--conc C]

use cua_sdk::{
    Cua, PoolCreateOptions, Sandbox, SandboxCreateOptions, SpacesdClient, SpacesdCommand,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default, Clone)]
struct Args {
    cmd: String,
    prefix: String,
    name: String,
    source: String,
    image: String,
    kind: Option<String>,
    runtime: Option<String>,
    cpus: Option<u32>,
    memory_mb: Option<u64>,
    n: usize,
    conc: usize,
    iters: usize,
    steps: usize,
    payload: usize,
    keep: bool,
    headless: bool,
    profile: Option<String>,
    wait_tcp: Option<u16>,
    forkable: bool,
    pin: bool,
    on: Option<String>,
    label: String,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        cmd: it.next().ok_or("missing command")?,
        image: "ghcr.io/trycua/linux:24.04-slim".into(),
        n: 1,
        conc: 1,
        iters: 200,
        steps: 50,
        payload: 4096,
        ..Default::default()
    };
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--prefix" => a.prefix = v()?,
            "--name" => a.name = v()?,
            "--source" => a.source = v()?,
            "--image" => a.image = v()?,
            "--kind" => a.kind = Some(v()?),
            "--runtime" => a.runtime = Some(v()?),
            "--cpus" => a.cpus = Some(v()?.parse().map_err(|e| format!("{e}"))?),
            "--memory-mb" => a.memory_mb = Some(v()?.parse().map_err(|e| format!("{e}"))?),
            "--n" => a.n = v()?.parse().map_err(|e| format!("{e}"))?,
            "--conc" => a.conc = v()?.parse().map_err(|e| format!("{e}"))?,
            "--iters" => a.iters = v()?.parse().map_err(|e| format!("{e}"))?,
            "--steps" => a.steps = v()?.parse().map_err(|e| format!("{e}"))?,
            "--payload-bytes" => a.payload = v()?.parse().map_err(|e| format!("{e}"))?,
            "--label" => a.label = v()?,
            "--keep" => a.keep = true,
            "--forkable" => a.forkable = true,
            "--pin" => a.pin = true,
            "--on" => a.on = Some(v()?),
            "--headless" => a.headless = true,
            "--profile" => a.profile = Some(v()?),
            // A local disk image's cua-spacesd is not known to the daemon:
            // probing its port makes the VM ready (and warm-startable) there.
            "--wait-tcp" => a.wait_tcp = Some(v()?.parse().map_err(|e| format!("{e}"))?),
            o => return Err(format!("unknown option {o}")),
        }
    }
    Ok(a)
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e4).round() / 10.0
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = ((p / 100.0) * s.len() as f64).ceil().max(1.0) as usize;
    s[rank.min(s.len()) - 1]
}

fn stats(label: &str, v: &[f64]) -> serde_json::Value {
    serde_json::json!({"metric": label, "n": v.len(), "p50": pct(v, 50.0), "p95": pct(v, 95.0),
        "p99": pct(v, 99.0), "max": pct(v, 100.0),
        "mean": if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }})
}

fn cmd(program: &str, args: &[&str]) -> SpacesdCommand {
    SpacesdCommand {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: HashMap::new(),
        cwd: None,
        user: None,
        timeout_ms: Some(120_000),
        tag: None,
        stdin: false,
        pty: None,
        request_id: None,
        epoch: None,
        scrollback_bytes: None,
        kill_on_disconnect: false,
    }
}

async fn names_with(
    cua: &Arc<Cua>,
    prefix: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut v: Vec<String> = cua
        .sandboxes()
        .list(Some("local".into()))
        .await?
        .into_iter()
        .map(|s| s.name)
        .filter(|n| n.starts_with(prefix))
        .collect();
    v.sort();
    Ok(v)
}

async fn attach(cua: &Arc<Cua>, name: &str) -> Result<(Arc<Sandbox>, Arc<SpacesdClient>), String> {
    let sb = cua
        .sandboxes()
        .connect(name.into())
        .await
        .map_err(|e| e.to_string())?;
    let env = sb.spacesd(Some(30_000)).await.map_err(|e| e.to_string())?;
    Ok((sb, env))
}

/// Starts N sandboxes (create or fork) with at most `conc` in flight; each
/// sample is time to the first successful exec.
async fn start_many(cua: Arc<Cua>, a: Args, fork: bool) -> Result<(), Box<dyn std::error::Error>> {
    let sem = Arc::new(tokio::sync::Semaphore::new(a.conc.max(1)));
    let wall = Instant::now();
    let mut tasks = Vec::new();
    for i in 0..a.n {
        let (cua, a, sem) = (cua.clone(), a.clone(), sem.clone());
        tasks.push(tokio::spawn(async move {
            let _p = sem.acquire_owned().await.unwrap();
            let name = format!("{}{}", a.prefix, i);
            let sbs = cua.sandboxes();
            let t0 = Instant::now();
            let created = if fork {
                sbs.fork(a.source.clone(), name.clone()).await
            } else {
                let mut o = SandboxCreateOptions::new("local", a.image.clone());
                o.name = Some(name.clone());
                o.kind = a.kind.clone();
                o.runtime = a.runtime.clone();
                o.cpus = a.cpus;
                o.memory_mb = a.memory_mb;
                o.headless = a.headless;
                o.profile = a.profile.clone();
                if a.forkable || std::env::var("FORKABLE").is_ok_and(|v| v == "1") {
                    o.forkable = Some(true);
                }
                if let Some(port) = a.wait_tcp {
                    o.ports.push(port);
                    o.wait_for.push(cua_sdk::ReadinessProbe {
                        port,
                        http_path: None,
                        http_status: None,
                        service: None,
                    });
                }
                sbs.create(o).await
            };
            let create_ms = ms(t0);
            let mut line = serde_json::json!({"cmd": if fork {"fork"} else {"create"}, "label": a.label,
                "i": i, "name": name, "create_ms": create_ms, "since_wall_start_ms": ms(wall)});
            let r: Result<f64, String> = async {
                let sb = created.map_err(|e| e.to_string())?;
                let t1 = Instant::now();
                let env = sb.spacesd(Some(60_000)).await.map_err(|e| e.to_string())?;
                let out = env.sh("true".into(), Some(60_000)).await.map_err(|e| e.to_string())?;
                if out.exit.code != Some(0) {
                    return Err(format!("first exec exited {:?}", out.exit));
                }
                line["first_exec_ms"] = ms(t1).into();
                Ok(ms(t0))
            }
            .await;
            match &r {
                Ok(t) => {
                    line["ok"] = true.into();
                    line["ready_ms"] = (*t).into();
                }
                Err(e) => {
                    line["ok"] = false.into();
                    line["error"] = e.clone().into();
                }
            }
            line["done_at_ms"] = ms(wall).into();
            println!("{line}");
            if !a.keep {
                let _ = sbs.delete(name).await;
            }
            r.ok()
        }));
    }
    let mut ready = Vec::new();
    let mut failed = 0;
    for t in tasks {
        match t.await? {
            Some(v) => ready.push(v),
            None => failed += 1,
        }
    }
    let mut s = stats("ready_ms", &ready);
    s["summary"] = true.into();
    s["cmd"] = (if fork { "fork" } else { "create" }).into();
    s["label"] = a.label.into();
    s["failed"] = failed.into();
    s["n_req"] = a.n.into();
    s["conc"] = a.conc.into();
    s["wall_ms"] = ms(wall).into();
    println!("{s}");
    Ok(())
}

async fn channel(cua: Arc<Cua>, a: Args) -> Result<(), Box<dyn std::error::Error>> {
    let (_sb, env) = attach(&cua, &a.name).await?;
    let mut out = Vec::new();
    // Exec RTT: a shell line, and a program with no shell.
    for (label, use_sh) in [("exec_sh_echo_ms", true), ("exec_run_echo_ms", false)] {
        for _ in 0..5 {
            let _ = env.sh("echo hi".into(), None).await?;
        }
        let mut v = Vec::new();
        for _ in 0..a.iters {
            let t = Instant::now();
            let o = if use_sh {
                env.sh("echo hi".into(), None).await?
            } else {
                env.run(cmd("/bin/echo", &["hi"])).await?
            };
            v.push(ms(t));
            assert_eq!(o.exit.code, Some(0));
        }
        let s = stats(label, &v);
        println!("{s}");
        out.push(s);
    }
    // Files.
    for (size, reps) in [(1usize << 20, 20usize), (100usize << 20, 3usize)] {
        let data: Vec<u8> = (0..size)
            .map(|i| (i as u32).wrapping_mul(2654435761) as u8)
            .collect();
        let (mut up, mut down) = (Vec::new(), Vec::new());
        for r in 0..reps {
            let p = format!("/tmp/bench-{size}-{r}.bin");
            let t = Instant::now();
            env.upload(p.clone(), data.clone(), None).await?;
            up.push(ms(t));
            let t = Instant::now();
            let got = env.download(p.clone(), None).await?;
            down.push(ms(t));
            assert_eq!(got.len(), size);
            let _ = env.remove(p, false, None).await;
        }
        let mb = size as f64 / (1 << 20) as f64;
        for (dir, v) in [("upload", &up), ("download", &down)] {
            let mut s = stats(&format!("{dir}_{}MB_ms", mb as u64), v);
            s["MBps_p50"] = (mb / (pct(v, 50.0) / 1000.0)).into();
            println!("{s}");
            out.push(s);
        }
    }
    // Streaming stdout.
    for bytes in [64u64 << 20, 512u64 << 20] {
        let p = env
            .spawn(cmd("/bin/sh", &["-c", &format!("yes 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde | head -c {bytes}")]))
            .await?;
        let t = Instant::now();
        let mut got = 0u64;
        let mut first = None;
        while let Some(ev) = p.next_event().await? {
            if first.is_none() && !ev.data.is_empty() {
                first = Some(ms(t));
            }
            got += ev.data.len() as u64;
        }
        let secs = t.elapsed().as_secs_f64();
        let s = serde_json::json!({"metric": "stream_stdout", "bytes": got, "secs": secs,
            "MBps": got as f64 / (1u64 << 20) as f64 / secs, "first_byte_ms": first});
        println!("{s}");
        out.push(s);
    }
    println!(
        "{}",
        serde_json::json!({"summary": true, "cmd": "channel", "label": a.label, "name": a.name, "results": out})
    );
    Ok(())
}

async fn probe(cua: Arc<Cua>, a: Args) -> Result<(), Box<dyn std::error::Error>> {
    let names = names_with(&cua, &a.prefix).await?;
    // --conc 0 (the default 1 means serial): every sandbox at once.
    let sem = Arc::new(tokio::sync::Semaphore::new(if a.conc == 0 {
        names.len().max(1)
    } else {
        a.conc
    }));
    let mut tasks = Vec::new();
    for n in names.clone() {
        let (cua, iters, sem) = (cua.clone(), a.iters, sem.clone());
        tasks.push(tokio::spawn(async move {
            let _p = sem.acquire_owned().await.unwrap();
            let (_sb, env) = match attach(&cua, &n).await {
                Ok(x) => x,
                Err(_) => return (Vec::new(), 1usize),
            };
            let mut v = Vec::new();
            let mut err = 0;
            for _ in 0..iters {
                let t = Instant::now();
                match env.sh("echo hi".into(), Some(30_000)).await {
                    Ok(o) if o.exit.code == Some(0) => v.push(ms(t)),
                    _ => err += 1,
                }
            }
            (v, err)
        }));
    }
    let (mut all, mut errs) = (Vec::new(), 0);
    for t in tasks {
        let (v, e) = t.await?;
        all.extend(v);
        errs += e;
    }
    let mut s = stats("exec_rtt_ms", &all);
    s["summary"] = true.into();
    s["cmd"] = "probe".into();
    s["label"] = a.label.into();
    s["sandboxes"] = names.len().into();
    s["errors"] = errs.into();
    println!("{s}");
    Ok(())
}

async fn rollout(cua: Arc<Cua>, a: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut names = names_with(&cua, &a.prefix).await?;
    if a.n > 1 {
        names.truncate(a.n);
    }
    // Attach first so the loop measures steady state.
    let mut envs = Vec::new();
    for n in &names {
        match attach(&cua, n).await {
            Ok((_, e)) => envs.push(e),
            Err(e) => eprintln!("attach {n}: {e}"),
        }
    }
    let payload: Vec<u8> = vec![b'a'; a.payload];
    let wall = Instant::now();
    let mut tasks = Vec::new();
    for (i, env) in envs.into_iter().enumerate() {
        let (steps, payload) = (a.steps, payload.clone());
        tasks.push(tokio::spawn(async move {
            let (mut step, mut ex, mut wr, mut rd) = (vec![], vec![], vec![], vec![]);
            let mut errs = 0;
            let mut first_errs: Vec<String> = Vec::new();
            for s in 0..steps {
                let t = Instant::now();
                let r: Result<(), String> = async {
                    let t1 = Instant::now();
                    let o = env
                        .sh(format!("cd /tmp && ls >/dev/null && printf '{{\"reward\": {}.{}}}' > reward-{i}.json", s % 2, s), Some(30_000))
                        .await
                        .map_err(|e| e.to_string())?;
                    if o.exit.code != Some(0) {
                        return Err("exec failed".into());
                    }
                    ex.push(ms(t1));
                    let t2 = Instant::now();
                    env.upload(format!("/tmp/action-{i}.json"), payload.clone(), None)
                        .await
                        .map_err(|e| e.to_string())?;
                    wr.push(ms(t2));
                    let t3 = Instant::now();
                    let b = env.download(format!("/tmp/reward-{i}.json"), None).await.map_err(|e| e.to_string())?;
                    if b.is_empty() {
                        return Err("empty reward".into());
                    }
                    rd.push(ms(t3));
                    Ok(())
                }
                .await;
                match r {
                    Ok(()) => step.push(ms(t)),
                    Err(e) => {
                        errs += 1;
                        if first_errs.len() < 2 {
                            first_errs.push(e.chars().take(200).collect());
                        }
                    }
                }
            }
            (step, ex, wr, rd, errs, first_errs)
        }));
    }
    let (mut st, mut ex, mut wr, mut rd, mut errs) = (vec![], vec![], vec![], vec![], 0);
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for t in tasks {
        let (a1, b, c, d, e, fe) = t.await?;
        for x in fe {
            // Group by the message without numbers (ports, names).
            let k: String = x
                .chars()
                .filter(|c| !c.is_ascii_digit())
                .take(120)
                .collect();
            *kinds.entry(k).or_default() += 1;
        }
        st.extend(a1);
        ex.extend(b);
        wr.extend(c);
        rd.extend(d);
        errs += e;
    }
    let secs = wall.elapsed().as_secs_f64();
    for (l, v) in [
        ("step_ms", &st),
        ("exec_ms", &ex),
        ("write_ms", &wr),
        ("read_reward_ms", &rd),
    ] {
        println!("{}", stats(l, v));
    }
    let mut s = stats("step_ms", &st);
    s["summary"] = true.into();
    s["cmd"] = "rollout".into();
    s["label"] = a.label.into();
    s["sandboxes"] = names.len().into();
    s["steps_ok"] = st.len().into();
    s["errors"] = errs.into();
    s["secs"] = secs.into();
    s["steps_per_sec"] = (st.len() as f64 / secs).into();
    s["error_kinds"] = serde_json::to_value(&kinds).unwrap_or_default();
    println!("{s}");
    Ok(())
}

async fn pool(cua: Arc<Cua>, a: Args) -> Result<(), Box<dyn std::error::Error>> {
    let sbs = cua.sandboxes();
    let o = PoolCreateOptions {
        name: Some(a.name.clone()),
        kind: a.kind.clone(),
        runtime: a.runtime.clone(),
        cpus: a.cpus,
        memory_mb: a.memory_mb,
        min: a.n as u32,
        headless: a.headless,
        ..Default::default()
    };
    let t = Instant::now();
    sbs.create_pool(a.image.clone(), o).await?;
    loop {
        let p = sbs.get_pool(a.name.clone()).await?;
        if p.ready as usize >= a.n {
            println!(
                "{}",
                serde_json::json!({"cmd": "pool-fill", "label": a.label, "size": a.n, "fill_ms": ms(t), "status": p.status})
            );
            break;
        }
        if t.elapsed().as_secs() > 900 {
            println!(
                "{}",
                serde_json::json!({"cmd": "pool-fill", "label": a.label, "error": "timeout", "ready": p.ready})
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let half = (a.n / 2).max(1);
    for (mode, conc) in [("serial", 1usize), ("batch", half)] {
        if mode == "batch" {
            // let the pool refill after the serial claims
            let t = Instant::now();
            while (sbs.get_pool(a.name.clone()).await?.ready as usize) < a.n
                && t.elapsed().as_secs() < 600
            {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
        let wall = Instant::now();
        let sem = Arc::new(tokio::sync::Semaphore::new(conc));
        let mut tasks = Vec::new();
        for i in 0..half {
            let (cua, a, sem) = (cua.clone(), a.clone(), sem.clone());
            tasks.push(tokio::spawn(async move {
                let _p = sem.acquire_owned().await.unwrap();
                let name = format!("{}{mode}-{i}", a.prefix);
                let mut o = SandboxCreateOptions::new("local", a.image.clone());
                o.name = Some(name.clone());
                o.pool = Some(a.name.clone());
                o.kind = a.kind.clone();
                o.runtime = a.runtime.clone();
                o.cpus = a.cpus;
                o.memory_mb = a.memory_mb;
                o.headless = a.headless;
                let t0 = Instant::now();
                let r: Result<f64, String> = async {
                    let sb = cua.sandboxes().create(o).await.map_err(|e| e.to_string())?;
                    let env = sb.spacesd(Some(60_000)).await.map_err(|e| e.to_string())?;
                    env.sh("true".into(), Some(60_000))
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(ms(t0))
                }
                .await;
                println!(
                    "{}",
                    serde_json::json!({"cmd": "pool-claim", "mode": mode, "i": i,
                    "ready_ms": r.as_ref().ok(), "error": r.as_ref().err()})
                );
                r.ok()
            }));
        }
        let mut v = Vec::new();
        for t in tasks {
            if let Some(x) = t.await? {
                v.push(x);
            }
        }
        let mut s = stats("claim_ready_ms", &v);
        s["summary"] = true.into();
        s["cmd"] = "pool".into();
        s["mode"] = mode.into();
        s["label"] = a.label.clone().into();
        s["n_req"] = half.into();
        s["wall_ms"] = ms(wall).into();
        println!("{s}");
    }
    let _ = sbs.delete_pool(a.name.clone()).await;
    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = parse().map_err(|e| format!("rollout_bench: {e}"))?;
    let cua = Cua::connect(None, None)?;
    match a.cmd.as_str() {
        "create" => start_many(cua, a, false).await,
        "fork" => start_many(cua, a, true).await,
        "fork-many" => fork_many(cua, a).await,
        "checkpoint" => {
            let t = Instant::now();
            let r = cua
                .sandboxes()
                .checkpoint(a.source.clone(), a.name.clone(), a.pin)
                .await;
            println!(
                "{}",
                serde_json::json!({"summary": true, "cmd": "checkpoint", "label": a.label,
                "source": a.source, "name": a.name, "ms": ms(t), "ok": r.is_ok(),
                "error": r.err().map(|e| e.to_string())})
            );
            Ok(())
        }
        "channel" => channel(cua, a).await,
        "probe" => probe(cua, a).await,
        "rollout" => rollout(cua, a).await,
        "pool" => pool(cua, a).await,
        "delete" => {
            let names = names_with(&cua, &a.prefix).await?;
            let sem = Arc::new(tokio::sync::Semaphore::new(a.conc.max(1)));
            let t = Instant::now();
            let mut tasks = Vec::new();
            for n in names.clone() {
                let (cua, sem) = (cua.clone(), sem.clone());
                tasks.push(tokio::spawn(async move {
                    let _p = sem.acquire_owned().await.unwrap();
                    cua.sandboxes()
                        .delete(n.clone())
                        .await
                        .map_err(|e| format!("{n}: {e}"))
                }));
            }
            let mut errs = Vec::new();
            for t in tasks {
                if let Err(e) = t.await? {
                    errs.push(e);
                }
            }
            println!(
                "{}",
                serde_json::json!({"summary": true, "cmd": "delete", "n": names.len(),
                "ms": ms(t), "errors": errs})
            );
            Ok(())
        }
        o => Err(format!("unknown command {o}").into()),
    }
}

/// One `Sandboxes.fork_many` batch of `--n` forks of `--source`: each fork's
/// ready time as the batch reports it, then its first exec.
async fn fork_many(cua: Arc<Cua>, a: Args) -> Result<(), Box<dyn std::error::Error>> {
    let wall = Instant::now();
    let opts = cua_sdk::ForkManyOptions {
        on: a.on.clone(),
        max_parallel_per_host: a.conc as u32,
        name_prefix: a.prefix.clone(),
        ..Default::default()
    };
    let batch = cua
        .sandboxes()
        .fork_many(a.source.clone(), a.n as u32, Some(opts))
        .await?;
    let mut execs = Vec::new();
    let (mut ready, mut failed) = (vec![], 0u32);
    while let Some(ev) = batch.next().await? {
        match ev {
            cua_sdk::ForkBatchEvent::Item { item } if item.error.is_empty() => {
                ready.push(item.elapsed_ms as f64);
                let (cua, name, at) = (cua.clone(), item.name.clone(), item.elapsed_ms);
                execs.push(tokio::spawn(async move {
                    let t = Instant::now();
                    let ok = async {
                        let sb = cua.sandboxes().connect(name.clone()).await.ok()?;
                        let env = sb.spacesd(Some(60_000)).await.ok()?;
                        let out = env.sh("true".into(), Some(60_000)).await.ok()?;
                        Some(out.exit.code == Some(0))
                    }
                    .await
                    .unwrap_or(false);
                    (name, at, ms(t), ok)
                }));
            }
            cua_sdk::ForkBatchEvent::Item { item } => {
                failed += 1;
                eprintln!("{}: {} {}", item.name, item.error_code, item.error);
            }
            cua_sdk::ForkBatchEvent::Done { summary } => {
                println!(
                    "{}",
                    serde_json::json!({"cmd": "fork-many-summary", "ok": summary.ok,
                    "failed": summary.failed, "elapsed_ms": summary.elapsed_ms,
                    "peak_parallel": summary.peak_parallel})
                );
            }
            _ => {}
        }
    }
    let mut first_exec = vec![];
    for e in execs {
        let (name, at, exec_ms, ok) = e.await?;
        println!(
            "{}",
            serde_json::json!({"cmd": "fork-many", "name": name, "ready_ms": at, "exec_ms": exec_ms, "ok": ok})
        );
        if ok {
            first_exec.push(at as f64 + exec_ms);
        }
    }
    ready.sort_by(|x, y| x.partial_cmp(y).unwrap());
    first_exec.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let pct = |v: &Vec<f64>, q: f64| {
        v.get(((v.len() as f64 - 1.0) * q).round() as usize)
            .copied()
    };
    println!(
        "{}",
        serde_json::json!({"summary": true, "cmd": "fork-many", "label": a.label, "n": a.n,
        "failed": failed, "p50": pct(&ready, 0.5), "p95": pct(&ready, 0.95),
        "first_exec_p50": pct(&first_exec, 0.5), "wall_ms": ms(wall)})
    );
    Ok(())
}
