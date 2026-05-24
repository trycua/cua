// cua-driver-uia: Windows uiAccess-elevated tool worker.
//
// Listens on \\.\pipe\cua-driver-uia for line-delimited JSON requests with the
// same shape as cua-driver's daemon pipe (\\.\pipe\cua-driver), so cua-driver's
// CLI and MCP server can prefer this worker on Windows for UIPI-blocked ops.
//
// Protocol (one JSON object per line, both directions):
//   request : {"method":"call","name":"<tool>","args":{...}}
//             {"method":"list"}
//             {"method":"describe","name":"<tool>"}
//             {"method":"shutdown"}
//   response: {"ok":true,"result":...}
//             {"ok":false,"error":"...","exit_code":N}
//
// The protocol is intentionally byte-identical to cua-driver/serve.rs so that
// the existing client code in cli.rs::run_call can talk to either pipe.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("cua-driver-uia: Windows-only. (this binary is a no-op on non-Windows hosts.)");
    std::process::exit(0);
}

#[cfg(target_os = "windows")]
use serde::{Deserialize, Serialize};

#[cfg(target_os = "windows")]
#[derive(Debug, Deserialize)]
struct PipeRequest {
    method: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    args: Option<serde_json::Value>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Serialize)]
struct PipeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

#[cfg(target_os = "windows")]
impl PipeResponse {
    fn ok(result: serde_json::Value) -> Self {
        Self { ok: true, result: Some(result), error: None, exit_code: None }
    }
    fn err(msg: impl Into<String>, code: i32) -> Self {
        Self { ok: false, result: None, error: Some(msg.into()), exit_code: Some(code) }
    }
}

#[cfg(target_os = "windows")]
const PIPE_PATH: &str = r"\\.\pipe\cua-driver-uia";

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}

#[cfg(target_os = "windows")]
async fn async_main() -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    let registry = std::sync::Arc::new(platform_windows::register_tools());
    let tool_count = registry.iter_defs().count();
    eprintln!("cua-driver-uia: {tool_count} tools registered; listening on {PIPE_PATH}");

    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(PIPE_PATH)
            .map_err(|e| anyhow::anyhow!("create named pipe {PIPE_PATH}: {e}"))?;

        server.connect().await
            .map_err(|e| anyhow::anyhow!("named pipe connect: {e}"))?;

        let reg = registry.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let req: PipeRequest = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = writer
                            .write_all(
                                (serde_json::to_string(&PipeResponse::err(
                                    format!("JSON parse error: {e}"), 65,
                                )).unwrap() + "\n")
                                .as_bytes(),
                            )
                            .await;
                        continue;
                    }
                };

                let resp = handle_request(&reg, req).await;
                let _ = writer
                    .write_all((serde_json::to_string(&resp).unwrap() + "\n").as_bytes())
                    .await;
            }
        });
    }
}

#[cfg(target_os = "windows")]
async fn handle_request(
    reg: &mcp_server::tool::ToolRegistry,
    req: PipeRequest,
) -> PipeResponse {
    match req.method.as_str() {
        "list" => {
            let tools: Vec<serde_json::Value> = reg.iter_defs()
                .map(|(name, def)| serde_json::json!({
                    "name": name,
                    "description": def.description,
                    "input_schema": def.input_schema,
                    "read_only": def.read_only,
                    "destructive": def.destructive,
                    "idempotent": def.idempotent,
                    "open_world": def.open_world,
                }))
                .collect();
            PipeResponse::ok(serde_json::json!({ "tools": tools }))
        }
        "describe" => {
            let name = req.name.as_deref().unwrap_or("");
            match reg.get_def(name) {
                Some(def) => PipeResponse::ok(serde_json::json!({
                    "name": def.name,
                    "description": def.description,
                    "input_schema": def.input_schema,
                })),
                None => PipeResponse::err(format!("Unknown tool: {name}"), 64),
            }
        }
        "call" => {
            let raw = req.name.as_deref().unwrap_or("").to_owned();
            let tool_name = if raw == "type_text_chars" { "type_text".to_owned() } else { raw };
            let args = req.args.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            if reg.get_def(&tool_name).is_none() {
                return PipeResponse::err(format!("Unknown tool: {tool_name}"), 64);
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
            if is_err {
                let msg = result.content.iter()
                    .filter_map(|c| if let mcp_server::protocol::Content::Text { text, .. } = c {
                        Some(text.as_str())
                    } else { None })
                    .collect::<Vec<_>>().join("\n");
                PipeResponse::err(msg, 1)
            } else {
                PipeResponse::ok(result_obj)
            }
        }
        "shutdown" => {
            // Worker shutdown is unsupported in the prototype — restarting requires
            // ShellExecute which the main daemon doesn't have a clean path to. Treat
            // as a no-op for now; the supervising launcher can taskkill the process.
            PipeResponse::ok(serde_json::json!({"shutdown": false, "reason": "uia worker ignores shutdown"}))
        }
        other => PipeResponse::err(format!("Unknown method: {other}"), 65),
    }
}
