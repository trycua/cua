//! The elastic pool from the outside: many Spaces asked for at once with
//! `on="auto"` (the pool decides where: your devices, hosts already
//! running, new cloud hosts), then a rollout-shaped loop in each, then
//! cleanup. Talks to the running `cua daemon` (run it with the daemon's
//! `CUA_HOME`), so every create goes through the daemon's one elastic pool.
//!
//!     cargo run --release -p cua-sdk --example elastic_bench -- [options]
//!
//! Options:
//!   --n N               Spaces (default 8)
//!   --conc C            creates in flight at once (default N)
//!   --on WHERE          auto (default), or a cloud (`hetzner`) to pin
//!   --image REF         default ghcr.io/trycua/linux:24.04-slim
//!   --kind K            container | vm | microvm
//!   --cpus N --memory-mb M --profile P
//!   --instance-type T --region R   pins (with --on <cloud>)
//!   --steps S           rollout steps per Space (default 20; 0: none)
//!   --payload-bytes B   bytes written per step (default 4096)
//!   --prefix P          Space name prefix (default eb-<random>)
//!   --keep              keep the Spaces (default: delete them)
//!   --plan              print the pool's plan first (`cloud_plan`)
//!   --label TEXT        copied into every JSON line
//!
//! One JSON line per Space (`create_ms`, `first_exec_ms`, `host`, `ok`,
//! `error`), one per rollout (`step_ms_p50`), and a summary line
//! (`"summary":true`): create p50/p95/max, wall time, steps per second,
//! failures, and the elastic hosts with their cost afterwards.

use std::sync::Arc;
use std::time::Instant;

use cua_sdk::{Cua, SpaceCreateOptions};

#[derive(Clone)]
struct Args {
    n: usize,
    conc: usize,
    on: String,
    image: String,
    kind: Option<String>,
    cpus: Option<u32>,
    memory_mb: Option<u64>,
    profile: Option<String>,
    instance_type: Option<String>,
    region: Option<String>,
    steps: usize,
    payload: usize,
    prefix: String,
    keep: bool,
    plan: bool,
    label: String,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        n: 8,
        conc: 0,
        on: "auto".into(),
        image: "ghcr.io/trycua/linux:24.04-slim".into(),
        kind: None,
        cpus: None,
        memory_mb: None,
        profile: None,
        instance_type: None,
        region: None,
        steps: 20,
        payload: 4096,
        prefix: format!("eb-{:06x}", rand_u32() & 0xff_ffff),
        keep: false,
        plan: false,
        label: String::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--n" => a.n = v()?.parse().map_err(|e| format!("{e}"))?,
            "--conc" => a.conc = v()?.parse().map_err(|e| format!("{e}"))?,
            "--on" => a.on = v()?,
            "--image" => a.image = v()?,
            "--kind" => a.kind = Some(v()?),
            "--cpus" => a.cpus = Some(v()?.parse().map_err(|e| format!("{e}"))?),
            "--memory-mb" => a.memory_mb = Some(v()?.parse().map_err(|e| format!("{e}"))?),
            "--profile" => a.profile = Some(v()?),
            "--instance-type" => a.instance_type = Some(v()?),
            "--region" => a.region = Some(v()?),
            "--steps" => a.steps = v()?.parse().map_err(|e| format!("{e}"))?,
            "--payload-bytes" => a.payload = v()?.parse().map_err(|e| format!("{e}"))?,
            "--prefix" => a.prefix = v()?,
            "--keep" => a.keep = true,
            "--plan" => a.plan = true,
            "--label" => a.label = v()?,
            o => return Err(format!("unknown option {o}")),
        }
    }
    if a.conc == 0 {
        a.conc = a.n;
    }
    Ok(a)
}

fn rand_u32() -> u32 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ (d.as_secs() as u32))
        .unwrap_or(7);
    t.wrapping_mul(2654435761)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[derive(Default)]
struct Created {
    id: String,
    create_ms: f64,
    first_exec_ms: f64,
    host: String,
    ok: bool,
    error: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let a = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let cua = Cua::connect(None, None).expect("a running cua daemon (cua daemon start)");
    let spaces = cua.spaces();
    if a.plan {
        match spaces
            .cloud_plan(cua_sdk::ElasticPlanRequest {
                count: Some(a.n as u32),
                image: Some(a.image.clone()),
                kind: a.kind.clone(),
                cpus: a.cpus,
                memory_mb: a.memory_mb,
                caller: Some("batch".into()),
                provider: (a.on != "auto").then(|| a.on.clone()),
                ..Default::default()
            })
            .await
        {
            Ok(p) => println!(
                "{}",
                serde_json::json!({"plan": true, "label": a.label, "explain": p.explain,
                    "new_hosts": p.new_hosts.len(), "added_usd_per_hour": p.added_usd_per_hour,
                    "projected_usd": p.projected_usd, "solver": p.solver, "ready_in_secs": p.ready_in_secs})
            ),
            Err(e) => println!(
                "{}",
                serde_json::json!({"plan": true, "error": e.to_string()})
            ),
        }
    }
    let started = Instant::now();
    let sem = Arc::new(tokio::sync::Semaphore::new(a.conc));
    let mut tasks = Vec::new();
    for i in 0..a.n {
        let sem = sem.clone();
        let spaces = spaces.clone();
        let a = a.clone();
        tasks.push(tokio::spawn(async move {
            let _p = sem.acquire().await;
            let mut c = Created::default();
            let t = Instant::now();
            let r = spaces
                .create(SpaceCreateOptions {
                    image: Some(a.image.clone()),
                    on: Some(a.on.clone()),
                    kind: a.kind.clone(),
                    name: Some(format!("{}-{i}", a.prefix)),
                    cpus: a.cpus,
                    memory_mb: a.memory_mb,
                    profile: a.profile.clone(),
                    instance_type: a.instance_type.clone(),
                    region: a.region.clone(),
                    wait: Some(true),
                    ..Default::default()
                })
                .await;
            c.create_ms = t.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(res) => match res.space {
                    Some(info) => {
                        c.id = info.id.clone();
                        c.host = info.host_name.clone();
                        let t = Instant::now();
                        match spaces.space(info.id.clone()).await {
                            Ok(s) => match s.bash("true".into(), Some(60_000)).await {
                                Ok(_) => {
                                    c.first_exec_ms = t.elapsed().as_secs_f64() * 1000.0;
                                    c.ok = true;
                                }
                                Err(e) => c.error = format!("exec: {e}"),
                            },
                            Err(e) => c.error = format!("connect: {e}"),
                        }
                    }
                    None => c.error = "still starting".into(),
                },
                Err(e) => c.error = e.to_string(),
            }
            println!(
                "{}",
                serde_json::json!({"i": i, "label": a.label, "id": c.id, "host": c.host,
                    "create_ms": c.create_ms.round(), "first_exec_ms": c.first_exec_ms.round(),
                    "ok": c.ok, "error": c.error})
            );
            c
        }));
    }
    let mut created = Vec::new();
    for t in tasks {
        if let Ok(c) = t.await {
            created.push(c);
        }
    }
    let create_wall = started.elapsed().as_secs_f64();
    // Rollout: every Space runs `steps` of exec + write + read the reward.
    let rolled = Instant::now();
    let mut step_tasks = Vec::new();
    for c in created.iter().filter(|c| c.ok) {
        let spaces = spaces.clone();
        let id = c.id.clone();
        let a = a.clone();
        step_tasks.push(tokio::spawn(async move {
            let Ok(s) = spaces.space(id).await else {
                return (0usize, vec![]);
            };
            let payload = vec![b'x'; a.payload];
            let mut lat = Vec::new();
            let mut done = 0;
            for step in 0..a.steps {
                let t = Instant::now();
                let ok = s
                    .bash(format!("echo step {step}"), Some(30_000))
                    .await
                    .is_ok()
                    && s.write("/tmp/eb-step".into(), payload.clone())
                        .await
                        .is_ok()
                    && s.bash("wc -c < /tmp/eb-step".into(), Some(30_000))
                        .await
                        .is_ok();
                lat.push(t.elapsed().as_secs_f64() * 1000.0);
                if ok {
                    done += 1;
                }
            }
            (done, lat)
        }));
    }
    let mut steps_done = 0;
    let mut step_lat = Vec::new();
    for t in step_tasks {
        if let Ok((d, l)) = t.await {
            steps_done += d;
            step_lat.extend(l);
        }
    }
    let roll_wall = rolled.elapsed().as_secs_f64();
    let mut cm: Vec<f64> = created
        .iter()
        .filter(|c| c.ok)
        .map(|c| c.create_ms)
        .collect();
    let mut fe: Vec<f64> = created
        .iter()
        .filter(|c| c.ok)
        .map(|c| c.first_exec_ms)
        .collect();
    let mut hosts: std::collections::BTreeMap<String, u32> = Default::default();
    for c in created.iter().filter(|c| c.ok) {
        *hosts
            .entry(if c.host.is_empty() {
                "this machine".into()
            } else {
                c.host.clone()
            })
            .or_default() += 1;
    }
    let elastic = spaces.cloud_hosts().await.unwrap_or_default();
    println!(
        "{}",
        serde_json::json!({
            "summary": true, "label": a.label, "n": a.n,
            "ok": created.iter().filter(|c| c.ok).count(),
            "failed": created.iter().filter(|c| !c.ok).count(),
            "create_wall_s": (create_wall * 10.0).round() / 10.0,
            "create_ms_p50": pct(&mut cm.clone(), 0.5).round(),
            "create_ms_p95": pct(&mut cm.clone(), 0.95).round(),
            "create_ms_max": pct(&mut cm, 1.0).round(),
            "first_exec_ms_p50": pct(&mut fe, 0.5).round(),
            "steps": steps_done,
            "steps_per_s": if roll_wall > 0.0 { ((steps_done as f64 / roll_wall) * 10.0).round() / 10.0 } else { 0.0 },
            "step_ms_p50": pct(&mut step_lat.clone(), 0.5).round(),
            "step_ms_p95": pct(&mut step_lat, 0.95).round(),
            "by_host": hosts,
            "elastic_hosts": elastic.iter().map(|h| serde_json::json!({"name": h.name, "type": h.type_name,
                "state": h.state, "usd_per_hour": h.usd_per_hour})).collect::<Vec<_>>(),
        })
    );
    if !a.keep {
        let mut dels = Vec::new();
        for c in created.into_iter().filter(|c| !c.id.is_empty()) {
            let spaces = spaces.clone();
            dels.push(tokio::spawn(async move { spaces.delete(c.id).await }));
        }
        for d in dels {
            let _ = d.await;
        }
    }
}
