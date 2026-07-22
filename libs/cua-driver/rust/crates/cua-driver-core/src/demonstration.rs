//! Human demonstration lifecycle and artifact writing.
//!
//! Human observations are separate from executable tool-call recording.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use serde_json::json;

use crate::demonstration_artifacts::{self, Artifacts};
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
        let mut active = self.active.lock().unwrap();
        if active.is_some() {
            anyhow::bail!("a demonstration is already active");
        }
        let output_dir = config.output_dir.clone();
        *active = Some(DemonstrationSession::start(config)?);
        Ok(output_dir)
    }

    pub fn stop(&self) -> anyhow::Result<Option<DemonstrationResult>> {
        self.take_if(|_| true)
            .map(DemonstrationSession::finish)
            .transpose()
    }

    pub fn stop_owner(&self, owner: &str) -> anyhow::Result<Option<DemonstrationResult>> {
        self.take_if(|session| session.owner.as_deref() == Some(owner))
            .map(DemonstrationSession::finish)
            .transpose()
    }

    fn take_if(
        &self,
        predicate: impl FnOnce(&DemonstrationSession) -> bool,
    ) -> Option<DemonstrationSession> {
        let mut active = self.active.lock().unwrap();
        if active.as_ref().is_some_and(predicate) {
            active.take()
        } else {
            None
        }
    }
}

impl Default for DemonstrationManager {
    fn default() -> Self {
        Self::new()
    }
}

struct DemonstrationSession {
    capture: input_capture::Demonstration,
    writer: std::thread::JoinHandle<anyhow::Result<usize>>,
    output_dir: PathBuf,
    owner: Option<String>,
}

impl DemonstrationSession {
    fn start(config: DemonstrationConfig) -> anyhow::Result<Self> {
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
            capture,
            writer,
            output_dir: config.output_dir,
            owner: config.owner,
        })
    }

    fn finish(self) -> anyhow::Result<DemonstrationResult> {
        self.capture.stop();
        self.writer
            .join()
            .map_err(|_| anyhow::anyhow!("demonstration writer panicked"))??;
        let artifacts = demonstration_artifacts::write(&self.output_dir)?;
        Ok(DemonstrationResult {
            output_dir: self.output_dir,
            artifacts,
        })
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

fn write_events(
    output_dir: &Path,
    pid: i64,
    window_id: u64,
    events: std::sync::mpsc::Receiver<input_capture::HumanEvent>,
) -> anyhow::Result<usize> {
    let mut count = 0;
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
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_capture::{Button, HumanEvent};

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

        assert_eq!(write_events(&output_dir, 42, 7, receiver).unwrap(), 1);
        let action = std::fs::read_to_string(output_dir.join("turn-00001/action.json")).unwrap();
        assert!(action.contains("human_click"));
        assert!(action.contains("\"source\": \"human\""));

        let artifacts = demonstration_artifacts::write(&output_dir).unwrap();
        assert_eq!(artifacts.summary.action_count, 1);
    }

    #[test]
    fn rejects_nonempty_output_directory() {
        let output_dir = temp_dir("nonempty");
        std::fs::write(output_dir.join("existing"), "data").unwrap();
        assert!(prepare_output_dir(&output_dir).is_err());
    }

    #[test]
    fn empty_demonstration_still_has_artifacts() {
        let output_dir = temp_dir("empty");
        let artifacts = demonstration_artifacts::write(&output_dir).unwrap();
        assert_eq!(artifacts.summary.action_count, 0);
        assert!(artifacts.trajectory_md.exists());
        assert!(artifacts.summary_json.exists());
    }
}
