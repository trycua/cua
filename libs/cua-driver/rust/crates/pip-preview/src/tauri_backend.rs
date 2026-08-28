use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{AgentViewAck, AgentViewCommand, PipBackend, PipConfig, PipFrame};

const COMPANION_ENV: &str = "CUA_AGENT_VIEW_BINARY";
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

enum WorkerMessage {
    Send(AgentViewCommand),
    SendAndWait {
        command: AgentViewCommand,
        request_id: u64,
        response: mpsc::Sender<anyhow::Result<()>>,
    },
    Shutdown(mpsc::Sender<()>),
}

pub struct TauriPipBackend {
    sender: mpsc::Sender<WorkerMessage>,
}

impl PipBackend for TauriPipBackend {
    fn push_frame(&self, frame: PipFrame) {
        let _ = self
            .sender
            .send(WorkerMessage::Send(AgentViewCommand::Upsert {
                frame: frame.into(),
            }));
    }

    fn remove_workspace(&self, workspace_id: &str) {
        let _ = self
            .sender
            .send(WorkerMessage::Send(AgentViewCommand::RemoveWorkspace {
                workspace_id: workspace_id.to_owned(),
            }));
    }

    fn remove_target(&self, workspace_id: &str, identity_key: &str) {
        let _ = self
            .sender
            .send(WorkerMessage::Send(AgentViewCommand::RemoveTarget {
                workspace_id: workspace_id.to_owned(),
                identity_key: identity_key.to_owned(),
            }));
    }

    fn set_input_passthrough(&self, passthrough: bool) -> anyhow::Result<()> {
        static REQUEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(2);
        let request_id = REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (response_tx, response_rx) = mpsc::channel();
        self.sender
            .send(WorkerMessage::SendAndWait {
                command: AgentViewCommand::SetInputPassthrough {
                    request_id,
                    passthrough,
                },
                request_id,
                response: response_tx,
            })
            .map_err(|_| anyhow::anyhow!("Tauri Agent View process has exited"))?;
        response_rx
            .recv_timeout(ACK_TIMEOUT)
            .map_err(|_| anyhow::anyhow!("timed out updating Tauri Agent View input handling"))?
    }

    fn shutdown(self: Box<Self>) {
        let (done_tx, done_rx) = mpsc::channel();
        let _ = self.sender.send(WorkerMessage::Shutdown(done_tx));
        let _ = done_rx.recv_timeout(Duration::from_secs(2));
    }
}

/// Candidate locations for the companion, ordered from explicit development
/// override to the sibling path used by installed builds.
pub fn companion_binary_candidates() -> Vec<PathBuf> {
    companion_binary_candidates_from(
        std::env::var_os(COMPANION_ENV).map(PathBuf::from),
        std::env::current_exe().ok(),
    )
}

fn companion_binary_candidates_from(
    explicit: Option<PathBuf>,
    executable: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates = explicit.into_iter().collect::<Vec<_>>();
    if let Some(executable) = executable {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join(companion_binary_name()));
        }
    }
    candidates
}

pub fn start_tauri_companion(cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
    let binary = companion_binary_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Tauri Agent View companion `{}` was not found beside cua-driver or in {COMPANION_ENV}",
                companion_binary_name()
            )
        })?;

    let mut child = Command::new(&binary)
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to start Tauri Agent View companion at {}: {error}",
                binary.display()
            )
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Tauri Agent View stdin is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Tauri Agent View stdout is unavailable"))?;
    let (ack_tx, ack_rx) = mpsc::channel();
    thread::Builder::new()
        .name("cua-agent-view-acks".to_owned())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<AgentViewAck>(&line) {
                    Ok(ack) => {
                        if ack_tx.send(ack).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "ignoring invalid Tauri Agent View acknowledgement")
                    }
                }
            }
        })?;

    let configure = AgentViewCommand::Configure {
        request_id: 1,
        title: cfg.title.clone(),
        geometry: cfg.geometry,
    };
    write_command(&mut stdin, &configure)?;
    wait_for_ack(&ack_rx, 1)?;

    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("cua-agent-view-tauri".to_owned())
        .spawn(move || run_worker(child, stdin, ack_rx, receiver))?;
    Ok(Box::new(TauriPipBackend { sender }))
}

fn run_worker(
    mut child: Child,
    mut stdin: ChildStdin,
    ack_rx: mpsc::Receiver<AgentViewAck>,
    receiver: mpsc::Receiver<WorkerMessage>,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            WorkerMessage::Send(command) => {
                if let Err(error) = write_command(&mut stdin, &command) {
                    tracing::warn!(%error, "Tauri Agent View command failed");
                    break;
                }
            }
            WorkerMessage::SendAndWait {
                command,
                request_id,
                response,
            } => {
                let result = write_command(&mut stdin, &command)
                    .and_then(|_| wait_for_ack(&ack_rx, request_id));
                let _ = response.send(result);
            }
            WorkerMessage::Shutdown(done) => {
                let _ = write_command(&mut stdin, &AgentViewCommand::Shutdown);
                wait_for_child_exit(&mut child);
                let _ = done.send(());
                return;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn write_command(stdin: &mut ChildStdin, command: &AgentViewCommand) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *stdin, command)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn wait_for_ack(ack_rx: &mpsc::Receiver<AgentViewAck>, request_id: u64) -> anyhow::Result<()> {
    let deadline = Instant::now() + ACK_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let ack = ack_rx.recv_timeout(remaining).map_err(|_| {
            anyhow::anyhow!("Tauri Agent View did not acknowledge request {request_id}")
        })?;
        if ack.request_id != request_id {
            continue;
        }
        return if ack.ok {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Tauri Agent View rejected request {request_id}: {}",
                ack.error.unwrap_or_else(|| "unknown error".to_owned())
            ))
        };
    }
}

fn wait_for_child_exit(child: &mut Child) {
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn companion_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "cua-agent-view.exe"
    } else {
        "cua-agent-view"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PipNativeContainer, PipTarget, PipTargetKind};

    #[test]
    fn protocol_round_trips_binary_frames_without_public_contract_types() {
        let frame = PipFrame {
            target: PipTarget {
                workspace_id: "private-session".to_owned(),
                workspace_label: "Research".to_owned(),
                target_id: "tab-4".to_owned(),
                identity_key: "page-4".to_owned(),
                target_kind: PipTargetKind::BrowserTab,
                target_label: "Cua".to_owned(),
                native_container: Some(PipNativeContainer {
                    pid: 42,
                    window_id: 9,
                }),
            },
            png_bytes: vec![0, 1, 2, 255],
            action_label: "click".to_owned(),
            timestamp_ms: 7,
            cursor_position: Some((0.25, 0.75)),
        };
        let command = AgentViewCommand::Upsert {
            frame: frame.into(),
        };
        let encoded = serde_json::to_string(&command).unwrap();
        assert!(!encoded.contains("0,1,2,255"));
        let AgentViewCommand::Upsert { frame } =
            serde_json::from_str::<AgentViewCommand>(&encoded).unwrap()
        else {
            panic!("expected upsert");
        };
        assert_eq!(
            frame.into_pip_frame().unwrap().png_bytes,
            vec![0, 1, 2, 255]
        );
    }

    #[test]
    fn explicit_companion_override_is_considered_first() {
        let candidates = companion_binary_candidates_from(
            Some(PathBuf::from("/tmp/cua-agent-view-test")),
            Some(PathBuf::from("/opt/cua/cua-driver")),
        );
        assert_eq!(
            candidates.first().unwrap(),
            &PathBuf::from("/tmp/cua-agent-view-test")
        );
        assert_eq!(candidates[1], PathBuf::from("/opt/cua/cua-agent-view"));
    }
}
