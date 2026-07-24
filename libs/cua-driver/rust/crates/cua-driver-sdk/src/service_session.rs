//! Authenticated, connection-bound session client for an explicit service.
//!
//! The accepted service connection owns exactly one trusted session. Authority
//! is never returned as a bearer value and cannot move to another connection.

use crate::{DriverError, TrustedSessionOptions};
use cua_driver_core::daemon::{DaemonClientKind, DaemonRequest, DaemonResponse};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const SERVICE_REQUEST_DEADLINE: Duration = Duration::from_secs(120);

#[cfg(unix)]
type ServiceStream = std::os::unix::net::UnixStream;
#[cfg(target_os = "windows")]
type ServiceStream = std::fs::File;

struct ServiceConnection {
    reader: BufReader<ServiceStream>,
    writer: ServiceStream,
    closed: bool,
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
        let stream = connect(&socket_path)?;
        let writer = stream.try_clone().map_err(|error| DriverError::Transport {
            socket_path: socket_path.clone(),
            reason: format!("clone trusted service connection: {error}"),
        })?;
        let client = Arc::new(Self {
            socket_path,
            connection: Mutex::new(ServiceConnection {
                reader: BufReader::new(stream),
                writer,
                closed: false,
            }),
        });
        let arguments = serde_json::to_value(options).map_err(|error| DriverError::Protocol {
            reason: format!("serialize trusted service session options: {error}"),
        })?;
        client.request(DaemonRequest {
            method: "trusted_session_begin".into(),
            name: None,
            args: Some(arguments),
            session_id: None,
            observation_origin: None,
            client_kind: Some(client_kind),
        })?;
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
        if connection.closed {
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
        let _ = write_request(&mut connection.writer, &request);
        let _ = read_response_line(&mut connection.reader);
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
            return Err(DriverError::Shutdown);
        }
        let action_call = request.method == "trusted_session_call";
        write_request(&mut connection.writer, &request).map_err(|error| {
            connection.closed = true;
            self.connection_failure(
                action_call,
                format!("write trusted service request: {error}"),
            )
        })?;
        let line = read_response_line(&mut connection.reader).map_err(|error| {
            connection.closed = true;
            self.connection_failure(
                action_call,
                format!("read trusted service response: {error}"),
            )
        })?;
        let Some(line) = line else {
            connection.closed = true;
            return Err(
                self.connection_failure(action_call, "trusted service connection closed".into())
            );
        };
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

#[cfg(unix)]
fn write_request(writer: &mut impl Write, request: &DaemonRequest) -> std::io::Result<()> {
    let mut frame = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    frame.push(b'\n');
    cua_driver_core::socket_io::write_all_with_retry(
        writer,
        &frame,
        Instant::now() + SERVICE_REQUEST_DEADLINE,
    )
}

#[cfg(target_os = "windows")]
fn write_request(writer: &mut impl Write, request: &DaemonRequest) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, request).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(unix)]
fn read_response_line(reader: &mut BufReader<ServiceStream>) -> std::io::Result<Option<String>> {
    let deadline = Instant::now() + SERVICE_REQUEST_DEADLINE;
    let mut bytes = Vec::new();
    loop {
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) if bytes.is_empty() => return Ok(None),
            Ok(0) | Ok(_) => {
                return String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out after 120s waiting for trusted service response",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn read_response_line(reader: &mut BufReader<ServiceStream>) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    match reader.read_line(&mut line)? {
        0 => Ok(None),
        _ => Ok(Some(line)),
    }
}

#[cfg(unix)]
fn connect(socket_path: &str) -> Result<ServiceStream, DriverError> {
    let stream = std::os::unix::net::UnixStream::connect(socket_path).map_err(|error| {
        DriverError::Transport {
            socket_path: socket_path.to_owned(),
            reason: format!("connect trusted service session: {error}"),
        }
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(5))))
        .map_err(|error| DriverError::Transport {
            socket_path: socket_path.to_owned(),
            reason: format!("configure trusted service session timeout: {error}"),
        })?;
    Ok(stream)
}

#[cfg(target_os = "windows")]
fn connect(socket_path: &str) -> Result<ServiceStream, DriverError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(socket_path)
        {
            Ok(pipe) => return Ok(pipe),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => {
                return Err(DriverError::Transport {
                    socket_path: socket_path.to_owned(),
                    reason: format!("connect trusted service named pipe: {error}"),
                });
            }
        }
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn connect(socket_path: &str) -> Result<ServiceStream, DriverError> {
    Err(DriverError::Transport {
        socket_path: socket_path.to_owned(),
        reason: "trusted service sessions are unsupported on this platform".into(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{SessionPermissionMode, TrustedSessionOptions};
    use std::os::unix::net::UnixListener;

    #[test]
    fn response_reader_preserves_partial_utf8_across_poll_timeouts() {
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let server = std::thread::spawn(move || {
            writer.write_all(&[0xe2]).unwrap();
            std::thread::sleep(Duration::from_millis(80));
            writer.write_all(&[0x82, 0xac, b'\n']).unwrap();
        });

        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_response_line(&mut reader).unwrap().as_deref(),
            Some("€\n")
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn lost_action_response_is_reported_with_unknown_completion() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("service.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
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
                serde_json::to_string(&DaemonResponse::ok(serde_json::json!({}))).unwrap()
            )
            .unwrap();

            line.clear();
            reader.read_line(&mut line).unwrap();
            let call: DaemonRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(call.method, "trusted_session_call");
            // Closing the accepted connection after reading the action leaves
            // its completion unknown to the client.
        });

        let client = ServiceSessionClient::connect_and_bind(
            socket.to_string_lossy().into_owned(),
            TrustedSessionOptions {
                public_session: "lost-response".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            },
            DaemonClientKind::Unknown,
        )
        .unwrap();
        assert!(matches!(
            client
                .invoke("click", serde_json::json!({"x": 1, "y": 1}))
                .await,
            Err(DriverError::ActionInterrupted {
                completion: crate::worker::ActionCompletion::Unknown,
                ..
            })
        ));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn transient_read_timeouts_are_poll_intervals_not_action_deadlines() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("delayed-service.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;

            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            writeln!(
                writer,
                "{}",
                serde_json::to_string(&DaemonResponse::ok(serde_json::json!({}))).unwrap()
            )
            .unwrap();

            line.clear();
            reader.read_line(&mut line).unwrap();
            let call: DaemonRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(call.method, "trusted_session_call");
            std::thread::sleep(Duration::from_millis(80));
            writeln!(
                writer,
                "{}",
                serde_json::to_string(&DaemonResponse::ok(serde_json::json!({
                    "content": [{"type": "text", "text": "completed"}],
                    "isError": false,
                })))
                .unwrap()
            )
            .unwrap();
        });

        let client = ServiceSessionClient::connect_and_bind(
            socket.to_string_lossy().into_owned(),
            TrustedSessionOptions {
                public_session: "delayed-response".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            },
            DaemonClientKind::Unknown,
        )
        .unwrap();
        client
            .connection
            .lock()
            .unwrap()
            .reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();

        let result = client
            .invoke("health_report", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "completed");
        server.join().unwrap();
    }
}
