// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Space under test and the helpers that reach inside it.
//!
//! Guest-side work (fixtures, server CPU) goes through the spacesd's own
//! process API, so the same code drives a local container, a sidecar that
//! shares the container's network, or any remote Space. Docker is used only
//! to start/stop local containers (scripts/space.sh) and, when the container
//! name is known, for the host-side view of its CPU.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const FIXTURE: &str = include_str!("../fixtures/benchfix.py");

pub fn unix_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Space {
    pub url: String,
    pub token: String,
    pub quic_addr: SocketAddr,
    pub time_addr: SocketAddr,
    pub runtime: String,
    /// Local docker container, when this process started or can see it.
    pub docker_name: Option<String>,
    docker_sock: Option<String>,
    env: cua_spacesd_client::SpacesdClient,
}

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(format!(
            "{:?} failed: {}{}",
            cmd,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// scripts/space.sh wrapper for local containers.
pub struct Launcher {
    pub script: PathBuf,
    pub name: String,
    pub port_base: u16,
}

impl Launcher {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("bash");
        cmd.arg(&self.script)
            .args(args)
            .args(["--name", &self.name]);
        cmd.env("CUA_BENCH_PORT_BASE", self.port_base.to_string());
        cmd
    }

    pub fn start(&self, runtime: &str, image: Option<&str>) -> Result<String> {
        let mut args = vec!["start", "--runtime", runtime];
        if let Some(image) = image {
            args.extend(["--image", image]);
        }
        run(&mut self.cmd(&args))?;
        let env = run(&mut self.cmd(&["env"]))?;
        Ok(env
            .lines()
            .find_map(|l| l.strip_prefix("export CUA_ENV_TOKEN="))
            .ok_or("space.sh env printed no token")?
            .trim()
            .to_owned())
    }

    pub fn stop(&self) {
        let _ = run(&mut self.cmd(&["stop"]));
    }

    pub fn addrs(&self) -> (String, SocketAddr, SocketAddr) {
        (
            format!("http://127.0.0.1:{}", self.port_base + 11),
            SocketAddr::from(([127, 0, 0, 1], self.port_base + 12)),
            SocketAddr::from(([127, 0, 0, 1], self.port_base + 81)),
        )
    }
}

impl Space {
    pub async fn connect(
        url: String,
        token: String,
        quic_addr: SocketAddr,
        time_addr: SocketAddr,
        runtime: String,
        docker_name: Option<String>,
    ) -> Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let env = loop {
            match cua_spacesd_client::SpacesdClient::connect_url(&url, Some(token.clone())).await {
                Ok(env) => break env,
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(error) => return Err(format!("spacesd at {url}: {error}").into()),
            }
        };
        let docker_sock = docker_name.as_ref().and_then(|_| docker_socket());
        Ok(Self {
            url,
            token,
            quic_addr,
            time_addr,
            runtime,
            docker_name,
            docker_sock,
            env,
        })
    }

    /// Run a shell line in the guest (as the driver's user), bounded.
    pub async fn exec(&self, line: &str) -> Result<String> {
        let cmd =
            cua_spacesd_client::Command::shell(line.to_owned()).timeout(Duration::from_secs(30));
        let out = self.env.run(cmd).await?;
        if out.status.code != Some(0) {
            return Err(format!(
                "guest `{}` exited {:?}: {}",
                line.chars().take(80).collect::<String>(),
                out.status.code,
                String::from_utf8_lossy(&out.stderr)
            )
            .into());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// (Re)start the bench fixture in `mode` and stop the A/V fixture.
    pub async fn fixture(&self, mode: &str) -> Result<()> {
        use base64_lite::encode;
        let script = format!(
            "cua-fixtures stop avsync >/dev/null 2>&1 || true; \
             if [ -r /tmp/benchfix.pid ]; then kill $(cat /tmp/benchfix.pid) 2>/dev/null || true; sleep 0.2; fi; \
             echo {} | base64 -d > /tmp/benchfix.py; \
             if [ -r /run/cua-desktop/desktop.env ]; then set -a; . /run/cua-desktop/desktop.env; set +a; fi; \
             log=/tmp/cua-fixtures/benchfix.jsonl; before=$(cat $log 2>/dev/null | wc -l); \
             setsid python3 /tmp/benchfix.py --mode {mode} >/tmp/benchfix.out 2>&1 < /dev/null & echo $! > /tmp/benchfix.pid; \
             for i in $(seq 1 50); do tail -n +$((before + 1)) $log 2>/dev/null | grep -q '\"type\": \"ready\"' && exit 0; sleep 0.2; done; \
             cat /tmp/benchfix.out >&2; exit 1",
            encode(FIXTURE.as_bytes())
        );
        self.exec(&script).await?;
        Ok(())
    }

    pub async fn avsync(&self, on: bool) -> Result<()> {
        let verb = if on { "start" } else { "stop" };
        self.exec(&format!("cua-fixtures {verb} avsync")).await?;
        Ok(())
    }

    /// cua-spacesd utime+stime in seconds (the guest's /proc).
    pub async fn driver_cpu_s(&self) -> Option<f64> {
        let out = self
            .exec("pid=$(pgrep -x cua-spacesd | head -1); cat /proc/$pid/stat; getconf CLK_TCK")
            .await
            .ok()?;
        let mut lines = out.lines();
        let stat = lines.next()?;
        let tck: f64 = lines.next()?.trim().parse().ok()?;
        let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        // After ")": state is field 3; utime is field 14, stime 15.
        let utime: f64 = fields.get(11)?.parse().ok()?;
        let stime: f64 = fields.get(12)?.parse().ok()?;
        Some((utime + stime) / tck)
    }

    /// Whole-container CPU seconds as the host sees it (docker cgroup; for
    /// runsc this includes the gVisor sandbox itself).
    pub fn container_cpu_s(&self) -> Option<f64> {
        let (sock, name) = (self.docker_sock.as_ref()?, self.docker_name.as_ref()?);
        let out = Command::new("curl")
            .args(["-s", "--max-time", "5", "--unix-socket", sock])
            .arg(format!(
                "http://localhost/containers/{name}/stats?stream=false&one-shot=true"
            ))
            .output()
            .ok()?;
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        Some(
            value
                .pointer("/cpu_stats/cpu_usage/total_usage")?
                .as_f64()?
                / 1e9,
        )
    }
}

fn docker_socket() -> Option<String> {
    let out = Command::new("docker")
        .args([
            "context",
            "inspect",
            "--format",
            "{{.Endpoints.docker.Host}}",
        ])
        .output()
        .ok()?;
    let host = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Some(
        host.strip_prefix("unix://")
            .map(str::to_owned)
            .unwrap_or_else(|| "/var/run/docker.sock".into()),
    )
}

/// Guest-minus-host clock offset in ns from the fixture's time server
/// (min-RTT of `samples` pings), and that sample's RTT.
pub fn clock_offset(addr: SocketAddr, samples: usize) -> Result<(i64, i64)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(error) => return Err(format!("time server {addr}: {error}").into()),
        }
    };
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut best: Option<(i64, i64)> = None;
    for _ in 0..samples.clamp(1, 1000) {
        let t0 = unix_ns();
        stream.write_all(b"t")?;
        let mut buf = [0u8; 8];
        stream.read_exact(&mut buf)?;
        let t1 = unix_ns();
        let rtt = t1 - t0;
        let offset = i64::from_be_bytes(buf) - (t0 + t1) / 2;
        if best.is_none_or(|(_, r)| rtt < r) {
            best = Some((offset, rtt));
        }
    }
    best.ok_or_else(|| "no clock samples".into())
}

/// Minimal standard base64 (no dependency needed for one fixture upload).
mod base64_lite {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[cfg(test)]
    #[test]
    fn encodes_rfc4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(input.as_bytes()), want);
        }
    }
}
