//! Hidden process-isolated SDK runtime entry point.
//!
//! This is intentionally not a CLI product surface. A trusted SDK host starts
//! it directly and owns the inherited stdin/stdout channel for the lifetime of
//! one runtime generation.

use cua_driver_sdk::worker::{
    ActionCompletion, ChannelRequest, ChannelResponse, WorkerInitialization,
    PRIVATE_WORKER_PROTOCOL_VERSION,
};
use cua_driver_sdk::{CuaDriver, CuaDriverSession, DriverHostOptions};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;

struct InitializationSignal(Option<std::sync::mpsc::SyncSender<bool>>);

impl InitializationSignal {
    fn ready(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(true);
        }
    }
}

impl Drop for InitializationSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(false);
        }
    }
}

pub fn requested_generation() -> Option<String> {
    let mut args = std::env::args();
    let _executable = args.next();
    if args.next().as_deref() != Some("__private-worker") {
        return None;
    }
    match (args.next().as_deref(), args.next()) {
        (Some("--generation"), Some(generation))
            if generation.len() <= 128
                && !generation.is_empty()
                && generation
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-') =>
        {
            Some(generation)
        }
        _ => Some(String::new()),
    }
}

pub fn run(
    generation: String,
    initialized: Option<std::sync::mpsc::SyncSender<bool>>,
) -> anyhow::Result<()> {
    if generation.is_empty() {
        anyhow::bail!("private worker requires one valid --generation value");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_async(generation, initialized))
}

async fn run_async(
    generation: String,
    initialized: Option<std::sync::mpsc::SyncSender<bool>>,
) -> anyhow::Result<()> {
    let mut initialized = InitializationSignal(initialized);
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    let Some(first_line) = lines.next() else {
        return Ok(());
    };
    let first_line = first_line?;
    let initialization_request: ChannelRequest = serde_json::from_str(&first_line)?;
    if initialization_request.protocol_version != PRIVATE_WORKER_PROTOCOL_VERSION
        || initialization_request.request_id != 0
        || initialization_request.generation != generation
        || initialization_request.operation != "initialize"
    {
        write_response(
            &mut writer,
            &ChannelResponse::error(
                initialization_request.request_id,
                &generation,
                "invalid_initialization",
                "private worker initialization identity mismatch",
                ActionCompletion::NotStarted,
            ),
        )?;
        return Ok(());
    }
    let initialization: WorkerInitialization = serde_json::from_value(
        initialization_request
            .arguments
            .ok_or_else(|| anyhow::anyhow!("private worker initialization omitted options"))?,
    )?;
    if initialization.host_bundle_id.trim().is_empty() {
        write_response(
            &mut writer,
            &ChannelResponse::error(
                0,
                &generation,
                "invalid_initialization",
                "private worker host identity is empty",
                ActionCompletion::NotStarted,
            ),
        )?;
        return Ok(());
    }

    let driver = match CuaDriver::try_create_configured_for_host(
        initialization.configured_driver,
        DriverHostOptions {
            cursor: cursor_overlay::CursorConfig::default(),
            claude_code_compatibility: false,
            prepare_desktop_environment: true,
            register_host_tools: Some(crate::check_update_tool::register_into),
        },
    ) {
        Ok(driver) => driver,
        Err(error) => {
            write_response(
                &mut writer,
                &ChannelResponse::error(
                    0,
                    &generation,
                    "runtime_initialization_failed",
                    error.to_string(),
                    ActionCompletion::NotStarted,
                ),
            )?;
            return Ok(());
        }
    };
    initialized.ready();
    let metadata = driver.metadata().await?;
    write_response(
        &mut writer,
        &ChannelResponse::ok(
            0,
            &generation,
            serde_json::json!({
                "ready": true,
                "pid": std::process::id(),
                "host_bundle_id": initialization.host_bundle_id,
                "metadata": metadata,
            }),
        ),
    )?;

    let mut sessions: HashMap<String, Arc<CuaDriverSession>> = HashMap::new();
    while let Some(line) = lines.next() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let request: ChannelRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut writer,
                    &ChannelResponse::error(
                        0,
                        &generation,
                        "invalid_request",
                        format!("parse private worker request: {error}"),
                        ActionCompletion::NotStarted,
                    ),
                )?;
                continue;
            }
        };
        if request.protocol_version != PRIVATE_WORKER_PROTOCOL_VERSION
            || request.generation != generation
        {
            write_response(
                &mut writer,
                &ChannelResponse::error(
                    request.request_id,
                    &generation,
                    "generation_mismatch",
                    "private worker request belongs to another runtime generation",
                    ActionCompletion::NotStarted,
                ),
            )?;
            continue;
        }

        let response = handle_request(&driver, &mut sessions, &generation, request).await;
        let shutdown = response
            .result
            .as_ref()
            .and_then(|value| value.get("shutdown"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        write_response(&mut writer, &response)?;
        if shutdown {
            break;
        }
    }

    sessions.clear();
    driver.shutdown().await?;
    Ok(())
}

async fn handle_request(
    driver: &Arc<CuaDriver>,
    sessions: &mut HashMap<String, Arc<CuaDriverSession>>,
    generation: &str,
    request: ChannelRequest,
) -> ChannelResponse {
    let request_id = request.request_id;
    let result: Result<Value, String> = match request.operation.as_str() {
        "metadata" => driver
            .metadata()
            .await
            .and_then(|metadata| {
                serde_json::to_value(metadata).map_err(|error| {
                    cua_driver_sdk::DriverError::Protocol {
                        reason: error.to_string(),
                    }
                })
            })
            .map_err(|error| error.to_string()),
        "list" => driver
            .list_tools_json()
            .await
            .map_err(|error| error.to_string())
            .and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string())),
        "call" => {
            let name = request.name.as_deref().unwrap_or("");
            let arguments = request
                .arguments
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
            let invocation = if let Some(handle) = request.session_handle.as_deref() {
                match sessions.get(handle) {
                    Some(session) => {
                        session
                            .call_tool(name.to_owned(), arguments.to_string())
                            .await
                    }
                    None => {
                        return ChannelResponse::error(
                            request_id,
                            generation,
                            "session_not_bound",
                            "private worker session handle is not live on this channel",
                            ActionCompletion::NotStarted,
                        );
                    }
                }
            } else {
                driver.call_tool_from_trusted_adapter(name, arguments).await
            };
            invocation
                .map_err(|error| error.to_string())
                .and_then(|result| {
                    serde_json::from_str(&result.raw_json).map_err(|error| error.to_string())
                })
        }
        "bind_session" => {
            let options = request
                .arguments
                .ok_or_else(|| "bind_session omitted options".to_owned())
                .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()));
            match options {
                Ok(options) => match driver.create_trusted_session(options) {
                    Ok(session) => {
                        let handle = uuid::Uuid::new_v4().to_string();
                        sessions.insert(handle.clone(), session);
                        Ok(serde_json::json!({"session_handle": handle}))
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error),
            }
        }
        "close_session" => {
            let Some(handle) = request.session_handle.as_deref() else {
                return ChannelResponse::error(
                    request_id,
                    generation,
                    "invalid_request",
                    "close_session omitted session_handle",
                    ActionCompletion::NotStarted,
                );
            };
            if let Some(session) = sessions.remove(handle) {
                session.close();
            }
            Ok(serde_json::json!({"closed": true}))
        }
        "shutdown" => {
            sessions.clear();
            match driver.shutdown().await {
                Ok(()) => Ok(serde_json::json!({"shutdown": true})),
                Err(error) => Err(error.to_string()),
            }
        }
        other => Err(format!("unknown private worker operation: {other}")),
    };

    match result {
        Ok(value) => ChannelResponse::ok(request_id, generation, value),
        Err(error) => ChannelResponse::error(
            request_id,
            generation,
            "worker_request_failed",
            error,
            ActionCompletion::Completed,
        ),
    }
}

fn write_response(writer: &mut impl Write, response: &ChannelResponse) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::requested_generation;

    #[test]
    fn ordinary_process_is_not_a_private_worker() {
        assert!(requested_generation().is_none());
    }
}
