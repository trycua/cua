//! Append-only evidence files owned by the fixture.
//!
//! Two channels, on purpose:
//!
//! * the JSONL journal carries the causal timing rows, and
//! * the state file carries only application-owned state.
//!
//! The runner asserts the mutation from the state file (and from the window
//! title), never from the timing rows, so a bug in the timing path cannot
//! manufacture a passing state assertion, and a bug in the state path cannot
//! manufacture a presented row.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::sample::{Sample, JOURNAL_SCHEMA};

/// One JSONL evidence file. Created exclusively: a stale journal is an error
/// rather than something to append to.
#[derive(Debug)]
pub struct Journal {
    file: File,
}

impl Journal {
    pub fn create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(Self { file })
    }

    /// Write one journal record. `fields` must be a JSON object; its keys are
    /// merged next to the schema, kind, and timestamp.
    pub fn record(&mut self, kind: &str, time_ns: u64, fields: Value) -> io::Result<()> {
        let mut line = json!({
            "schema": JOURNAL_SCHEMA,
            "kind": kind,
            "time_ns": time_ns,
        });
        if let (Some(target), Value::Object(extra)) = (line.as_object_mut(), fields) {
            for (key, value) in extra {
                target.insert(key, value);
            }
        }
        self.write_line(&line)
    }

    pub fn sample(&mut self, sample: &Sample) -> io::Result<()> {
        let value = serde_json::to_value(sample).map_err(io::Error::other)?;
        self.write_line(&value)
    }

    fn write_line(&mut self, value: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        // Line-buffered by hand: the runner polls this file while the fixture
        // is still running, so every record must be readable immediately.
        self.file.flush()
    }
}

/// Replace the application state file atomically, so a concurrent reader sees
/// either the previous state or the new one and never a partial write.
pub fn write_state(path: &Path, state: &Value) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut bytes = serde_json::to_vec(state).map_err(io::Error::other)?;
    bytes.push(b'\n');
    {
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{finalize, Feedback, Pending, Region, CLOCK_MONOTONIC_ID};
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "cua-wayland-presentation-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    #[test]
    fn records_carry_the_schema_kind_and_timestamp_one_per_line() {
        let directory = scratch("journal");
        let path = directory.join("journal.jsonl");
        let mut journal = Journal::create(&path).expect("create journal");
        journal
            .record("startup", 10, json!({"title": "CuaTestHarness Presentation"}))
            .expect("startup record");
        journal
            .record("input", 20, json!({"event": "button", "button": 272}))
            .expect("input record");
        let contents = std::fs::read_to_string(&path).expect("read journal");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).expect("first line");
        assert_eq!(first["schema"], JOURNAL_SCHEMA);
        assert_eq!(first["kind"], "startup");
        assert_eq!(first["time_ns"], 10);
        assert_eq!(first["title"], "CuaTestHarness Presentation");
        let second: Value = serde_json::from_str(lines[1]).expect("second line");
        assert_eq!(second["kind"], "input");
        assert_eq!(second["button"], 272);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_stale_journal_is_refused_rather_than_appended_to() {
        let directory = scratch("stale");
        let path = directory.join("journal.jsonl");
        std::fs::write(&path, b"{}\n").expect("seed stale journal");
        let error = Journal::create(&path).expect_err("stale journal must be refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn samples_are_written_as_whole_rows() {
        let directory = scratch("samples");
        let path = directory.join("journal.jsonl");
        let mut journal = Journal::create(&path).expect("create journal");
        let pending = Pending {
            action: "click".to_owned(),
            sequence: 3,
            region: Region::Active,
            input_received_ns: 1_000,
            state_changed_ns: Some(1_100),
            surface_commit_ns: Some(1_500),
            counter_before: 2,
            counter_after: 3,
            supersede_probe: false,
        };
        let sample = finalize(
            &pending,
            Feedback::Presented {
                presented_ns: 9_000,
                refresh_ns: 16_666_666,
                sequence: 7,
                flags: 0x1,
                clock_id: Some(CLOCK_MONOTONIC_ID),
            },
            1_000_000_000,
        );
        journal.sample(&sample).expect("write sample");
        let contents = std::fs::read_to_string(&path).expect("read journal");
        let row: Value = serde_json::from_str(contents.lines().next().expect("row")).expect("row");
        assert_eq!(row["kind"], "sample");
        assert_eq!(row["sequence"], 3);
        assert_eq!(row["fixture_outcome"], "verified");
        assert_eq!(row["presented_ns"], 9_000);
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn state_replacement_leaves_no_partial_file_and_no_temporary() {
        let directory = scratch("state");
        let path = directory.join("state.json");
        write_state(&path, &json!({"counter": 1, "color": "#1e6fb8"})).expect("first state");
        write_state(&path, &json!({"counter": 2, "color": "#b8541e"})).expect("second state");
        let state: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read state"))
                .expect("parse state");
        assert_eq!(state["counter"], 2);
        assert_eq!(state["color"], "#b8541e");
        assert!(!path.with_extension("tmp").exists());
        std::fs::remove_dir_all(&directory).ok();
    }
}
