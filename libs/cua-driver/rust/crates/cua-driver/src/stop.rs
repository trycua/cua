use cua_driver_core::daemon::{send_request, DaemonRequest};

fn pid_bound_shutdown(socket_path: &str, expected_pid: u32) -> anyhow::Result<()> {
    let response = send_request(
        socket_path,
        &DaemonRequest {
            method: "shutdown_if_pid".to_owned(),
            name: None,
            args: Some(serde_json::json!({"expected_pid": expected_pid})),
            session_id: None,
            observation_origin: None,
            client_kind: None,
        },
    )?;
    if !response.ok {
        anyhow::bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "daemon shutdown request failed".to_owned())
        );
    }
    Ok(())
}

pub fn run_pid_bound_stop_cmd(socket_path: &str, expected_pid: u32) {
    if let Err(error) = pid_bound_shutdown(socket_path, expected_pid) {
        eprintln!("stop: {error}");
        std::process::exit(1);
    }

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
