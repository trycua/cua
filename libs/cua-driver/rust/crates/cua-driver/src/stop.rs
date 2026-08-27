//! PID-bound daemon shutdown for the public `cua-driver stop` command.
//!
//! The uninstaller validates a release daemon's executable identity before it
//! asks the installed helper to stop that exact PID. When `--expected-pid` is
//! present, the daemon metadata handshake and the shutdown request must share
//! one connected transport so another daemon cannot replace the socket between
//! those two operations.

use cua_driver_core::daemon::{DaemonMetadata, DaemonRequest, DaemonResponse};

fn request(method: &str) -> DaemonRequest {
    DaemonRequest {
        method: method.to_owned(),
        name: None,
        args: None,
        session_id: None,
        observation_origin: None,
        client_kind: None,
    }
}

fn require_ok(response: DaemonResponse, operation: &str) -> anyhow::Result<serde_json::Value> {
    if !response.ok {
        anyhow::bail!(
            "daemon {operation} request failed: {}",
            response.error.unwrap_or_else(|| "unknown error".to_owned())
        );
    }
    Ok(response.result.unwrap_or(serde_json::Value::Null))
}

#[cfg(unix)]
fn pid_bound_shutdown(socket_path: &str, expected_pid: u32) -> anyhow::Result<()> {
    use std::io::Read as _;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let mut stream = UnixStream::connect(socket_path)
        .map_err(|error| anyhow::anyhow!("connect to {socket_path}: {error}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut writer = stream.try_clone()?;
    let mut pending = Vec::<u8>::with_capacity(64 * 1024);

    let mut exchange = |request: &DaemonRequest| -> anyhow::Result<DaemonResponse> {
        let line = serde_json::to_string(request)? + "\n";
        let write_deadline = Instant::now() + Duration::from_secs(120);
        cua_driver_core::socket_io::write_all_with_retry(
            &mut writer,
            line.as_bytes(),
            write_deadline,
        )?;

        let overall_deadline = Instant::now() + Duration::from_secs(120);
        let mut chunk = [0_u8; 64 * 1024];
        let response_line = loop {
            if let Some(newline) = pending.iter().position(|&byte| byte == b'\n') {
                let line = String::from_utf8_lossy(&pending[..newline]).into_owned();
                pending.drain(..=newline);
                break line;
            }
            match stream.read(&mut chunk) {
                Ok(0) if pending.is_empty() => {
                    anyhow::bail!("daemon closed connection without response");
                }
                Ok(0) => {
                    let line = String::from_utf8_lossy(&pending).into_owned();
                    pending.clear();
                    break line;
                }
                Ok(read) => pending.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() >= overall_deadline {
                        anyhow::bail!(
                            "timed out after 120s waiting for daemon response (received {} bytes so far)",
                            pending.len()
                        );
                    }
                }
                Err(error) => return Err(error.into()),
            }
        };
        Ok(serde_json::from_str(&response_line)?)
    };

    let metadata_value = require_ok(exchange(&request("metadata"))?, "metadata")?;
    let metadata: DaemonMetadata = serde_json::from_value(metadata_value)
        .map_err(|error| anyhow::anyhow!("invalid daemon metadata: {error}"))?;
    if metadata.pid != expected_pid {
        anyhow::bail!(
            "daemon pid mismatch (expected {expected_pid}, found {})",
            metadata.pid
        );
    }

    require_ok(exchange(&request("shutdown"))?, "shutdown")?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn pid_bound_shutdown(socket_path: &str, expected_pid: u32) -> anyhow::Result<()> {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(socket_path)
        {
            Ok(pipe) => break pipe,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(anyhow::anyhow!("connect to {socket_path}: {error}"));
            }
        }
    };
    let mut reader = BufReader::new(pipe.try_clone()?);

    let mut exchange = |request: &DaemonRequest| -> anyhow::Result<DaemonResponse> {
        pipe.write_all((serde_json::to_string(request)? + "\n").as_bytes())?;
        pipe.flush()?;
        let mut response_line = String::new();
        if reader.read_line(&mut response_line)? == 0 {
            anyhow::bail!("daemon closed connection without response");
        }
        Ok(serde_json::from_str(response_line.trim_end())?)
    };

    let metadata_value = require_ok(exchange(&request("metadata"))?, "metadata")?;
    let metadata: DaemonMetadata = serde_json::from_value(metadata_value)
        .map_err(|error| anyhow::anyhow!("invalid daemon metadata: {error}"))?;
    if metadata.pid != expected_pid {
        anyhow::bail!(
            "daemon pid mismatch (expected {expected_pid}, found {})",
            metadata.pid
        );
    }

    require_ok(exchange(&request("shutdown"))?, "shutdown")?;
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn pid_bound_shutdown(_socket_path: &str, _expected_pid: u32) -> anyhow::Result<()> {
    anyhow::bail!("--expected-pid is not supported on this platform")
}

pub fn run_pid_bound_stop_cmd(socket_path: &str, expected_pid: u32) {
    // A PID-bound stop intentionally does not make an initial liveness probe:
    // the very first connection is the one whose metadata is checked and whose
    // shutdown is requested, closing the check/use race.
    if let Err(error) = pid_bound_shutdown(socket_path, expected_pid) {
        eprintln!("stop: {error}");
        std::process::exit(1);
    }

    // Poll only after the authority-bound shutdown request has completed.
    // These probes can use fresh connections safely because they confer no
    // authority; they only verify that the selected endpoint disappeared.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let gone = if std::path::Path::new(socket_path).exists() {
            false
        } else {
            !crate::serve::is_daemon_listening(socket_path)
        };
        if gone {
            return;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("Cua Driver daemon did not release socket within 2s");
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
