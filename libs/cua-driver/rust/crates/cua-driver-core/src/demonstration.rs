//! Human demonstration lifecycle and artifact writing.
//!
//! Human observations are separate from executable tool-call recording.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use serde_json::json;

use crate::demonstration_artifacts::{self, Artifacts, CaptureEvidence};
use crate::recording::screenshot_for;

const EVENT_QUEUE_CAPACITY: usize = 1024;

#[derive(Debug)]
pub struct DemonstrationConfig {
    pub pid: i64,
    pub window_id: u64,
    pub output_dir: PathBuf,
    pub owner: Option<String>,
}

#[derive(Debug)]
pub struct DemonstrationResult {
    pub output_dir: PathBuf,
    pub manifest: PathBuf,
    pub artifacts: Artifacts,
}

/// Owns the one process-wide human demonstration.
pub struct DemonstrationManager {
    active: Mutex<Option<DemonstrationSession>>,
}

impl DemonstrationManager {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    pub fn start(&self, config: DemonstrationConfig) -> anyhow::Result<PathBuf> {
        self.start_with(config, DemonstrationSession::start)
    }

    fn start_with(
        &self,
        config: DemonstrationConfig,
        start: impl FnOnce(DemonstrationConfig) -> anyhow::Result<DemonstrationSession>,
    ) -> anyhow::Result<PathBuf> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.is_some() {
            anyhow::bail!("a demonstration is already active");
        }
        let output_dir = config.output_dir.clone();
        *active = Some(start(config)?);
        Ok(output_dir)
    }

    pub fn stop(&self, requester: Option<&str>) -> anyhow::Result<Option<DemonstrationResult>> {
        self.finish_if(
            |session| requester_can_stop(session.owner.as_deref(), requester),
            "requested",
        )
    }

    pub fn stop_owner(&self, owner: &str) -> anyhow::Result<Option<DemonstrationResult>> {
        self.finish_if(
            |session| session.owner.as_deref() == Some(owner),
            "owner_disconnected",
        )
    }

    pub(crate) fn output_dir(&self, requester: Option<&str>) -> Option<PathBuf> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|session| requester_can_stop(session.owner.as_deref(), requester))
            .map(|session| session.output_dir.clone())
    }

    fn finish_if(
        &self,
        predicate: impl FnOnce(&DemonstrationSession) -> bool,
        stop_reason: &'static str,
    ) -> anyhow::Result<Option<DemonstrationResult>> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = active.as_mut().filter(|session| predicate(session)) else {
            return Ok(None);
        };
        let result = session.finish(stop_reason);
        if result.is_ok() || session.is_terminal() {
            *active = None;
        }
        result.map(Some)
    }
}

impl Drop for DemonstrationManager {
    fn drop(&mut self) {
        if let Some(session) = self
            .active
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            let _ = session.finish("runtime_shutdown");
        }
    }
}

fn requester_can_stop(owner: Option<&str>, requester: Option<&str>) -> bool {
    requester.is_none() || owner == requester
}

impl Default for DemonstrationManager {
    fn default() -> Self {
        Self::new()
    }
}

struct DemonstrationSession {
    state: DemonstrationState,
    output_dir: PathBuf,
    owner: Option<String>,
    pid: i64,
    window_id: u64,
    started_at_ms: u64,
}

enum DemonstrationState {
    Active {
        capture: DemonstrationCapture,
        writer: std::thread::JoinHandle<anyhow::Result<WriteSummary>>,
    },
    Artifacts {
        evidence: CaptureEvidence,
        action_count: usize,
    },
    Terminal(String),
}

enum DemonstrationCapture {
    Physical(input_capture::Demonstration),
    #[cfg(test)]
    Injected(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

impl DemonstrationCapture {
    fn stop(self) -> input_capture::CaptureStats {
        match self {
            Self::Physical(capture) => capture.stop(),
            #[cfg(test)]
            Self::Injected(stopped) => {
                stopped.store(true, std::sync::atomic::Ordering::Release);
                input_capture::CaptureStats::default()
            }
        }
    }
}

impl DemonstrationSession {
    fn start(config: DemonstrationConfig) -> anyhow::Result<Self> {
        let started_at_ms = crate::recording::now_ms();
        prepare_output_dir(&config.output_dir)?;

        let (sender, receiver) = std::sync::mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let capture = input_capture::Demonstration::start(
            input_capture::CaptureConfig {
                pid: config.pid,
                window_id: config.window_id,
            },
            sender,
        )
        .map_err(|error| anyhow::anyhow!("start input capture: {error}"))?;

        let writer_dir = config.output_dir.clone();
        let pid = config.pid;
        let window_id = config.window_id;
        let writer = std::thread::Builder::new()
            .name("demonstration-writer".into())
            .spawn(move || write_events(&writer_dir, pid, window_id, receiver))
            .context("start demonstration writer")?;

        Ok(Self {
            state: DemonstrationState::Active {
                capture: DemonstrationCapture::Physical(capture),
                writer,
            },
            output_dir: config.output_dir,
            owner: config.owner,
            pid,
            window_id,
            started_at_ms,
        })
    }

    fn finish(&mut self, stop_reason: &'static str) -> anyhow::Result<DemonstrationResult> {
        if matches!(self.state, DemonstrationState::Active { .. }) {
            let state = std::mem::replace(
                &mut self.state,
                DemonstrationState::Terminal("demonstration finalization interrupted".into()),
            );
            let DemonstrationState::Active { capture, writer } = state else {
                unreachable!();
            };
            let capture = capture.stop();
            let written = match writer.join() {
                Ok(Ok(written)) => written,
                Ok(Err(error)) => {
                    let message = format!("demonstration writer failed: {error:#}");
                    self.state = DemonstrationState::Terminal(message.clone());
                    anyhow::bail!(message);
                }
                Err(_) => {
                    let message = "demonstration writer panicked".to_owned();
                    self.state = DemonstrationState::Terminal(message.clone());
                    anyhow::bail!(message);
                }
            };
            self.state = DemonstrationState::Artifacts {
                evidence: CaptureEvidence {
                    dropped_events: capture.dropped_events,
                    screenshot_failures: written.screenshot_failures,
                },
                action_count: written.action_count,
            };
        }
        let (evidence, action_count) = match &self.state {
            DemonstrationState::Artifacts {
                evidence,
                action_count,
            } => (*evidence, *action_count),
            DemonstrationState::Terminal(message) => anyhow::bail!(message.clone()),
            DemonstrationState::Active { .. } => unreachable!(),
        };
        let artifacts = demonstration_artifacts::write(&self.output_dir, evidence)?;
        debug_assert_eq!(artifacts.summary.action_count, action_count);
        let manifest = write_manifest(
            &self.output_dir,
            self.pid,
            self.window_id,
            self.started_at_ms,
            stop_reason,
            &artifacts.summary,
        )?;
        Ok(DemonstrationResult {
            output_dir: self.output_dir.clone(),
            manifest,
            artifacts,
        })
    }

    fn is_terminal(&self) -> bool {
        matches!(self.state, DemonstrationState::Terminal(_))
    }
}

fn prepare_output_dir(output_dir: &Path) -> anyhow::Result<()> {
    if output_dir.exists() {
        let mut entries = std::fs::read_dir(output_dir)
            .with_context(|| format!("read {}", output_dir.display()))?;
        if entries.next().is_some() {
            anyhow::bail!(
                "demonstration output directory must be empty: {}",
                output_dir.display()
            );
        }
    } else {
        std::fs::create_dir_all(output_dir)
            .with_context(|| format!("create {}", output_dir.display()))?;
    }
    Ok(())
}

fn write_manifest(
    output_dir: &Path,
    pid: i64,
    window_id: u64,
    started_at_ms: u64,
    stop_reason: &str,
    summary: &demonstration_artifacts::Summary,
) -> anyhow::Result<PathBuf> {
    let path = output_dir.join("DEMONSTRATION.json");
    let temporary = output_dir.join("DEMONSTRATION.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "driver_version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "pid": pid,
            "window_id": window_id,
            "started_at_ms": started_at_ms,
            "stopped_at_ms": crate::recording::now_ms(),
            "stop_reason": stop_reason,
            "action_count": summary.action_count,
            "dropped_events": summary.dropped_events,
            "screenshot_failures": summary.screenshot_failures,
            "complete": summary.complete,
        }))?,
    )?;
    std::fs::rename(temporary, &path)?;
    Ok(path)
}

struct WriteSummary {
    action_count: usize,
    screenshot_failures: usize,
}

fn write_events(
    output_dir: &Path,
    pid: i64,
    window_id: u64,
    events: std::sync::mpsc::Receiver<input_capture::HumanEvent>,
) -> anyhow::Result<WriteSummary> {
    let mut count = 0;
    let mut screenshot_failures = 0;
    for event in events {
        let (action, mut arguments) = event.to_record();
        count += 1;
        let turn_dir = output_dir.join(format!("turn-{count:05}"));
        std::fs::create_dir_all(&turn_dir)?;
        if let Some(arguments) = arguments.as_object_mut() {
            arguments.insert("pid".into(), json!(pid));
            arguments.insert("window_id".into(), json!(window_id));
        }
        std::fs::write(
            turn_dir.join("action.json"),
            serde_json::to_vec_pretty(&json!({
                "tool": action,
                "arguments": arguments,
                "source": "human",
                "t_ms_from_session_start": event.t_ms(),
            }))?,
        )?;
        if let Some(png) = screenshot_for(Some(window_id), Some(pid)) {
            std::fs::write(turn_dir.join("screenshot.png"), png)?;
        } else {
            screenshot_failures += 1;
        }
    }
    Ok(WriteSummary {
        action_count: count,
        screenshot_failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_capture::{Button, HumanEvent};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cua-demonstration-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stopping_an_inactive_manager_is_a_noop() {
        let manager = DemonstrationManager::new();
        assert!(manager.stop(None).unwrap().is_none());
        assert!(manager.stop_owner("missing").unwrap().is_none());
    }

    #[test]
    fn manual_stop_is_scoped_to_the_owning_session() {
        assert!(requester_can_stop(Some("session-a"), Some("session-a")));
        assert!(!requester_can_stop(Some("session-a"), Some("session-b")));
        assert!(!requester_can_stop(None, Some("session-a")));
        assert!(requester_can_stop(Some("session-a"), None));
        assert!(requester_can_stop(None, None));
    }

    #[test]
    fn failed_artifact_finalization_is_retryable_after_capture_stops() {
        let output_dir = temp_dir("retry-finalize").join("output");
        std::fs::write(&output_dir, "blocks read_dir").unwrap();
        let capture_stopped = Arc::new(AtomicBool::new(false));
        let manager = DemonstrationManager::new();
        let stopped = capture_stopped.clone();
        manager
            .start_with(
                DemonstrationConfig {
                    pid: 42,
                    window_id: 7,
                    output_dir: output_dir.clone(),
                    owner: Some("session-a".into()),
                },
                move |config| {
                    Ok(DemonstrationSession {
                        state: DemonstrationState::Active {
                            capture: DemonstrationCapture::Injected(stopped),
                            writer: std::thread::spawn(|| {
                                Ok(WriteSummary {
                                    action_count: 0,
                                    screenshot_failures: 0,
                                })
                            }),
                        },
                        output_dir: config.output_dir,
                        owner: config.owner,
                        pid: config.pid,
                        window_id: config.window_id,
                        started_at_ms: 10,
                    })
                },
            )
            .unwrap();

        assert!(manager.stop_owner("session-a").is_err());
        assert!(capture_stopped.load(Ordering::Acquire));
        assert_eq!(
            manager.output_dir(Some("session-a")),
            Some(output_dir.clone())
        );

        std::fs::remove_file(&output_dir).unwrap();
        std::fs::create_dir(&output_dir).unwrap();
        let result = manager.stop_owner("session-a").unwrap().unwrap();
        assert!(result.manifest.exists());
        assert!(manager.output_dir(Some("session-a")).is_none());
    }

    #[test]
    fn writes_non_executable_human_observations() {
        let output_dir = temp_dir("events");
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(HumanEvent::Click {
                x: 10.0,
                y: 20.0,
                button: Button::Right,
                t_ms: 50,
            })
            .unwrap();
        drop(sender);

        assert_eq!(
            write_events(&output_dir, 42, 7, receiver)
                .unwrap()
                .action_count,
            1
        );
        let action = std::fs::read_to_string(output_dir.join("turn-00001/action.json")).unwrap();
        assert!(action.contains("human_click"));
        assert!(action.contains("\"source\": \"human\""));

        let artifacts =
            demonstration_artifacts::write(&output_dir, CaptureEvidence::default()).unwrap();
        assert_eq!(artifacts.summary.action_count, 1);
    }

    #[test]
    fn rejects_nonempty_output_directory() {
        let output_dir = temp_dir("nonempty");
        std::fs::write(output_dir.join("existing"), "data").unwrap();
        assert!(prepare_output_dir(&output_dir).is_err());
    }

    #[test]
    fn manifest_records_capture_completeness() {
        let output_dir = temp_dir("manifest");
        let summary = demonstration_artifacts::Summary {
            action_count: 3,
            duration_ms: 100,
            dropped_events: 1,
            screenshot_failures: 2,
            complete: false,
            action_histogram: Default::default(),
        };
        let path = write_manifest(&output_dir, 42, 7, 10, "requested", &summary).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["pid"], 42);
        assert_eq!(value["complete"], false);
        assert_eq!(value["stop_reason"], "requested");
    }

    #[test]
    fn empty_demonstration_still_has_artifacts() {
        let output_dir = temp_dir("empty");
        let artifacts =
            demonstration_artifacts::write(&output_dir, CaptureEvidence::default()).unwrap();
        assert_eq!(artifacts.summary.action_count, 0);
        assert!(artifacts.trajectory_md.exists());
        assert!(artifacts.summary_json.exists());
    }

    #[test]
    fn writer_failure_stops_capture_and_allows_the_next_demonstration() {
        let manager = Arc::new(DemonstrationManager::new());
        let session = format!(
            "demonstration-writer-failure-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let hook_manager = manager.clone();
        let _hook = crate::session::register_scoped_fallible_session_end_hook(
            "injected_demonstration_writer",
            move |session_id| {
                hook_manager
                    .stop_owner(session_id)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
        );
        let stopped = Arc::new(AtomicBool::new(false));
        let failed_dir = temp_dir("writer-failure");
        manager
            .start_with(
                DemonstrationConfig {
                    pid: 42,
                    window_id: 7,
                    output_dir: failed_dir.clone(),
                    owner: Some(session.clone()),
                },
                |config| {
                    Ok(DemonstrationSession {
                        state: DemonstrationState::Active {
                            capture: DemonstrationCapture::Injected(stopped.clone()),
                            writer: std::thread::spawn(|| anyhow::bail!("injected writer failure")),
                        },
                        output_dir: config.output_dir,
                        owner: config.owner,
                        pid: config.pid,
                        window_id: config.window_id,
                        started_at_ms: 0,
                    })
                },
            )
            .unwrap();

        assert!(crate::session::fire_session_end(&session));
        let cleanup = crate::session::session_cleanup_status(&session);
        assert!(!cleanup.complete);
        assert!(cleanup
            .failures
            .iter()
            .any(|failure| failure.error.contains("injected writer failure")));
        assert!(stopped.load(Ordering::Acquire));
        assert!(manager.output_dir(Some(&session)).is_none());
        assert!(!failed_dir.join("DEMONSTRATION.json").exists());
        assert!(crate::session::revive_session(&session));

        let next_dir = temp_dir("writer-failure-next");
        manager
            .start_with(
                DemonstrationConfig {
                    pid: 42,
                    window_id: 7,
                    output_dir: next_dir.clone(),
                    owner: Some(session.clone()),
                },
                |config| {
                    Ok(DemonstrationSession {
                        state: DemonstrationState::Artifacts {
                            evidence: CaptureEvidence::default(),
                            action_count: 0,
                        },
                        output_dir: config.output_dir,
                        owner: config.owner,
                        pid: config.pid,
                        window_id: config.window_id,
                        started_at_ms: 0,
                    })
                },
            )
            .expect("terminal writer failure must release start eligibility");
        assert!(manager.stop_owner(&session).unwrap().is_some());
        assert!(next_dir.join("DEMONSTRATION.json").exists());
    }
}
