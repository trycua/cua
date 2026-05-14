//! Unix-socket daemon server and client for `cua-driver serve`/`stop`/`status`.
//!
//! Protocol: line-delimited JSON over a Unix domain socket.
//!
//! Request shapes:
//!   {"method":"call","name":"<tool>","args":{...}}
//!   {"method":"list"}
//!   {"method":"describe","name":"<tool>"}
//!   {"method":"shutdown"}
//!
//! Response shapes:
//!   {"ok":true,"result":...}
//!   {"ok":false,"error":"...","exit_code":1}
//!
//! The socket file is at:
//!   macOS  — ~/Library/Caches/cua-driver/cua-driver.sock
//!   Linux  — ~/.cache/cua-driver/cua-driver.sock
//!   Windows — \\.\pipe\cua-driver  (TODO: use named pipe; stubs only for now)

use serde::{Deserialize, Serialize};

// ── Paths ─────────────────────────────────────────────────────────────────────

/// Returns the platform default socket/pipe path.
pub fn default_socket_path() -> String {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/Library/Caches/cua-driver/cua-driver.sock")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.cache/cua-driver/cua-driver.sock")
    }
    #[cfg(target_os = "windows")]
    {
        r"\\.\pipe\cua-driver".to_owned()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "/tmp/cua-driver.sock".to_owned()
    }
}

/// Returns the platform default PID file path.
pub fn default_pid_file_path() -> String {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/Library/Caches/cua-driver/cua-driver.pid")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.cache/cua-driver/cua-driver.pid")
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:/Temp".into());
        format!("{local}/cua-driver/cua-driver.pid")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "/tmp/cua-driver.pid".to_owned()
    }
}

// ── Protocol types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl DaemonResponse {
    pub fn ok(result: serde_json::Value) -> Self {
        Self { ok: true, result: Some(result), error: None, exit_code: None }
    }
    pub fn err(msg: impl Into<String>, code: i32) -> Self {
        Self { ok: false, result: None, error: Some(msg.into()), exit_code: Some(code) }
    }
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Probe whether a daemon is listening on `socket_path` by sending a
/// trivial `list` request and checking for a valid response.
pub fn is_daemon_listening(socket_path: &str) -> bool {
    let req = DaemonRequest { method: "list".into(), name: None, args: None };
    send_request(socket_path, &req)
        .ok()
        .map(|r| r.ok)
        .unwrap_or(false)
}

/// Read the PID stored in `pid_file_path`, if any.
pub fn read_pid_file(pid_file_path: &str) -> Option<u32> {
    std::fs::read_to_string(pid_file_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Send a request to the daemon and return the response.
/// Uses a 3-second connect timeout (by polling) and a 10-second read timeout.
#[cfg(unix)]
pub fn send_request(socket_path: &str, req: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let stream = UnixStream::connect(socket_path)
        .map_err(|e| anyhow::anyhow!("connect to {socket_path}: {e}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;

    let mut w = stream.try_clone()?;
    let line = serde_json::to_string(req)? + "\n";
    w.write_all(line.as_bytes())?;
    w.flush()?;

    let reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader.lines().next()
        .ok_or_else(|| anyhow::anyhow!("daemon closed connection without response"))??
        .clone_into(&mut resp_line);
    let resp: DaemonResponse = serde_json::from_str(&resp_line)?;
    Ok(resp)
}

#[cfg(not(unix))]
pub fn send_request(socket_path: &str, req: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    #[cfg(target_os = "windows")]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::time::Duration;

        // Retry opening the pipe — server may still be starting.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(socket_path)
            {
                Ok(f) => break f,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => anyhow::bail!("connect to named pipe {socket_path}: {e}"),
            }
        };

        let mut writer = pipe.try_clone()?;
        let line = serde_json::to_string(req)? + "\n";
        writer.write_all(line.as_bytes())?;
        writer.flush()?;

        // Server writes one JSON line then flushes; pipe stays open until we drop it.
        let reader = BufReader::new(pipe);
        let resp_line = reader
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("daemon closed connection without response"))??;
        let resp: DaemonResponse = serde_json::from_str(&resp_line)?;
        Ok(resp)
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("daemon client not supported on this platform (socket path: {socket_path})");
    }
}

// ── Server ────────────────────────────────────────────────────────────────────

/// Run the daemon server. Binds `socket_path`, writes `pid_file_path`,
/// accepts connections, and serves requests until `{"method":"shutdown"}`.
///
/// This is `async` and must be called from a tokio runtime.
#[cfg(unix)]
pub async fn run_serve(
    registry: std::sync::Arc<mcp_server::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    // Create parent directory.
    if let Some(dir) = std::path::Path::new(socket_path).parent() {
        std::fs::create_dir_all(dir)?;
    }

    // Remove stale socket file (from a crashed previous daemon).
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)
        .map_err(|e| anyhow::anyhow!("bind {socket_path}: {e}"))?;

    eprintln!("cua-driver daemon listening on {socket_path}");

    // Write PID file.
    if let Some(pid_path) = pid_file_path {
        if let Some(dir) = std::path::Path::new(pid_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(pid_path, std::process::id().to_string());
    }

    // Shutdown channel.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let reg = registry.clone();
                let shutdown_tx2 = shutdown_tx.clone();

                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut lines = BufReader::new(reader).lines();

                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: DaemonRequest = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                let resp = DaemonResponse::err(
                                    format!("JSON parse error: {e}"), 65
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                continue;
                            }
                        };

                        match req.method.as_str() {
                            "shutdown" => {
                                let resp = DaemonResponse::ok(serde_json::json!({"shutdown": true}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                let mut guard = shutdown_tx2.lock().await;
                                if let Some(tx) = guard.take() {
                                    let _ = tx.send(());
                                }
                                return;
                            }
                            "list" => {
                                let tools: Vec<serde_json::Value> = reg.iter_defs()
                                    .map(|(name, def)| serde_json::json!({
                                        "name": name,
                                        "description": def.description
                                    }))
                                    .collect();
                                let resp = DaemonResponse::ok(serde_json::json!({"tools": tools}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "describe" => {
                                let name = req.name.as_deref().unwrap_or("");
                                match reg.get_def(name) {
                                    Some(def) => {
                                        let resp = DaemonResponse::ok(serde_json::json!({
                                            "name": def.name,
                                            "description": def.description,
                                            "input_schema": def.input_schema
                                        }));
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                    }
                                    None => {
                                        let resp = DaemonResponse::err(
                                            format!("Unknown tool: {name}"), 64
                                        );
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                    }
                                }
                            }
                            "call" => {
                                let raw_name = req.name.as_deref().unwrap_or("").to_owned();
                                // Deprecated alias: `type_text_chars` → `type_text`.
                                // Mirrors Swift's `ToolRegistry.call` aliasing.
                                let tool_name = if raw_name == "type_text_chars" {
                                    eprintln!("[cua-driver-rs] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                                    "type_text".to_owned()
                                } else { raw_name.clone() };
                                let args = req.args.unwrap_or(serde_json::Value::Object(
                                    serde_json::Map::new()
                                ));
                                if reg.get_def(&tool_name).is_none() {
                                    let resp = DaemonResponse::err(
                                        format!("Unknown tool: {tool_name}"), 64
                                    );
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let result = reg.invoke(&tool_name, args).await;
                                let is_err = result.is_error.unwrap_or(false);
                                let content: Vec<serde_json::Value> = result.content.iter().map(|c| {
                                    match c {
                                        mcp_server::protocol::Content::Text { text, .. } =>
                                            serde_json::json!({"type":"text","text":text}),
                                        mcp_server::protocol::Content::Image { data, mime_type, .. } =>
                                            serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
                                    }
                                }).collect();
                                let mut result_obj = serde_json::json!({
                                    "content": content,
                                    "isError": is_err
                                });
                                if let Some(sc) = result.structured_content {
                                    result_obj["structuredContent"] = sc;
                                }
                                let resp = if is_err {
                                    DaemonResponse::err(
                                        result.content.iter()
                                            .filter_map(|c| if let mcp_server::protocol::Content::Text { text, .. } = c { Some(text.as_str()) } else { None })
                                            .collect::<Vec<_>>()
                                            .join("\n"),
                                        1
                                    )
                                } else {
                                    DaemonResponse::ok(result_obj)
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            other => {
                                let resp = DaemonResponse::err(
                                    format!("Unknown method: {other}"), 65
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                        }
                    }
                });
            }
            _ = &mut shutdown_rx => {
                eprintln!("cua-driver daemon shutting down.");
                break;
            }
        }
    }

    // Clean up.
    let _ = std::fs::remove_file(socket_path);
    if let Some(pid_path) = pid_file_path {
        let _ = std::fs::remove_file(pid_path);
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub async fn run_serve(
    registry: std::sync::Arc<mcp_server::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    eprintln!("cua-driver daemon listening on {socket_path}");

    // Write PID file.
    if let Some(pid_path) = pid_file_path {
        if let Some(dir) = std::path::Path::new(pid_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(pid_path, std::process::id().to_string());
    }

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));

    loop {
        // Create a new pipe server instance to accept the next client.
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(socket_path)
            .map_err(|e| anyhow::anyhow!("create named pipe {socket_path}: {e}"))?;

        tokio::select! {
            result = server.connect() => {
                result.map_err(|e| anyhow::anyhow!("named pipe connect: {e}"))?;

                let reg = registry.clone();
                let shutdown_tx2 = shutdown_tx.clone();

                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();

                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: DaemonRequest = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                let resp = DaemonResponse::err(format!("JSON parse error: {e}"), 65);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                continue;
                            }
                        };

                        match req.method.as_str() {
                            "shutdown" => {
                                let resp = DaemonResponse::ok(serde_json::json!({"shutdown": true}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                let mut guard = shutdown_tx2.lock().await;
                                if let Some(tx) = guard.take() { let _ = tx.send(()); }
                                return;
                            }
                            "list" => {
                                let tools: Vec<serde_json::Value> = reg.iter_defs()
                                    .map(|(name, def)| serde_json::json!({"name": name, "description": def.description}))
                                    .collect();
                                let resp = DaemonResponse::ok(serde_json::json!({"tools": tools}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "describe" => {
                                let name = req.name.as_deref().unwrap_or("");
                                let resp = match reg.get_def(name) {
                                    Some(def) => DaemonResponse::ok(serde_json::json!({
                                        "name": def.name, "description": def.description,
                                        "input_schema": def.input_schema
                                    })),
                                    None => DaemonResponse::err(format!("Unknown tool: {name}"), 64),
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "call" => {
                                let raw_name = req.name.as_deref().unwrap_or("").to_owned();
                                let tool_name = if raw_name == "type_text_chars" {
                                    eprintln!("[cua-driver-rs] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                                    "type_text".to_owned()
                                } else { raw_name.clone() };
                                let args = req.args.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                if reg.get_def(&tool_name).is_none() {
                                    let resp = DaemonResponse::err(format!("Unknown tool: {tool_name}"), 64);
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let result = reg.invoke(&tool_name, args).await;
                                let is_err = result.is_error.unwrap_or(false);
                                let content: Vec<serde_json::Value> = result.content.iter().map(|c| match c {
                                    mcp_server::protocol::Content::Text { text, .. } =>
                                        serde_json::json!({"type":"text","text":text}),
                                    mcp_server::protocol::Content::Image { data, mime_type, .. } =>
                                        serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
                                }).collect();
                                let mut result_obj = serde_json::json!({"content": content, "isError": is_err});
                                if let Some(sc) = result.structured_content {
                                    result_obj["structuredContent"] = sc;
                                }
                                let resp = if is_err {
                                    DaemonResponse::err(
                                        result.content.iter()
                                            .filter_map(|c| if let mcp_server::protocol::Content::Text { text, .. } = c { Some(text.as_str()) } else { None })
                                            .collect::<Vec<_>>().join("\n"),
                                        1
                                    )
                                } else {
                                    DaemonResponse::ok(result_obj)
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            other => {
                                let resp = DaemonResponse::err(format!("Unknown method: {other}"), 65);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                        }
                    }
                });
            }
            _ = &mut shutdown_rx => {
                eprintln!("cua-driver daemon shutting down.");
                break;
            }
        }
    }

    if let Some(pid_path) = pid_file_path {
        let _ = std::fs::remove_file(pid_path);
    }
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
pub async fn run_serve(
    _registry: std::sync::Arc<mcp_server::tool::ToolRegistry>,
    _socket_path: &str,
    _pid_file_path: Option<&str>,
) -> anyhow::Result<()> {
    anyhow::bail!("cua-driver serve is not supported on this platform");
}

// ── CLI helpers ───────────────────────────────────────────────────────────────

/// `cua-driver serve` implementation.
pub fn run_serve_cmd(
    registry: std::sync::Arc<mcp_server::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
) {
    let socket_path = socket_path.to_owned();
    let pid_file_path = pid_file_path.map(str::to_owned);

    // Fail fast if another daemon is already running.
    if is_daemon_listening(&socket_path) {
        let pid_hint = pid_file_path.as_deref()
            .and_then(|p| read_pid_file(p))
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default();
        eprintln!(
            "cua-driver daemon is already running on {socket_path}{pid_hint}. \
             Run `cua-driver stop` first."
        );
        std::process::exit(1);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    if let Err(e) = rt.block_on(run_serve(
        registry,
        &socket_path,
        pid_file_path.as_deref(),
    )) {
        eprintln!("cua-driver serve error: {e}");
        std::process::exit(1);
    }
}

/// `cua-driver stop` implementation.
pub fn run_stop_cmd(socket_path: &str) {
    if !is_daemon_listening(socket_path) {
        eprintln!("cua-driver daemon is not running");
        std::process::exit(1);
    }

    let req = DaemonRequest { method: "shutdown".into(), name: None, args: None };
    match send_request(socket_path, &req) {
        Ok(_) => {
            // Poll until daemon stops responding (up to 2 seconds).
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                // On Windows the pipe path isn't a filesystem file; probe liveness instead.
                let gone = if std::path::Path::new(socket_path).exists() {
                    false
                } else {
                    !is_daemon_listening(socket_path)
                };
                if gone {
                    // Swift's `stop` exits silently on success (no stdout
                    // line) — match that for byte-for-byte parity.
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    eprintln!("cua-driver daemon did not release socket within 2s");
                    std::process::exit(1);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Err(e) => {
            eprintln!("stop: {e}");
            std::process::exit(1);
        }
    }
}

/// `cua-driver status` implementation.
pub fn run_status_cmd(socket_path: &str, pid_file_path: &str) {
    if is_daemon_listening(socket_path) {
        println!("cua-driver daemon is running");
        println!("  socket: {socket_path}");
        if let Some(pid) = read_pid_file(pid_file_path) {
            println!("  pid: {pid}");
        } else {
            println!("  pid: unknown (no pid file)");
        }
    } else {
        eprintln!("cua-driver daemon is not running");
        std::process::exit(1);
    }
}
