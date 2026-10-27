//! The coordinator-to-Space channel: exec round trips, file throughput and
//! streamed stdout, through the SDK the way an RL coordinator calls a Space.
//!
//!     cargo run --release -p cua-sdk --example channel_bench -- <target> [options]
//!
//! Target (one):
//!   --space S          a Space by id or name (Spaces API, `Space.spacesd()`)
//!   --sandbox S        a sandbox by name (`Sandbox.spacesd()`)
//!   --url U            a cua-spacesd at U directly (token: `CUA_ENV_TOKEN`)
//!
//! Options:
//!   --embedded         run the SDK runtime in this process (default: the
//!                      running `cua daemon`)
//!   --iters N          exec samples per kind (default 300)
//!   --files SPEC       MiB:reps list (default `1:20,100:5`; empty = none)
//!   --stream-mib N     streamed stdout size (default 256; 0 = none)
//!   --forward-mib N    with --space: `Space.forward` to a guest TCP server
//!                      (python3 in the guest) that sends N MiB; connect
//!                      time, first byte and MiB/s (default 0 = none)
//!   --label TEXT       copied into the summary line
//!
//! Metrics: `rpc_health_ms` (a call that runs nothing: the transport's round
//! trip), `exec_run_ms` / `exec_sh_ms` (run `true` directly / through
//! `/bin/sh`), `upload_*` / `download_*` (each size, MiB/s at p50) and
//! `stream_stdout` (MiB/s of a process's stdout).
//!
//! One JSON line per metric, then a summary line (`"summary":true`) with all
//! of them and the client's route (`endpoint`, `transport`, `route`).
//! `libs/cua/bench/channel/check.py` compares a summary with the committed
//! baselines.

use cua_sdk::{Cua, CuaConfig, SpacesdClient, SpacesdCommand, UploadOptions};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
struct Args {
    space: Option<String>,
    sandbox: Option<String>,
    url: Option<String>,
    embedded: bool,
    iters: usize,
    files: Vec<(usize, usize)>,
    stream_mib: u64,
    forward_mib: u64,
    label: String,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        space: None,
        sandbox: None,
        url: None,
        embedded: false,
        iters: 300,
        files: vec![(1, 20), (100, 5)],
        stream_mib: 256,
        forward_mib: 0,
        label: String::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--space" => a.space = Some(v()?),
            "--sandbox" => a.sandbox = Some(v()?),
            "--url" => a.url = Some(v()?),
            "--embedded" => a.embedded = true,
            "--iters" => a.iters = v()?.parse().map_err(|e| format!("--iters: {e}"))?,
            "--files" => {
                let spec = v()?;
                a.files = spec
                    .split(',')
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| {
                        let (m, r) = s.split_once(':').ok_or(format!("--files {s:?}"))?;
                        Ok((
                            m.trim().parse().map_err(|e| format!("{e}"))?,
                            r.trim().parse().map_err(|e| format!("{e}"))?,
                        ))
                    })
                    .collect::<Result<_, String>>()?;
            }
            "--stream-mib" => a.stream_mib = v()?.parse().map_err(|e| format!("{e}"))?,
            "--forward-mib" => a.forward_mib = v()?.parse().map_err(|e| format!("{e}"))?,
            "--label" => a.label = v()?,
            o => return Err(format!("unknown option {o}")),
        }
    }
    if [&a.space, &a.sandbox, &a.url]
        .iter()
        .filter(|t| t.is_some())
        .count()
        != 1
    {
        return Err("give exactly one of --space, --sandbox, --url".into());
    }
    Ok(a)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn pct(v: &[f64], p: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = ((p / 100.0) * s.len() as f64).ceil().max(1.0) as usize;
    s[rank.min(s.len()) - 1]
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn stats(metric: &str, v: &[f64]) -> serde_json::Value {
    serde_json::json!({"metric": metric, "n": v.len(), "p50": round(pct(v, 50.0)),
        "p95": round(pct(v, 95.0)), "p99": round(pct(v, 99.0)), "max": round(pct(v, 100.0))})
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

async fn client(
    cua: &Arc<Cua>,
    a: &Args,
) -> Result<Arc<SpacesdClient>, Box<dyn std::error::Error>> {
    if let Some(s) = &a.space {
        return Ok(cua.spaces().space(s.clone()).await?.spacesd()?);
    }
    if let Some(s) = &a.sandbox {
        let sb = cua.sandboxes().connect(s.clone()).await?;
        return Ok(sb.spacesd(Some(30_000)).await?);
    }
    let url = a.url.clone().expect("checked");
    Ok(cua
        .spacesd(url, std::env::var("CUA_ENV_TOKEN").ok())
        .await?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = parse().map_err(|e| format!("channel_bench: {e}"))?;
    let cua = if a.embedded {
        Cua::embedded(CuaConfig::default())?
    } else {
        Cua::connect(None, None)?
    };
    let env = client(&cua, &a).await?;
    let mut out = Vec::new();

    // The transport alone: a call that runs nothing (`Health`).
    {
        for _ in 0..10 {
            env.health().await?;
        }
        let mut v = Vec::with_capacity(a.iters);
        for _ in 0..a.iters {
            let t = Instant::now();
            env.health().await?;
            v.push(ms(t));
        }
        let s = stats("rpc_health_ms", &v);
        println!("{s}");
        out.push(s);
    }

    // Exec round trips: a program with no shell, and a shell line.
    for (metric, sh) in [("exec_run_ms", false), ("exec_sh_ms", true)] {
        for _ in 0..10 {
            env.run(cmd("true", &[])).await?;
        }
        let mut v = Vec::with_capacity(a.iters);
        for _ in 0..a.iters {
            let t = Instant::now();
            let o = if sh {
                env.sh("true".into(), None).await?
            } else {
                env.run(cmd("true", &[])).await?
            };
            v.push(ms(t));
            assert_eq!(o.exit.code, Some(0), "{metric}: {o:?}");
        }
        let s = stats(metric, &v);
        println!("{s}");
        out.push(s);
    }

    // Files: upload then download each size, `reps` times.
    for &(mib, reps) in &a.files {
        let size = mib << 20;
        let data: Vec<u8> = (0..size)
            .map(|i| (i as u32).wrapping_mul(2_654_435_761) as u8)
            .collect();
        let (mut up, mut down) = (Vec::new(), Vec::new());
        for r in 0..reps {
            let path = format!("/tmp/channel-bench-{mib}-{r}.bin");
            let payload = data.clone();
            let t = Instant::now();
            env.upload(
                path.clone(),
                payload,
                Some(UploadOptions {
                    create_parents: true,
                    ..Default::default()
                }),
            )
            .await?;
            up.push(ms(t));
            let t = Instant::now();
            let got = env.download(path.clone(), None).await?;
            down.push(ms(t));
            assert_eq!(got.len(), size);
            let _ = env.remove(path, false, None).await;
        }
        for (dir, v) in [("upload", &up), ("download", &down)] {
            let mut s = stats(&format!("{dir}_{mib}MiB_ms"), v);
            s["MiBps_p50"] = round(mib as f64 / (pct(v, 50.0) / 1000.0)).into();
            println!("{s}");
            out.push(s);
        }
    }

    // Streamed stdout.
    if a.stream_mib > 0 {
        let bytes = a.stream_mib << 20;
        let p = env
            .spawn(cmd(
                "/bin/sh",
                &["-c", &format!("head -c {bytes} /dev/zero")],
            ))
            .await?;
        let t = Instant::now();
        let mut got = 0u64;
        let mut first = None;
        while let Some(ev) = p.next_event().await? {
            if first.is_none() && !ev.data.is_empty() {
                first = Some(round(ms(t)));
            }
            got += ev.data.len() as u64;
        }
        let secs = t.elapsed().as_secs_f64();
        // spacesd keeps a bounded scrollback per process and never waits
        // for a slow reader: what a reader falls behind by more than that
        // is skipped (and counted here).
        let s = serde_json::json!({"metric": "stream_stdout", "MiB": a.stream_mib,
            "MiBps": round(got as f64 / (1u64 << 20) as f64 / secs), "first_byte_ms": first,
            "skipped_bytes": bytes - got.min(bytes)});
        println!("{s}");
        out.push(s);
    }

    // A port forward over the Space's own channel (`Space.forward`).
    if a.forward_mib > 0 {
        let space = cua
            .spaces()
            .space(a.space.clone().ok_or("--forward-mib needs --space")?)
            .await?;
        let port: u16 = 47_311;
        let bytes = a.forward_mib << 20;
        let server = format!(
            "import socket\ns=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
             s.bind(('127.0.0.1', {port})); s.listen(1); c, _ = s.accept(); b = bytes(1 << 20)\n\
             for _ in range({}): c.sendall(b)\nc.close()",
            a.forward_mib
        );
        let guest = env.spawn(cmd("python3", &["-c", &server])).await?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let t = Instant::now();
        let fwd = space.forward(port, None).await?;
        let opened = ms(t);
        let local = fwd.local_addr().ok_or("no local address")?;
        use tokio::io::AsyncReadExt;
        let t = Instant::now();
        let mut c = tokio::net::TcpStream::connect(&local).await?;
        let mut buf = vec![0u8; 1 << 20];
        let (mut got, mut first) = (0u64, None);
        while got < bytes {
            let n = c.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            first.get_or_insert_with(|| round(ms(t)));
            got += n as u64;
        }
        let secs = t.elapsed().as_secs_f64();
        assert_eq!(got, bytes, "forwarded bytes");
        let _ = guest.wait().await;
        fwd.close().await?;
        let s = serde_json::json!({"metric": "forward", "MiB": a.forward_mib,
            "open_ms": round(opened), "first_byte_ms": first,
            "MiBps": round(got as f64 / (1u64 << 20) as f64 / secs), "local_addr": local});
        println!("{s}");
        out.push(s);
    }

    println!(
        "{}",
        serde_json::json!({"summary": true, "label": a.label,
            "target": a.space.clone().or(a.sandbox.clone()).or(a.url.clone()),
            "endpoint": env.endpoint(), "transport": env.transport(), "route": env.route().await.unwrap_or_default(),
            "results": out})
    );
    Ok(())
}
