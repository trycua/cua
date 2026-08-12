//! Private supervised-worker transport for the typed SDK.
//!
//! The worker owns one runtime in a directly spawned child process. Its only
//! control surface is the child's inherited stdin/stdout pair: there is no
//! socket path, listener, discovery, reconnect, or reattachment protocol.

use crate::{
    embedded::{allowed_environment_name, safe_environment},
    ConfiguredDriverOptions, DriverError, DriverMetadata, EmbeddedEnvironmentVariable,
    TrustedSessionOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub const PRIVATE_WORKER_PROTOCOL_VERSION: u32 = 1;
const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_SHUTDOWN_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionCompletion {
    NotStarted,
    Completed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub protocol_version: u32,
    pub request_id: u64,
    pub generation: String,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelResponse {
    pub protocol_version: u32,
    pub request_id: u64,
    pub generation: String,
    pub ok: bool,
    pub completion: ActionCompletion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl ChannelResponse {
    pub fn ok(request_id: u64, generation: impl Into<String>, result: Value) -> ChannelResponse {
        ChannelResponse {
            protocol_version: PRIVATE_WORKER_PROTOCOL_VERSION,
            request_id,
            generation: generation.into(),
            ok: true,
            completion: ActionCompletion::Completed,
            result: Some(result),
            error: None,
            error_code: None,
        }
    }

    pub fn error(
        request_id: u64,
        generation: impl Into<String>,
        error_code: impl Into<String>,
        error: impl Into<String>,
        completion: ActionCompletion,
    ) -> ChannelResponse {
        ChannelResponse {
            protocol_version: PRIVATE_WORKER_PROTOCOL_VERSION,
            request_id,
            generation: generation.into(),
            ok: false,
            completion,
            result: None,
            error: Some(error.into()),
            error_code: Some(error_code.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInitialization {
    pub configured_driver: ConfiguredDriverOptions,
    pub host_bundle_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedWorkerOptions {
    pub binary_path: String,
    pub host_bundle_id: String,
    pub startup_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub configured_driver: ConfiguredDriverOptions,
    pub environment: Vec<EmbeddedEnvironmentVariable>,
    pub inherit_stderr: bool,
}

struct WorkerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_request_id: u64,
    stopped: bool,
}

impl WorkerProcess {
    fn stop_and_reap(&mut self) {
        self.stdin.take();
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        self.stopped = true;
    }
}

pub(crate) struct PrivateWorkerClient {
    generation: String,
    process: Mutex<WorkerProcess>,
    shutdown_timeout: Duration,
}

impl PrivateWorkerClient {
    pub(crate) fn spawn(options: ValidatedWorkerOptions) -> Result<Arc<Self>, DriverError> {
        let generation = Uuid::new_v4().to_string();
        let mut command = Command::new(&options.binary_path);
        command
            .arg("__private-worker")
            .arg("--generation")
            .arg(&generation)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if options.inherit_stderr {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .env_clear();
        for variable in safe_environment(&options.environment) {
            command.env(variable.name, variable.value);
        }

        let mut child = command.spawn().map_err(|error| DriverError::Worker {
            reason: format!("spawn private worker {}: {error}", options.binary_path),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| DriverError::Worker {
            reason: "private worker stdin was not piped".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| DriverError::Worker {
            reason: "private worker stdout was not piped".into(),
        })?;
        let client = Arc::new(Self {
            generation,
            process: Mutex::new(WorkerProcess {
                child,
                stdin: Some(stdin),
                stdout: Some(BufReader::new(stdout)),
                next_request_id: 1,
                stopped: false,
            }),
            shutdown_timeout: options.shutdown_timeout,
        });

        let host_bundle_id = options.host_bundle_id.clone();
        let initialization = serde_json::to_value(WorkerInitialization {
            configured_driver: options.configured_driver,
            host_bundle_id: options.host_bundle_id,
        })
        .map_err(|error| DriverError::Protocol {
            reason: format!("serialize private worker initialization: {error}"),
        })?;
        let request = ChannelRequest {
            protocol_version: PRIVATE_WORKER_PROTOCOL_VERSION,
            request_id: 0,
            generation: client.generation.clone(),
            operation: "initialize".into(),
            name: None,
            arguments: Some(initialization),
            session_handle: None,
        };
        let ready = client.request_with_timeout(request, options.startup_timeout)?;
        if ready.get("ready").and_then(Value::as_bool) != Some(true)
            || ready.get("pid").and_then(Value::as_u64) == Some(std::process::id() as u64)
            || ready.get("host_bundle_id").and_then(Value::as_str) != Some(host_bundle_id.as_str())
        {
            return Err(DriverError::Protocol {
                reason: "private worker readiness proof did not match the spawned host generation"
                    .into(),
            });
        }
        Ok(client)
    }

    pub(crate) fn is_available(&self) -> bool {
        let mut process = self.process.lock().unwrap();
        if process.stopped {
            return false;
        }
        match process.child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) | Err(_) => {
                process.stopped = true;
                false
            }
        }
    }

    pub(crate) async fn metadata(self: &Arc<Self>) -> Result<DriverMetadata, DriverError> {
        let response = self.request_async("metadata", None, None, None).await?;
        serde_json::from_value(response).map_err(|error| DriverError::Protocol {
            reason: format!("private worker returned invalid metadata: {error}"),
        })
    }

    pub(crate) async fn list_tools(self: &Arc<Self>) -> Result<Value, DriverError> {
        self.request_async("list", None, None, None).await
    }

    pub(crate) async fn invoke(
        self: &Arc<Self>,
        name: &str,
        arguments: Value,
        session_handle: Option<String>,
    ) -> Result<Value, DriverError> {
        self.request_async(
            "call",
            Some(name.to_owned()),
            Some(arguments),
            session_handle,
        )
        .await
    }

    pub(crate) fn bind_session(
        self: &Arc<Self>,
        options: TrustedSessionOptions,
    ) -> Result<String, DriverError> {
        let arguments = serde_json::to_value(options).map_err(|error| DriverError::Protocol {
            reason: format!("serialize private worker session options: {error}"),
        })?;
        let response = self.request_sync("bind_session", None, Some(arguments), None)?;
        response
            .get("session_handle")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| DriverError::Protocol {
                reason: "private worker bind response omitted session_handle".into(),
            })
    }

    pub(crate) fn close_session(&self, session_handle: &str) {
        let _ = self.request_sync("close_session", None, None, Some(session_handle.to_owned()));
    }

    pub(crate) async fn shutdown(self: &Arc<Self>) -> Result<(), DriverError> {
        let client = self.clone();
        tokio::task::spawn_blocking(move || client.shutdown_sync())
            .await
            .map_err(|error| DriverError::Worker {
                reason: format!("join private worker shutdown: {error}"),
            })?
    }

    fn shutdown_sync(&self) -> Result<(), DriverError> {
        let response = self.request_sync("shutdown", None, None, None);
        let mut process = self.process.lock().unwrap();
        process.stdin.take();
        let deadline = std::time::Instant::now() + self.shutdown_timeout;
        loop {
            match process.child.try_wait() {
                Ok(Some(_)) => {
                    process.stopped = true;
                    return response.map(|_| ());
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    drop(process);
                    std::thread::sleep(Duration::from_millis(20));
                    process = self.process.lock().unwrap();
                }
                Ok(None) => {
                    process.stop_and_reap();
                    return response.map(|_| ());
                }
                Err(error) => {
                    process.stopped = true;
                    return Err(DriverError::Worker {
                        reason: format!("wait for private worker: {error}"),
                    });
                }
            }
        }
    }

    async fn request_async(
        self: &Arc<Self>,
        operation: &str,
        name: Option<String>,
        arguments: Option<Value>,
        session_handle: Option<String>,
    ) -> Result<Value, DriverError> {
        let client = self.clone();
        let operation = operation.to_owned();
        tokio::task::spawn_blocking(move || {
            client.request_sync(&operation, name, arguments, session_handle)
        })
        .await
        .map_err(|error| DriverError::Worker {
            reason: format!("join private worker request: {error}"),
        })?
    }

    fn request_sync(
        &self,
        operation: &str,
        name: Option<String>,
        arguments: Option<Value>,
        session_handle: Option<String>,
    ) -> Result<Value, DriverError> {
        let request_id = {
            let mut process = self.process.lock().unwrap();
            let request_id = process.next_request_id;
            process.next_request_id = process.next_request_id.saturating_add(1);
            request_id
        };
        self.request_with_timeout(
            ChannelRequest {
                protocol_version: PRIVATE_WORKER_PROTOCOL_VERSION,
                request_id,
                generation: self.generation.clone(),
                operation: operation.into(),
                name,
                arguments,
                session_handle,
            },
            Duration::from_secs(120),
        )
    }

    fn request_with_timeout(
        &self,
        request: ChannelRequest,
        timeout: Duration,
    ) -> Result<Value, DriverError> {
        let mut process = self.process.lock().unwrap();
        if process.stopped {
            return Err(DriverError::Shutdown);
        }
        if process.child.try_wait().ok().flatten().is_some() {
            process.stopped = true;
            return Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::NotStarted,
                reason: "private worker exited before the request was written".into(),
            });
        }

        let line = serde_json::to_string(&request).map_err(|error| DriverError::Protocol {
            reason: format!("serialize private worker request: {error}"),
        })?;
        let stdin = process.stdin.as_mut().ok_or(DriverError::Shutdown)?;
        if let Err(error) = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
        {
            process.stop_and_reap();
            return Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::Unknown,
                reason: format!("write private worker request: {error}"),
            });
        }

        // Anonymous pipes do not expose a portable read-timeout API. A helper
        // thread owns this one blocking read and returns the reader afterward.
        let stdout = process.stdout.take().ok_or_else(|| DriverError::Worker {
            reason: "private worker response reader is unavailable".into(),
        })?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut line = String::new();
            let result = stdout.read_line(&mut line);
            let _ = tx.send((stdout, result, line));
        });
        let (stdout, read, response_line) = match rx.recv_timeout(timeout) {
            Ok(response) => response,
            Err(_) => {
                process.stop_and_reap();
                return Err(DriverError::ActionInterrupted {
                    completion: ActionCompletion::Unknown,
                    reason: format!(
                        "private worker did not answer request {} within {}ms",
                        request.request_id,
                        timeout.as_millis()
                    ),
                });
            }
        };
        process.stdout = Some(stdout);
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                process.stop_and_reap();
                return Err(DriverError::ActionInterrupted {
                    completion: ActionCompletion::Unknown,
                    reason: format!("read private worker response: {error}"),
                });
            }
        };
        if read == 0 {
            process.stop_and_reap();
            return Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::Unknown,
                reason: "private worker channel closed before completion was reported".into(),
            });
        }
        let response: ChannelResponse = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => {
                process.stop_and_reap();
                return Err(DriverError::Protocol {
                    reason: format!("parse private worker response: {error}"),
                });
            }
        };
        if response.protocol_version != PRIVATE_WORKER_PROTOCOL_VERSION
            || response.request_id != request.request_id
            || response.generation != self.generation
        {
            process.stop_and_reap();
            return Err(DriverError::Protocol {
                reason: "private worker response identity mismatch".into(),
            });
        }
        if response.completion == ActionCompletion::Unknown {
            return Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::Unknown,
                reason: response
                    .error
                    .unwrap_or_else(|| "private worker completion is unknown".into()),
            });
        }
        if response.ok && response.completion != ActionCompletion::Completed {
            return Err(DriverError::Protocol {
                reason: "private worker reported success without completed execution".into(),
            });
        }
        if !response.ok {
            return Err(DriverError::Worker {
                reason: format!(
                    "{}: {}",
                    response.error_code.as_deref().unwrap_or("worker_error"),
                    response.error.as_deref().unwrap_or("request failed")
                ),
            });
        }
        Ok(response.result.unwrap_or(Value::Null))
    }
}

impl Drop for PrivateWorkerClient {
    fn drop(&mut self) {
        let Ok(process) = self.process.get_mut() else {
            return;
        };
        process.stop_and_reap();
    }
}

pub(crate) fn validate_worker_options(
    binary_path: String,
    host_bundle_id: String,
    startup_timeout_ms: Option<u64>,
    shutdown_timeout_ms: Option<u64>,
    configured_driver: ConfiguredDriverOptions,
    environment: Vec<EmbeddedEnvironmentVariable>,
    inherit_stderr: bool,
) -> Result<ValidatedWorkerOptions, DriverError> {
    if binary_path.trim().is_empty() || !std::path::Path::new(&binary_path).is_absolute() {
        return Err(DriverError::Configuration {
            reason: "private worker binary_path must be absolute and non-empty".into(),
        });
    }
    if host_bundle_id.trim().is_empty()
        || host_bundle_id.len() > 255
        || host_bundle_id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(DriverError::Configuration {
            reason:
                "private worker host_bundle_id must be 1-255 non-whitespace, non-control characters"
                    .into(),
        });
    }
    let startup_timeout_ms = startup_timeout_ms.unwrap_or(DEFAULT_STARTUP_TIMEOUT_MS);
    let shutdown_timeout_ms = shutdown_timeout_ms.unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT_MS);
    if startup_timeout_ms == 0 || shutdown_timeout_ms == 0 {
        return Err(DriverError::Configuration {
            reason: "private worker startup and shutdown timeouts must be positive".into(),
        });
    }
    for variable in &environment {
        if !allowed_environment_name(&variable.name)
            || variable.name.contains('=')
            || variable.name.contains('\0')
            || variable.value.contains('\0')
        {
            return Err(DriverError::Configuration {
                reason: format!(
                    "environment variable {} is not in the private-worker safe allowlist",
                    variable.name
                ),
            });
        }
    }
    Ok(ValidatedWorkerOptions {
        binary_path,
        host_bundle_id,
        startup_timeout: Duration::from_millis(startup_timeout_ms),
        shutdown_timeout: Duration::from_millis(shutdown_timeout_ms),
        configured_driver,
        environment,
        inherit_stderr,
    })
}

#[cfg(test)]
mod tests {
    use crate::embedded::{allowed_environment_name, inherited_managed_environment_name};

    #[test]
    fn managed_authorization_is_inherited_but_cannot_be_overridden() {
        for name in [
            "CUA_DRIVER_DISABLE_UNRESTRICTED",
            "CUA_DRIVER_MANAGED_POLICY_FILE",
            "CUA_DRIVER_SESSION_POLICY_FILE",
        ] {
            assert!(inherited_managed_environment_name(name));
            assert!(
                !allowed_environment_name(name),
                "caller-provided worker environment must not override {name}"
            );
        }
    }
}
