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
        Self {
            ok: true,
            result: Some(result),
            error: None,
            exit_code: None,
        }
    }
    fn err(msg: impl Into<String>, code: i32) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(msg.into()),
            exit_code: Some(code),
        }
    }
}

#[cfg(target_os = "windows")]
fn pipe_path() -> &'static str {
    let is_local = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(str::to_owned))
        .is_some_and(|name| name.eq_ignore_ascii_case("cua-driver-uia-local.exe"));
    if is_local {
        r"\\.\pipe\cua-driver-local-uia"
    } else {
        r"\\.\pipe\cua-driver-uia"
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct SecurityAttributesRaw {
    n_length: u32,
    lp_security_descriptor: *mut std::ffi::c_void,
    b_inherit_handle: i32,
}

#[cfg(target_os = "windows")]
unsafe fn security_attrs_from_sddl(
    sddl: &str,
) -> Option<(SecurityAttributesRaw, *mut std::ffi::c_void)> {
    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut std::ffi::c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
    }

    let sddl: Vec<u16> = format!("{sddl}\0").encode_utf16().collect();
    let mut descriptor = std::ptr::null_mut();
    let mut descriptor_size = 0_u32;
    let ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl.as_ptr(),
        1,
        &mut descriptor,
        &mut descriptor_size,
    );
    if ok == 0 || descriptor.is_null() {
        return None;
    }
    Some((
        SecurityAttributesRaw {
            n_length: std::mem::size_of::<SecurityAttributesRaw>() as u32,
            lp_security_descriptor: descriptor,
            b_inherit_handle: 0,
        },
        descriptor,
    ))
}

#[cfg(target_os = "windows")]
unsafe fn token_user_sid(token: *mut std::ffi::c_void) -> Option<String> {
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut std::ffi::c_void,
        attributes: u32,
    }
    #[repr(C)]
    struct TokenUserRaw {
        user: SidAndAttributes,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn GetTokenInformation(
            token: *mut std::ffi::c_void,
            information_class: u32,
            information: *mut std::ffi::c_void,
            information_length: u32,
            return_length: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *mut std::ffi::c_void, string_sid: *mut *mut u16) -> i32;
    }

    const TOKEN_USER_CLASS: u32 = 1;
    let mut required = 0_u32;
    let _ = GetTokenInformation(
        token,
        TOKEN_USER_CLASS,
        std::ptr::null_mut(),
        0,
        &mut required,
    );
    if required == 0 {
        let _ = CloseHandle(token);
        return None;
    }
    let mut buffer = vec![0_u8; required as usize];
    let ok = GetTokenInformation(
        token,
        TOKEN_USER_CLASS,
        buffer.as_mut_ptr().cast(),
        required,
        &mut required,
    );
    let _ = CloseHandle(token);
    if ok == 0 {
        return None;
    }
    let token_user = std::ptr::read_unaligned(buffer.as_ptr().cast::<TokenUserRaw>());
    let mut string_sid = std::ptr::null_mut();
    if ConvertSidToStringSidW(token_user.user.sid, &mut string_sid) == 0 || string_sid.is_null() {
        return None;
    }
    let length = (0..)
        .find(|&index| *string_sid.add(index) == 0)
        .unwrap_or(0);
    let sid = String::from_utf16_lossy(std::slice::from_raw_parts(string_sid, length));
    let _ = LocalFree(string_sid.cast());
    (!sid.is_empty()).then_some(sid)
}

#[cfg(target_os = "windows")]
unsafe fn current_user_sid_string() -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(
            process: *mut std::ffi::c_void,
            desired_access: u32,
            token: *mut *mut std::ffi::c_void,
        ) -> i32;
    }

    const TOKEN_QUERY: u32 = 0x0008;
    let mut token = std::ptr::null_mut();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 || token.is_null() {
        return None;
    }
    token_user_sid(token)
}

#[cfg(target_os = "windows")]
unsafe fn named_pipe_client_sid(pipe: *mut std::ffi::c_void) -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetNamedPipeClientProcessId(
            pipe: *mut std::ffi::c_void,
            client_process_id: *mut u32,
        ) -> i32;
        fn OpenProcess(
            desired_access: u32,
            inherit_handle: i32,
            process_id: u32,
        ) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(
            process: *mut std::ffi::c_void,
            desired_access: u32,
            token: *mut *mut std::ffi::c_void,
        ) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const TOKEN_QUERY: u32 = 0x0008;
    let mut client_process_id = 0_u32;
    if GetNamedPipeClientProcessId(pipe, &mut client_process_id) == 0 || client_process_id == 0 {
        return None;
    }
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, client_process_id);
    if process.is_null() {
        return None;
    }
    let mut token = std::ptr::null_mut();
    let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
    let _ = CloseHandle(process);
    if opened == 0 || token.is_null() {
        return None;
    }
    token_user_sid(token)
}

#[cfg(target_os = "windows")]
fn current_user_pipe_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})S:(ML;;NW;;;LW)")
}

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
    use std::os::windows::io::AsRawHandle as _;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    let registry = std::sync::Arc::new(platform_windows::register_tools());
    let tool_count = registry.iter_defs().count();
    let pipe_path = pipe_path();
    eprintln!("cua-driver-uia: {tool_count} tools registered; listening on {pipe_path}");

    let owner_sid = unsafe { current_user_sid_string() }
        .ok_or_else(|| anyhow::anyhow!("resolve current Windows user SID"))?;
    let pipe_security = unsafe { security_attrs_from_sddl(&current_user_pipe_sddl(&owner_sid)) }
        .ok_or_else(|| anyhow::anyhow!("build current-user UIAccess pipe security descriptor"))?;
    // Hold the descriptor for the worker lifetime. Windows reclaims it at
    // process exit; every pipe instance receives the same current-user ACL.
    let security_attributes = &pipe_security.0 as *const _ as *mut std::ffi::c_void;

    loop {
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(false)
                .create_with_security_attributes_raw(pipe_path, security_attributes)
                .map_err(|e| anyhow::anyhow!("create named pipe {pipe_path}: {e}"))?
        };

        server
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("named pipe connect: {e}"))?;

        let client_sid =
            unsafe { named_pipe_client_sid(server.as_raw_handle().cast::<std::ffi::c_void>()) };
        if client_sid.as_deref() != Some(owner_sid.as_str()) {
            tracing::warn!("UIAccess named-pipe connection rejected before request parsing");
            let _ = server.disconnect();
            continue;
        }

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
                                    format!("JSON parse error: {e}"),
                                    65,
                                ))
                                .unwrap()
                                    + "\n")
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
    reg: &cua_driver_core::tool::ToolRegistry,
    req: PipeRequest,
) -> PipeResponse {
    match req.method.as_str() {
        "list" => {
            let tools: Vec<serde_json::Value> = reg
                .iter_defs()
                .map(|(name, def)| {
                    serde_json::json!({
                        "name": name,
                        "description": def.description,
                        "input_schema": def.input_schema,
                        "read_only": def.read_only,
                        "destructive": def.destructive,
                        "idempotent": def.idempotent,
                        "open_world": def.open_world,
                    })
                })
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
            let tool_name = if raw == "type_text_chars" {
                "type_text".to_owned()
            } else {
                raw
            };
            let args = req
                .args
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            if reg.get_def(&tool_name).is_none() {
                return PipeResponse::err(format!("Unknown tool: {tool_name}"), 64);
            }
            let result = reg.invoke(&tool_name, args).await;
            let is_err = result.is_error.unwrap_or(false);
            let content: Vec<serde_json::Value> = result
                .content
                .iter()
                .map(|c| match c {
                    cua_driver_core::protocol::Content::Text { text, .. } => {
                        serde_json::json!({"type":"text","text":text})
                    }
                    cua_driver_core::protocol::Content::Image {
                        data, mime_type, ..
                    } => serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
                })
                .collect();
            let mut result_obj = serde_json::json!({"content": content, "isError": is_err});
            if let Some(sc) = result.structured_content {
                result_obj["structuredContent"] = sc;
            }
            if is_err {
                let msg = result
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let cua_driver_core::protocol::Content::Text { text, .. } = c {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                PipeResponse::err(msg, 1)
            } else {
                PipeResponse::ok(result_obj)
            }
        }
        "shutdown" => {
            // Worker shutdown is unsupported in the prototype — restarting requires
            // ShellExecute which the main daemon doesn't have a clean path to. Treat
            // as a no-op for now; the supervising launcher can taskkill the process.
            PipeResponse::ok(
                serde_json::json!({"shutdown": false, "reason": "uia worker ignores shutdown"}),
            )
        }
        other => PipeResponse::err(format!("Unknown method: {other}"), 65),
    }
}
