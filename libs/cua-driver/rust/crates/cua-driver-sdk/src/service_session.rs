// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Authenticated, connection-bound session client for an explicit service.
//!
//! The accepted service connection owns exactly one trusted session. Authority
//! is never returned as a bearer value and cannot move to another connection.

use crate::{DriverError, TrustedSessionOptions};
use cua_driver_core::daemon::{DaemonClientKind, DaemonRequest, DaemonResponse, REPLY_DEADLINE};
use cua_driver_core::ipc::{ConnectPolicy, ExchangeError, LocalIpcClient};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RESUME_REGISTRATION_RETRIES: usize = 20;
const RESUME_REGISTRATION_DELAY: Duration = Duration::from_millis(10);

struct ServiceConnection {
    client: LocalIpcClient,
    closed: bool,
    resume_credential: String,
}

pub(crate) struct ServiceSessionClient {
    socket_path: String,
    connection: Mutex<ServiceConnection>,
}

impl ServiceSessionClient {
    pub(crate) fn connect_and_bind(
        socket_path: String,
        options: TrustedSessionOptions,
        client_kind: DaemonClientKind,
    ) -> Result<Arc<Self>, DriverError> {
        let metadata =
            cua_driver_core::daemon::request_daemon_metadata(&socket_path).map_err(|error| {
                DriverError::Transport {
                    socket_path: socket_path.clone(),
                    reason: format!("read service compatibility metadata: {error}"),
                }
            })?;
        crate::validate_daemon_metadata(&metadata)?;
        let client = connect(&socket_path)?;
        let client = Arc::new(Self {
            socket_path,
            connection: Mutex::new(ServiceConnection {
                client,
                closed: false,
                resume_credential: String::new(),
            }),
        });
        let arguments = serde_json::to_value(options).map_err(|error| DriverError::Protocol {
            reason: format!("serialize trusted service session options: {error}"),
        })?;
        let bound = client.request(DaemonRequest {
            method: "trusted_session_begin".into(),
            name: None,
            args: Some(arguments),
            session_id: None,
            observation_origin: None,
            client_kind: Some(client_kind),
        })?;
        let resume_credential = bound
            .get("resume_credential")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| DriverError::Protocol {
                reason: "trusted service bind omitted resume credential".into(),
            })?;
        client.connection.lock().unwrap().resume_credential = resume_credential.to_owned();
        Ok(client)
    }

    pub(crate) async fn invoke(
        self: &Arc<Self>,
        name: &str,
        arguments: Value,
    ) -> Result<Value, DriverError> {
        let client = self.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            client.request(DaemonRequest {
                method: "trusted_session_call".into(),
                name: Some(name),
                args: Some(arguments),
                session_id: None,
                observation_origin: None,
                client_kind: None,
            })
        })
        .await
        .map_err(|error| DriverError::Protocol {
            reason: format!("join trusted service request: {error}"),
        })?
    }

    pub(crate) fn close(&self) {
        let mut connection = self.connection.lock().unwrap();
        if connection.closed && self.resume_connection(&mut connection).is_err() {
            return;
        }
        let request = DaemonRequest {
            method: "trusted_session_end".into(),
            name: None,
            args: None,
            session_id: None,
            observation_origin: None,
            client_kind: None,
        };
        let _ = exchange(&mut connection.client, &request, REPLY_DEADLINE);
        connection.closed = true;
    }

    pub(crate) fn abandon(&self) {
        if let Ok(mut connection) = self.connection.lock() {
            connection.closed = true;
        }
    }

    fn request(&self, request: DaemonRequest) -> Result<Value, DriverError> {
        let mut connection = self.connection.lock().unwrap();
        if connection.closed {
            self.resume_connection(&mut connection)?;
        }
        let action_call = request.method == "trusted_session_call";
        let line = exchange(&mut connection.client, &request, REPLY_DEADLINE).map_err(|error| {
            connection.closed = true;
            self.connection_failure(
                action_call,
                describe(error, "trusted service request", REPLY_DEADLINE),
            )
        })?;
        let response: DaemonResponse = serde_json::from_str(&line).map_err(|error| {
            connection.closed = true;
            if action_call {
                DriverError::ActionInterrupted {
                    completion: crate::worker::ActionCompletion::Unknown,
                    reason: format!("parse trusted service response: {error}"),
                }
            } else {
                DriverError::Protocol {
                    reason: format!("parse trusted service response: {error}"),
                }
            }
        })?;
        if !response.ok {
            return Err(DriverError::Tool {
                tool: request.name.unwrap_or_else(|| request.method.clone()),
                message: response
                    .error
                    .unwrap_or_else(|| "trusted service request failed".into()),
                error_code: response
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_default(),
            });
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    fn resume_connection(&self, connection: &mut ServiceConnection) -> Result<(), DriverError> {
        if connection.resume_credential.is_empty() {
            return Err(DriverError::Shutdown);
        }
        let credential = connection.resume_credential.clone();
        for attempt in 0..=RESUME_REGISTRATION_RETRIES {
            // Replace the connection before presenting the credential.
            // Dropping the old one makes the daemon observe EOF and publish
            // the detachable lease on Unix sockets and Windows named pipes.
            connection.client = connect(&self.socket_path)?;
            let request = DaemonRequest {
                method: "trusted_session_resume".into(),
                name: None,
                args: Some(serde_json::json!({
                    "resume_credential": credential,
                })),
                session_id: None,
                observation_origin: None,
                client_kind: None,
            };
            let line =
                exchange(&mut connection.client, &request, REPLY_DEADLINE).map_err(|error| {
                    DriverError::Transport {
                        socket_path: self.socket_path.clone(),
                        reason: describe(error, "trusted session resume", REPLY_DEADLINE),
                    }
                })?;
            let response: DaemonResponse =
                serde_json::from_str(&line).map_err(|error| DriverError::Protocol {
                    reason: format!("parse trusted session resume response: {error}"),
                })?;
            if response.ok {
                let rotated = response
                    .result
                    .as_ref()
                    .and_then(|value| value.get("resume_credential"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| DriverError::Protocol {
                        reason: "trusted session resume omitted rotated credential".into(),
                    })?;
                connection.resume_credential = rotated.to_owned();
                connection.closed = false;
                return Ok(());
            }

            let reason = response
                .error
                .unwrap_or_else(|| "trusted session resume failed".into());
            let registration_race = reason.contains("unavailable or already used")
                && attempt < RESUME_REGISTRATION_RETRIES;
            if registration_race {
                std::thread::sleep(RESUME_REGISTRATION_DELAY);
                continue;
            }
            connection.resume_credential.clear();
            return Err(DriverError::Transport {
                socket_path: self.socket_path.clone(),
                reason,
            });
        }
        unreachable!("bounded resume loop returns on its final attempt")
    }

    fn connection_failure(&self, action_call: bool, reason: String) -> DriverError {
        if action_call {
            DriverError::ActionInterrupted {
                completion: crate::worker::ActionCompletion::Unknown,
                reason,
            }
        } else {
            DriverError::Transport {
                socket_path: self.socket_path.clone(),
                reason,
            }
        }
    }
}

impl Drop for ServiceSessionClient {
    fn drop(&mut self) {
        self.abandon();
    }
}

/// Send one request and read its reply line within `reply_deadline`.
fn exchange(
    client: &mut LocalIpcClient,
    request: &DaemonRequest,
    reply_deadline: Duration,
) -> Result<String, ExchangeError> {
    let frame = serde_json::to_vec(request)
        .map_err(|error| ExchangeError::Write(std::io::Error::other(error)))?;
    client.exchange_line(&frame, Instant::now() + reply_deadline)
}

/// Why `exchange` for `what` got no reply.
fn describe(error: ExchangeError, what: &str, reply_deadline: Duration) -> String {
    match error {
        ExchangeError::Write(error) => format!("write {what}: {error}"),
        ExchangeError::Read(error) => format!("read reply to {what}: {error}"),
        ExchangeError::TimedOut { .. } => {
            format!("timed out after {reply_deadline:?} waiting for the reply to {what}")
        }
        ExchangeError::Closed => format!("trusted service closed the connection during {what}"),
    }
}

fn connect(socket_path: &str) -> Result<LocalIpcClient, DriverError> {
    LocalIpcClient::connect(socket_path, ConnectPolicy::REQUEST).map_err(|error| {
        DriverError::Transport {
            socket_path: socket_path.to_owned(),
            reason: format!("connect trusted service session: {error}"),
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{SessionPermissionMode, TrustedSessionOptions};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    fn serve_compatible_metadata(listener: &UnixListener) {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let request: DaemonRequest = serde_json::from_str(&line).unwrap();
        assert_eq!(request.method, "metadata");
        let mut writer = stream;
        writeln!(
            writer,
            "{}",
            serde_json::to_string(&DaemonResponse::ok(
                serde_json::to_value(cua_driver_core::daemon::current_daemon_metadata()).unwrap()
            ))
            .unwrap()
        )
        .unwrap();
    }

    #[test]
    fn a_stalled_service_reply_times_out_at_the_reply_deadline() {
        let directory = crate::socket_tempdir();
        let socket = directory.path().join("stalled-service.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            (&stream).write_all(b"{\"ok\":").unwrap();
            let _ = released.recv();
        });

        let mut client = connect(socket.to_str().unwrap()).unwrap();
        let request = DaemonRequest {
            method: "trusted_session_call".into(),
            name: Some("health_report".into()),
            args: None,
            session_id: None,
            observation_origin: None,
            client_kind: None,
        };
        let deadline = Duration::from_millis(300);
        let error = exchange(&mut client, &request, deadline).unwrap_err();
        assert_eq!(
            describe(error, "trusted service request", deadline),
            "timed out after 300ms waiting for the reply to trusted service request"
        );
        release.send(()).unwrap();
        server.join().unwrap();
    }

    #[tokio::test]
    async fn next_call_resumes_with_single_use_host_credential_after_disconnect() {
        let directory = crate::socket_tempdir();
        let socket = directory.path().join("resumable-service.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            serve_compatible_metadata(&listener);
            {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let begin: DaemonRequest = serde_json::from_str(&line).unwrap();
                assert_eq!(begin.method, "trusted_session_begin");
                writeln!(
                    writer,
                    "{}",
                    serde_json::to_string(&DaemonResponse::ok(serde_json::json!({
                        "resume_credential": "resume-original"
                    })))
                    .unwrap()
                )
                .unwrap();

                line.clear();
                reader.read_line(&mut line).unwrap();
                let call: DaemonRequest = serde_json::from_str(&line).unwrap();
                assert_eq!(call.method, "trusted_session_call");
                // Drop without a response. The SDK must not retry this action.
            }

            // The reconnect can beat the daemon task that records EOF from the
            // previous connection. A transient unavailable response must keep
            // the single-use credential intact and retry on a fresh socket.
            {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let resume: DaemonRequest = serde_json::from_str(&line).unwrap();
                assert_eq!(resume.method, "trusted_session_resume");
                writeln!(
                    writer,
                    "{}",
                    serde_json::to_string(&DaemonResponse::err(
                        "trusted session resume credential is unavailable or already used",
                        77,
                    ))
                    .unwrap()
                )
                .unwrap();
            }

            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let resume: DaemonRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(resume.method, "trusted_session_resume");
            assert_eq!(resume.args.unwrap()["resume_credential"], "resume-original");
            writeln!(
                writer,
                "{}",
                serde_json::to_string(&DaemonResponse::ok(serde_json::json!({
                    "resume_credential": "resume-rotated"
                })))
                .unwrap()
            )
            .unwrap();

            line.clear();
            reader.read_line(&mut line).unwrap();
            let call: DaemonRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(call.method, "trusted_session_call");
            writeln!(
                writer,
                "{}",
                serde_json::to_string(&DaemonResponse::ok(serde_json::json!({
                    "resumed": true
                })))
                .unwrap()
            )
            .unwrap();
        });

        let client = ServiceSessionClient::connect_and_bind(
            socket.to_string_lossy().into_owned(),
            TrustedSessionOptions {
                public_session: "resume-test".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                capability_manifest_path: None,
                bounded_manifest_path: None,
            },
            DaemonClientKind::Unknown,
        )
        .unwrap();
        assert!(matches!(
            client.invoke("click", serde_json::json!({})).await,
            Err(DriverError::ActionInterrupted {
                completion: crate::worker::ActionCompletion::Unknown,
                ..
            })
        ));
        let resumed = client
            .invoke("get_session", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(resumed["resumed"], true);
        server.join().unwrap();
    }
}
