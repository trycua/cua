//! The doctor shim: the same report for images without cua-spacesd, through
//! whatever API the image already runs.
//!
//! - `computer-server=<url>`: the legacy computer-server (`POST /cmd`,
//!   SSE-framed replies; Windows workspaces, older VMs);
//! - `osworld=<url>`: the OSWorld desktop_env Flask server (`/screenshot`,
//!   `/execute`, `/accessibility`, `/platform`, `/screen_size`);
//! - `mcp=<url>`: a streamable-HTTP cua-driver MCP endpoint.
//!
//! It runs from the host against the forwarded port, so nothing is copied
//! into the guest. The checks that map onto those APIs run (reachability,
//! screenshot, screen size, process, files, clock skew, disk, units,
//! accessibility, and with `--effects virtual` a cursor round trip); the
//! report has `spacesd: null` and `producer: cua-doctor-shim/<kind>`.
//! Severity follows `--expect-manifest` like everywhere else.

use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use cua_sdk::CuaError;
use cua_spacesd_client::diagnose::{Artifact, Check, Environment, Fidelity, Image, Report, Status};
use cua_spacesd_client::manifest::Loaded;
use serde_json::{Value, json};

/// Largest reply read from the target.
const MAX_REPLY: usize = 32 * 1024 * 1024;

/// What the shim talks to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Legacy computer-server.
    ComputerServer(String),
    /// OSWorld desktop_env server.
    OsWorld(String),
    /// cua-driver MCP.
    Mcp(String),
}

impl Target {
    /// Parses `kind=url`.
    pub fn parse(spec: &str) -> Result<Self, CuaError> {
        let (kind, url) = spec.split_once('=').ok_or_else(|| {
            CuaError::InvalidArgument(format!("--shim wants KIND=URL, got {spec:?}"))
        })?;
        let url = url.trim_end_matches('/').to_owned();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(CuaError::InvalidArgument(format!(
                "--shim URL must be http(s): {url}"
            )));
        }
        Ok(match kind {
            "computer-server" | "computer_server" => Target::ComputerServer(url),
            "osworld" => Target::OsWorld(url),
            "mcp" => Target::Mcp(url),
            other => {
                return Err(CuaError::InvalidArgument(format!(
                    "unknown shim kind {other:?} (computer-server, osworld, mcp)"
                )));
            }
        })
    }

    fn kind(&self) -> &'static str {
        match self {
            Target::ComputerServer(_) => "computer-server",
            Target::OsWorld(_) => "osworld",
            Target::Mcp(_) => "mcp",
        }
    }

    fn url(&self) -> &str {
        match self {
            Target::ComputerServer(u) | Target::OsWorld(u) | Target::Mcp(u) => u,
        }
    }
}

/// Command output.
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// Exit code.
    pub code: i64,
    /// stdout.
    pub stdout: String,
}

struct Shim {
    target: Target,
    http: reqwest::Client,
    /// Bearer for targets behind auth (a spacesd `/mcp`).
    token: Option<String>,
    mcp_session: tokio::sync::Mutex<Option<String>>,
}

fn sse_json(text: &str) -> Result<Value, String> {
    let body = text
        .lines()
        .find_map(|l| l.strip_prefix("data: ").or_else(|| l.strip_prefix("data:")))
        .unwrap_or(text);
    serde_json::from_str(body.trim()).map_err(|e| format!("reply is not JSON: {e}"))
}

impl Shim {
    async fn get(&self, path: &str) -> Result<(u16, Vec<u8>), String> {
        let resp = self
            .http
            .get(format!("{}{path}", self.target.url()))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_REPLY {
            return Err("reply too large".into());
        }
        Ok((status, bytes.to_vec()))
    }

    async fn post(
        &self,
        path: &str,
        body: Value,
        headers: &[(&str, String)],
    ) -> Result<(u16, String, Vec<(String, String)>), String> {
        let mut req = self
            .http
            .post(format!("{}{path}", self.target.url()))
            .json(&body);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let hdrs = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_owned()))
            .collect();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if text.len() > MAX_REPLY {
            return Err("reply too large".into());
        }
        Ok((status, text, hdrs))
    }

    /// computer-server `POST /cmd`.
    async fn cmd(&self, command: &str, params: Value) -> Result<Value, String> {
        let (status, text, _) = self
            .post("/cmd", json!({"command": command, "params": params}), &[])
            .await?;
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        let reply = sse_json(&text)?;
        if reply["success"] == false {
            return Err(reply["error"]
                .as_str()
                .unwrap_or("command failed")
                .to_owned());
        }
        Ok(reply)
    }

    /// MCP JSON-RPC over streamable HTTP.
    async fn mcp(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut headers = vec![("accept", "application/json, text/event-stream".to_owned())];
        if let Some(s) = self.mcp_session.lock().await.clone() {
            headers.push(("mcp-session-id", s));
        }
        let (status, text, hdrs) = self
            .post(
                "",
                json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
                &headers,
            )
            .await?;
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        if let Some((_, s)) = hdrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id"))
        {
            *self.mcp_session.lock().await = Some(s.clone());
        }
        let reply = sse_json(&text)?;
        if let Some(error) = reply.get("error") {
            return Err(error.to_string());
        }
        Ok(reply["result"].clone())
    }

    async fn mcp_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        let result = self
            .mcp("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        if result["isError"] == true {
            return Err(result["content"][0]["text"]
                .as_str()
                .unwrap_or("tool error")
                .to_owned());
        }
        Ok(result)
    }

    async fn reachable(&self) -> Result<String, String> {
        match &self.target {
            Target::ComputerServer(_) => {
                let (status, body) = self.get("/status").await?;
                if status != 200 {
                    return Err(format!("GET /status: HTTP {status}"));
                }
                let version = self.cmd("version", json!({})).await.ok();
                Ok(format!(
                    "/status {}; version {}",
                    String::from_utf8_lossy(&body).trim(),
                    version
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "unknown".into())
                ))
            }
            Target::OsWorld(_) => {
                let (status, body) = self.get("/platform").await?;
                if status != 200 {
                    return Err(format!("GET /platform: HTTP {status}"));
                }
                Ok(format!(
                    "platform {}",
                    String::from_utf8_lossy(&body).trim()
                ))
            }
            Target::Mcp(_) => {
                let init = self
                    .mcp("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": {"name": "cua-doctor-shim", "version": env!("CARGO_PKG_VERSION")}}))
                    .await?;
                Ok(format!(
                    "MCP {} {}",
                    init["serverInfo"]["name"], init["serverInfo"]["version"]
                ))
            }
        }
    }

    /// Guest OS family: "linux", "windows" or "macos".
    async fn os(&self) -> String {
        let raw = match &self.target {
            Target::OsWorld(_) => self
                .get("/platform")
                .await
                .map(|(_, b)| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default(),
            Target::ComputerServer(_) => self
                .cmd("get_desktop_environment", json!({}))
                .await
                .map(|v| v["environment"].as_str().unwrap_or_default().to_owned())
                .unwrap_or_default(),
            Target::Mcp(_) => String::new(),
        }
        .to_lowercase();
        if raw.contains("windows") {
            "windows"
        } else if raw.contains("darwin") || raw.contains("mac") {
            "macos"
        } else if raw.is_empty() {
            ""
        } else {
            "linux"
        }
        .to_owned()
    }

    async fn screenshot(&self) -> Result<Vec<u8>, String> {
        match &self.target {
            Target::ComputerServer(_) => {
                let reply = self.cmd("screenshot", json!({})).await?;
                let data = reply["image_data"].as_str().ok_or("no image_data")?;
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| e.to_string())
            }
            Target::OsWorld(_) => {
                let (status, body) = self.get("/screenshot").await?;
                if status != 200 {
                    return Err(format!("HTTP {status}"));
                }
                Ok(body)
            }
            Target::Mcp(_) => {
                // get_desktop_state carries a screenshot and has a reviewed
                // risk class (authorized MCP servers refuse unreviewed
                // tools); plain `screenshot` is the fallback.
                let result = match self.mcp_tool("get_desktop_state", json!({})).await {
                    Ok(r)
                        if r["content"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|p| p["type"] == "image")) =>
                    {
                        r
                    }
                    _ => self.mcp_tool("screenshot", json!({})).await?,
                };
                let data = result["content"]
                    .as_array()
                    .and_then(|parts| parts.iter().find(|p| p["type"] == "image"))
                    .and_then(|p| p["data"].as_str())
                    .ok_or("no image content")?;
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| e.to_string())
            }
        }
    }

    async fn screen_size(&self) -> Result<(u32, u32), String> {
        let v = match &self.target {
            Target::ComputerServer(_) => {
                self.cmd("get_screen_size", json!({})).await?["size"].clone()
            }
            Target::OsWorld(_) => {
                let (_, text, _) = self.post("/screen_size", json!({}), &[]).await?;
                serde_json::from_str(&text).map_err(|e| e.to_string())?
            }
            Target::Mcp(_) => {
                let r = self.mcp_tool("get_screen_size", json!({})).await?;
                r.get("structuredContent")
                    .cloned()
                    .or_else(|| {
                        r["content"][0]["text"]
                            .as_str()
                            .and_then(|t| serde_json::from_str(t).ok())
                    })
                    .unwrap_or_default()
            }
        };
        match (v["width"].as_u64(), v["height"].as_u64()) {
            (Some(w), Some(h)) => Ok((w as u32, h as u32)),
            _ => Err(format!("no width/height in {v}")),
        }
    }

    async fn run(&self, line: &str) -> Result<Output, String> {
        match &self.target {
            Target::ComputerServer(_) => {
                let r = self.cmd("run_command", json!({"command": line})).await?;
                Ok(Output {
                    code: r["return_code"].as_i64().unwrap_or(0),
                    stdout: r["stdout"].as_str().unwrap_or_default().to_owned(),
                })
            }
            Target::OsWorld(_) => {
                let (status, text, _) = self
                    .post("/execute", json!({"command": line, "shell": true}), &[])
                    .await?;
                if status != 200 {
                    return Err(format!(
                        "HTTP {status}: {}",
                        text.chars().take(200).collect::<String>()
                    ));
                }
                let r: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                Ok(Output {
                    code: r["returncode"]
                        .as_i64()
                        .unwrap_or(if r["status"] == "success" { 0 } else { 1 }),
                    stdout: r["output"].as_str().unwrap_or_default().to_owned(),
                })
            }
            Target::Mcp(_) => Err("the MCP target runs no shell commands".into()),
        }
    }

    async fn a11y(&self) -> Result<usize, String> {
        match &self.target {
            Target::ComputerServer(_) => {
                let r = self.cmd("get_accessibility_tree", json!({})).await?;
                Ok(r.to_string().len())
            }
            Target::OsWorld(_) => {
                let (status, body) = self.get("/accessibility").await?;
                if status != 200 {
                    return Err(format!("HTTP {status}"));
                }
                let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
                Ok(v["AT"].as_str().map(str::len).unwrap_or(0))
            }
            Target::Mcp(_) => Err("not exposed".into()),
        }
    }
}

fn shell_for(os: &str) -> ShellCmds {
    match os {
        "windows" => ShellCmds {
            echo: "echo %CUA_DOCTOR%",
            env_prefix: |n| format!("set CUA_DOCTOR={n}&& "),
            exit7: "exit /b 7",
            now_ms: "powershell -NoProfile -Command [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()",
            free_bytes: "powershell -NoProfile -Command (Get-PSDrive C).Free",
            write: |p, c| {
                format!(
                    "powershell -NoProfile -Command Set-Content -NoNewline -Path '{p}' -Value '{c}'"
                )
            },
            read: |p| format!("powershell -NoProfile -Command Get-Content -Raw -Path '{p}'"),
            remove: |p| format!("del /f /q \"{p}\""),
            tmp: "C:\\Windows\\Temp",
        },
        _ => ShellCmds {
            echo: "echo \"$CUA_DOCTOR\"",
            env_prefix: |n| format!("CUA_DOCTOR={n} "),
            exit7: "sh -c 'exit 7'",
            now_ms: "python3 -c 'import time;print(int(time.time()*1000))'",
            free_bytes: "df -Pk / | awk 'NR==2{print $4*1024}'",
            write: |p, c| format!("printf '%s' '{c}' > '{p}'"),
            read: |p| format!("cat '{p}'"),
            remove: |p| format!("rm -f '{p}'"),
            tmp: "/tmp",
        },
    }
}

struct ShellCmds {
    echo: &'static str,
    env_prefix: fn(&str) -> String,
    exit7: &'static str,
    now_ms: &'static str,
    free_bytes: &'static str,
    write: fn(&str, &str) -> String,
    read: fn(&str) -> String,
    remove: fn(&str) -> String,
    tmp: &'static str,
}

fn finish(loaded: &Loaded, mut check: Check, claims: &[&str]) -> Check {
    check.severity = loaded.severity(claims);
    check.claimed_by = claims.iter().map(|c| (*c).to_owned()).collect();
    check
}

/// Runs the shim and returns its report.
pub async fn run(
    target: &Target,
    loaded: &Loaded,
    strict: bool,
    effects: bool,
    token: Option<String>,
) -> Report {
    let started = Instant::now();
    let shim = Shim {
        target: target.clone(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("http client"),
        token,
        mcp_session: Default::default(),
    };
    let nonce = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let mut checks = Vec::new();
    let mut fidelity = Fidelity::default();
    let core: &[&str] = &["core"];

    match shim.reachable().await {
        Ok(detail) => checks.push(finish(
            loaded,
            Check::new("meta.shim.reachable", Status::Pass, detail).fact("kind", target.kind()),
            core,
        )),
        Err(error) => {
            checks.push(finish(
                loaded,
                Check::new(
                    "meta.shim.reachable",
                    Status::Fail,
                    format!("{} at {}: {error}", target.kind(), target.url()),
                ),
                core,
            ));
            let mut report = report_shell(target, loaded, checks, fidelity, "");
            report.finalize(strict, started.elapsed());
            return report;
        }
    }
    let os = shim.os().await;
    if loaded.present() && !loaded.manifest.os.is_empty() && !os.is_empty() {
        checks.push(finish(
            loaded,
            Check::new(
                "annotations.os",
                super::verdict(os == loaded.manifest.os),
                format!(
                    "guest reports {os}, the image claims {}",
                    loaded.manifest.os
                ),
            ),
            &["manifest:os"],
        ));
    }
    // Screen.
    let display: &[&str] = &["manifest:display"];
    let size = shim.screen_size().await;
    match shim.screenshot().await {
        Ok(png) => {
            let decoded = image::load_from_memory(&png).map(|i| i.to_rgb8());
            let check = match decoded {
                Ok(img) => {
                    let colors = distinct_colors(&img);
                    let dims_ok = size
                        .as_ref()
                        .map(|(w, h)| {
                            (*w, *h) == (img.width(), img.height()) || *w * 2 == img.width()
                        })
                        .unwrap_or(true);
                    fidelity.display = format!("{}x{}@1", img.width(), img.height());
                    let mut c = Check::new(
                        "screenshot.display",
                        super::verdict(colors > 16 && dims_ok),
                        format!(
                            "{}x{} screenshot, {colors} distinct colours{}",
                            img.width(),
                            img.height(),
                            if dims_ok {
                                ""
                            } else {
                                ", size differs from the reported screen size"
                            }
                        ),
                    );
                    c.artifacts.push(Artifact {
                        name: "screenshot.display.png".into(),
                        media_type: "image/png".into(),
                        data: if png.len()
                            <= cua_spacesd_client::diagnose::MAX_INLINE_ARTIFACT_BYTES
                        {
                            png.clone()
                        } else {
                            Vec::new()
                        },
                        path: String::new(),
                    });
                    c
                }
                Err(error) => Check::new(
                    "screenshot.display",
                    Status::Fail,
                    format!("screenshot is not an image: {error}"),
                ),
            };
            checks.push(finish(loaded, check, display));
        }
        Err(error) => checks.push(finish(
            loaded,
            Check::new(
                "screenshot.display",
                Status::Fail,
                format!("screenshot: {error}"),
            ),
            display,
        )),
    }
    if let Err(error) = &size {
        checks.push(finish(
            loaded,
            Check::new(
                "screenshot.size",
                Status::Fail,
                format!("screen size: {error}"),
            ),
            display,
        ));
    }

    // Process, files, clock, disk (targets that run commands).
    if !matches!(target, Target::Mcp(_)) {
        let sh = shell_for(&os);
        let line = format!("{}{}", (sh.env_prefix)(&nonce), sh.echo);
        checks.push(finish(
            loaded,
            match shim.run(&line).await {
                Ok(o) => Check::new(
                    "process.run",
                    super::verdict(o.code == 0 && o.stdout.trim() == nonce),
                    format!("exit {}, stdout {:?}", o.code, o.stdout.trim()),
                ),
                Err(e) => Check::new("process.run", Status::Fail, e),
            },
            core,
        ));
        checks.push(finish(
            loaded,
            match shim.run(sh.exit7).await {
                Ok(o) => Check::new(
                    "process.exit_code",
                    super::verdict(o.code == 7),
                    format!("`exit 7` reported {}", o.code),
                ),
                Err(e) => Check::new("process.exit_code", Status::Fail, e),
            },
            core,
        ));
        let path = format!(
            "{}{}cua-doctor-{nonce}.txt",
            sh.tmp,
            if os == "windows" { "\\" } else { "/" }
        );
        let wrote = shim.run(&(sh.write)(&path, &nonce)).await;
        let read = shim.run(&(sh.read)(&path)).await;
        let _ = shim.run(&(sh.remove)(&path)).await;
        checks.push(finish(
            loaded,
            match (wrote, read) {
                (Ok(_), Ok(o)) => Check::new(
                    "files.roundtrip",
                    super::verdict(o.stdout.trim() == nonce),
                    format!("wrote and read back {path}"),
                ),
                (Err(e), _) | (_, Err(e)) => Check::new("files.roundtrip", Status::Fail, e),
            },
            core,
        ));
        let sent = SystemTime::now();
        let t0 = Instant::now();
        let clock = shim.run(sh.now_ms).await;
        let rtt = t0.elapsed();
        checks.push(finish(
            loaded,
            match clock.ok().and_then(|o| o.stdout.trim().parse::<i64>().ok()) {
                Some(guest_ms) => {
                    let host_ms = (sent
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        + rtt / 2)
                        .as_millis() as i64;
                    let skew = guest_ms - host_ms;
                    fidelity.clock_skew_ms = skew;
                    // The command round trip bounds the error; grant it.
                    let slack = rtt.as_millis() as i64 / 2;
                    let status = match skew.abs() - slack {
                        s if s > 500 => Status::Fail,
                        s if s > 100 => Status::Warn,
                        _ => Status::Pass,
                    };
                    Check::new(
                        "time.skew",
                        status,
                        format!(
                            "guest clock {skew:+} ms from this host (round trip {} ms)",
                            rtt.as_millis()
                        ),
                    )
                }
                None => Check::new("time.skew", Status::Fail, "could not read the guest clock"),
            },
            &["manifest:time"],
        ));
        checks.push(finish(
            loaded,
            match shim
                .run(sh.free_bytes)
                .await
                .ok()
                .and_then(|o| o.stdout.trim().parse::<u64>().ok())
            {
                Some(free) => {
                    let gib = free as f64 / (1024.0 * 1024.0 * 1024.0);
                    let floor = loaded.manifest.resources.min_disk_gib;
                    Check::new(
                        "resources.disk.root",
                        super::verdict(gib >= floor),
                        format!("{gib:.1} GiB free (floor {floor:.1} GiB)"),
                    )
                }
                None => Check::new(
                    "resources.disk.root",
                    Status::Fail,
                    "could not read free disk space",
                ),
            },
            &["manifest:resources"],
        ));
        if loaded.present() && !loaded.manifest.units.is_empty() {
            let mut bad = Vec::new();
            for unit in &loaded.manifest.units {
                let q = match os.as_str() {
                    "windows" => format!("sc query \"{unit}\""),
                    "macos" => format!("launchctl list {unit}"),
                    _ => format!("systemctl is-active {unit} || supervisorctl status {unit}"),
                };
                match shim.run(&q).await {
                    Ok(o) if o.code == 0 && (os != "windows" || o.stdout.contains("RUNNING")) => {}
                    _ => bad.push(unit.clone()),
                }
            }
            checks.push(finish(
                loaded,
                Check::new(
                    "init.units",
                    super::verdict(bad.is_empty()),
                    if bad.is_empty() {
                        "every claimed unit runs".into()
                    } else {
                        format!("not running: {}", bad.join(", "))
                    },
                ),
                &["manifest:units"],
            ));
        }
        match shim.a11y().await {
            Ok(n) => checks.push(finish(
                loaded,
                Check::new(
                    "a11y.tree",
                    super::verdict(n > 16),
                    format!("accessibility tree of {n} bytes"),
                ),
                &["feature:a11y"],
            )),
            Err(e) => checks.push(finish(
                loaded,
                Check::new("a11y.tree", Status::Fail, e),
                &["feature:a11y"],
            )),
        }
    } else {
        // cua-driver MCP: the contract tools must be there.
        let listed = shim.mcp("tools/list", json!({})).await;
        let names: Vec<String> = listed
            .as_ref()
            .ok()
            .and_then(|v| {
                v["tools"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|t| t["name"].as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default();
        let required = [
            "list_apps",
            "list_windows",
            "get_window_state",
            "launch_app",
            "click",
            "type_text",
            "press_key",
        ];
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|t| !names.iter().any(|n| n == t))
            .collect();
        checks.push(finish(
            loaded,
            Check::new(
                "driver.registry",
                super::verdict(listed.is_ok() && missing.is_empty()),
                if missing.is_empty() {
                    format!("{} tools, every contract tool present", names.len())
                } else {
                    format!("missing contract tools: {}", missing.join(", "))
                },
            ),
            &["feature:driver"],
        ));
    }

    // Input (effects): a cursor round trip.
    if effects {
        let result = match target {
            Target::ComputerServer(_) => async {
                shim.cmd("move_cursor", json!({"x": 17, "y": 23})).await?;
                let pos = shim.cmd("get_cursor_position", json!({})).await?;
                let p = if pos["position"].is_object() { pos["position"].clone() } else { pos };
                Ok::<_, String>((p["x"].as_f64().unwrap_or(-1.0), p["y"].as_f64().unwrap_or(-1.0)))
            }
            .await,
            Target::OsWorld(_) => async {
                let o = shim.run("python3 -c \"import pyautogui;pyautogui.moveTo(17,23);p=pyautogui.position();print(p[0],p[1])\"").await?;
                let mut it = o.stdout.split_whitespace().filter_map(|v| v.parse::<f64>().ok());
                Ok((it.next().unwrap_or(-1.0), it.next().unwrap_or(-1.0)))
            }
            .await,
            Target::Mcp(_) => Err("cursor round trip not available over MCP".to_owned()),
        };
        checks.push(finish(
            loaded,
            match result {
                Ok((x, y)) => Check::new(
                    "input.cursor",
                    super::verdict((x - 17.0).abs() < 2.0 && (y - 23.0).abs() < 2.0),
                    format!("moved to (17, 23), reads back ({x}, {y})"),
                ),
                Err(e) => Check::new("input.cursor", Status::Fail, e),
            },
            &["feature:driver", "manifest:display"],
        ));
    } else {
        let mut skip = Check::new(
            "input.cursor",
            Status::Skip,
            "effectful checks need --effects virtual",
        )
        .skip_reason("effects_disabled");
        skip = finish(loaded, skip, &["feature:driver", "manifest:display"]);
        checks.push(skip);
    }

    let mut report = report_shell(target, loaded, checks, fidelity, &os);
    report.finalize(strict, started.elapsed());
    report
}

fn report_shell(
    target: &Target,
    loaded: &Loaded,
    checks: Vec<Check>,
    mut fidelity: Fidelity,
    os: &str,
) -> Report {
    fidelity.runtime = String::new();
    Report {
        schema_version: cua_spacesd_client::diagnose::SCHEMA_VERSION,
        producer: format!("cua-doctor-shim/{}", target.kind()),
        started_at: None,
        spacesd: None,
        image: Image {
            name: loaded.manifest.name.clone(),
            reference: loaded.manifest.reference.clone(),
            variant: loaded.manifest.variant.clone(),
            os: if os.is_empty() {
                loaded.manifest.os.clone()
            } else {
                os.to_owned()
            },
            manifest_sha256: loaded.sha256.clone(),
            manifest_source: loaded.source.clone(),
        },
        environment: Environment {
            os: os.to_owned(),
            ..Default::default()
        },
        summary: Default::default(),
        checks,
        fidelity,
    }
}

fn distinct_colors(img: &image::RgbImage) -> usize {
    let mut seen = std::collections::BTreeSet::new();
    for y in (0..img.height()).step_by(7) {
        for x in (0..img.width()).step_by(7) {
            seen.insert(img.get_pixel(x, y).0);
            if seen.len() > 4096 {
                return seen.len();
            }
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(
            Target::parse("computer-server=http://127.0.0.1:8000/").unwrap(),
            Target::ComputerServer("http://127.0.0.1:8000".into())
        );
        assert!(matches!(
            Target::parse("osworld=http://h:5000").unwrap(),
            Target::OsWorld(_)
        ));
        assert!(Target::parse("nope=http://x").is_err());
        assert!(Target::parse("mcp=ftp://x").is_err());
        assert!(Target::parse("mcp").is_err());
    }

    use axum::{
        Json, Router,
        extract::State,
        routing::{get, post},
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn png() -> Vec<u8> {
        let img = image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, 128])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// Interprets the few shell lines the shim sends (never executes anything).
    fn fake_shell(files: &Mutex<HashMap<String, String>>, line: &str) -> (i64, String) {
        if let Some(rest) = line.strip_prefix("CUA_DOCTOR=") {
            return (
                0,
                rest.split(' ').next().unwrap_or_default().to_owned() + "\n",
            );
        }
        if line == "sh -c 'exit 7'" {
            return (7, String::new());
        }
        if line.starts_with("python3 -c 'import time") {
            let ms = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            return (0, format!("{ms}\n"));
        }
        if line.starts_with("df -Pk") {
            return (0, format!("{}\n", 50u64 << 30));
        }
        if let Some(rest) = line.strip_prefix("printf '%s' '") {
            let (content, path) = rest.split_once("' > '").unwrap();
            files
                .lock()
                .unwrap()
                .insert(path.trim_end_matches('\'').to_owned(), content.to_owned());
            return (0, String::new());
        }
        if let Some(path) = line.strip_prefix("cat '") {
            return (
                0,
                files
                    .lock()
                    .unwrap()
                    .get(path.trim_end_matches('\''))
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        if line.starts_with("rm -f") || line.starts_with("systemctl is-active") {
            return (0, String::new());
        }
        (127, String::new())
    }

    #[derive(Default)]
    struct Fake {
        files: Mutex<HashMap<String, String>>,
        cursor: Mutex<(f64, f64)>,
    }

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    async fn computer_server() -> String {
        let fake = Arc::new(Fake::default());
        let app = Router::new()
            .route("/status", get(|| async { Json(json!({"status": "ok"})) }))
            .route(
                "/cmd",
                post(|State(f): State<Arc<Fake>>, Json(body): Json<Value>| async move {
                    let p = &body["params"];
                    let reply = match body["command"].as_str().unwrap_or("") {
                        "version" => json!({"success": true, "protocol": 1}),
                        "get_desktop_environment" => json!({"success": true, "environment": "xfce linux"}),
                        "get_screen_size" => json!({"success": true, "size": {"width": 64, "height": 48}}),
                        "screenshot" => json!({"success": true, "image_data": base64::engine::general_purpose::STANDARD.encode(png())}),
                        "run_command" => {
                            let (code, out) = fake_shell(&f.files, p["command"].as_str().unwrap_or(""));
                            json!({"success": true, "stdout": out, "stderr": "", "return_code": code})
                        }
                        "get_accessibility_tree" => json!({"success": true, "tree": {"role": "desktop", "children": [{"role": "window", "name": "x"}]}}),
                        "move_cursor" => {
                            *f.cursor.lock().unwrap() = (p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap());
                            json!({"success": true})
                        }
                        "get_cursor_position" => {
                            let (x, y) = *f.cursor.lock().unwrap();
                            json!({"success": true, "position": {"x": x, "y": y}})
                        }
                        other => json!({"success": false, "error": format!("Unknown command: {other}")}),
                    };
                    format!("data: {reply}\n\n")
                }),
            )
            .with_state(fake);
        serve(app).await
    }

    fn manifest() -> Loaded {
        Loaded::parse(
            br#"{"schema_version":1,"name":"fake","os":"linux","features_required":["a11y","driver"],"units":["cua-x"]}"#,
            "test",
        )
    }

    #[tokio::test]
    async fn computer_server_shim_reports_like_the_doctor() {
        let url = computer_server().await;
        let report = run(&Target::ComputerServer(url), &manifest(), true, true, None).await;
        let status = |id: &str| {
            report
                .checks
                .iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("{id}"))
                .status
        };
        for id in [
            "meta.shim.reachable",
            "annotations.os",
            "screenshot.display",
            "process.run",
            "process.exit_code",
            "files.roundtrip",
            "time.skew",
            "resources.disk.root",
            "init.units",
            "a11y.tree",
            "input.cursor",
        ] {
            assert_eq!(status(id), Status::Pass, "{id}: {}", report.to_human());
        }
        assert!(report.spacesd.is_none());
        assert_eq!(report.producer, "cua-doctor-shim/computer-server");
        assert_eq!(report.summary.status, Status::Pass, "{}", report.to_human());
        assert_eq!(report.fidelity.display, "64x48@1");
        // The same schema as cua-spacesd doctor writes.
        assert!(Report::from_json(&report.to_json()).is_ok());
    }

    #[tokio::test]
    async fn unreachable_targets_fail_fast() {
        let report = run(
            &Target::OsWorld("http://127.0.0.1:9".into()),
            &Loaded::default(),
            false,
            false,
            None,
        )
        .await;
        assert_eq!(report.summary.status, Status::Fail);
        assert_eq!(report.checks.len(), 1);
    }

    #[tokio::test]
    async fn osworld_shim_runs_commands_and_reads_the_screen() {
        let files = Arc::new(Mutex::new(HashMap::new()));
        let app = Router::new()
            .route("/platform", get(|| async { "Linux" }))
            .route("/screenshot", get(|| async { png() }))
            .route("/screen_size", post(|| async { Json(json!({"width": 64, "height": 48})) }))
            .route("/accessibility", get(|| async { Json(json!({"AT": "<desktop-frame><window name=\"x\"/></desktop-frame>"})) }))
            .route(
                "/execute",
                post(|State(files): State<Arc<Mutex<HashMap<String, String>>>>, Json(body): Json<Value>| async move {
                    let (code, out) = fake_shell(&files, body["command"].as_str().unwrap_or(""));
                    Json(json!({"status": if code == 0 { "success" } else { "error" }, "output": out, "error": "", "returncode": code}))
                }),
            )
            .with_state(files);
        let url = serve(app).await;
        let report = run(
            &Target::OsWorld(url),
            &Loaded::default(),
            false,
            false,
            None,
        )
        .await;
        let status = |id: &str| report.checks.iter().find(|c| c.id == id).unwrap().status;
        assert_eq!(status("process.run"), Status::Pass, "{}", report.to_human());
        assert_eq!(status("files.roundtrip"), Status::Pass);
        assert_eq!(status("screenshot.display"), Status::Pass);
        assert_eq!(status("a11y.tree"), Status::Pass);
        assert_eq!(status("input.cursor"), Status::Skip);
    }

    #[test]
    fn reads_sse_and_plain_json() {
        assert_eq!(sse_json("data: {\"a\":1}\n\n").unwrap()["a"], 1);
        assert_eq!(sse_json("{\"a\":2}").unwrap()["a"], 2);
        assert!(sse_json("nope").is_err());
    }
}
