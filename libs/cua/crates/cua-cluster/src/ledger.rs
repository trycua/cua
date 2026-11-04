//! The host's job ledger: an append-only JSON-lines file of job records
//! (`<dir>/ledger.jsonl`) plus `<dir>/meta.json` (the ledger id and the
//! runner's epoch).
//!
//! One writer thread owns the file. It takes batches of records, gives
//! each the next sequence number, appends them, syncs once per group
//! (group commit), and only then publishes them to the in-memory change
//! feed, in sequence order, so a reader of the feed never sees a record
//! the disk does not have (unless the write failed and the batch said a
//! failed write may still be published: a job's result is reported even
//! when the disk is full; its acceptance is not).
//!
//! A write that fails part-way is cut back to the last whole line, so the
//! file never holds a torn record. Opening the ledger compacts it: the
//! latest record of each job, without the ended ones older than the
//! retention.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::{HostJobState, now_ms};

/// One job's record as the ledger and the feed keep it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    /// The job.
    pub job_id: String,
    /// The newest attempt seen.
    pub attempt: u32,
    /// Its state.
    pub state: StateWord,
    /// Exit code of the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Ran out of time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// Tail of stdout.
    #[serde(default, with = "b64", skip_serializing_if = "Vec::is_empty")]
    pub stdout: Vec<u8>,
    /// Tail of stderr.
    #[serde(default, with = "b64", skip_serializing_if = "Vec::is_empty")]
    pub stderr: Vec<u8>,
    /// The result file.
    #[serde(default, with = "b64", skip_serializing_if = "Vec::is_empty")]
    pub result: Vec<u8>,
    /// What went wrong.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Ledger sequence number (set by the writer).
    #[serde(default)]
    pub seq: u64,
    /// The Space's name on this host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub space: String,
    /// Accepted (host clock, Unix ms).
    #[serde(default)]
    pub accepted_at_ms: i64,
    /// Ended (0 while running).
    #[serde(default)]
    pub finished_at_ms: i64,
    /// Space create, command, Space delete (ms).
    #[serde(default)]
    pub create_ms: u32,
    /// Command run time (ms).
    #[serde(default)]
    pub run_ms: u32,
    /// Space delete time (ms).
    #[serde(default)]
    pub delete_ms: u32,
    /// Output was cut to its tail.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Reserved CPU (thousandths), while running.
    #[serde(default)]
    pub millicpus: u32,
    /// Reserved memory (MiB), while running.
    #[serde(default)]
    pub memory_mb: u64,
    /// The kind it asked for (empty: auto).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Who delivered it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scheduler: String,
}

/// [`HostJobState`] as it is stored (a default for `Default`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateWord(pub HostJobState);

impl Default for StateWord {
    fn default() -> Self {
        Self(HostJobState::Running)
    }
}

impl JobRecord {
    /// The state.
    pub fn state(&self) -> HostJobState {
        self.state.0
    }

    /// Bytes of output held.
    pub fn output_bytes(&self) -> usize {
        self.stdout.len() + self.stderr.len() + self.result.len()
    }

    /// The record on the wire.
    pub fn to_pb(&self, with_output: bool) -> cua_proto::env::v1::HostJob {
        cua_proto::env::v1::HostJob {
            job_id: self.job_id.clone(),
            attempt: self.attempt,
            state: self.state().as_str().into(),
            exit_code: self.exit_code,
            timed_out: self.timed_out,
            stdout: if with_output {
                self.stdout.clone()
            } else {
                vec![]
            },
            stderr: if with_output {
                self.stderr.clone()
            } else {
                vec![]
            },
            result: if with_output {
                self.result.clone()
            } else {
                vec![]
            },
            error: self.error.clone(),
            seq: self.seq,
            space: if self.space.is_empty() {
                String::new()
            } else {
                format!("local:{}", self.space)
            },
            accepted_at_ms: self.accepted_at_ms,
            finished_at_ms: self.finished_at_ms,
            create_ms: self.create_ms,
            run_ms: self.run_ms,
            delete_ms: self.delete_ms,
            truncated: self.truncated,
        }
    }
}

pub(crate) mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

/// `<dir>/meta.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    /// Random when the ledger is created.
    pub ledger_id: String,
    /// One more on every open.
    pub epoch: u64,
    /// The newest sequence number ever issued, as of the last open. Expired
    /// records leave the file when it is compacted, so the file alone
    /// would let the numbering start again below a coordinator's feed
    /// cursor, and that coordinator would never see another record.
    #[serde(default)]
    pub last_seq: u64,
}

/// What [`load`] found.
pub struct Loaded {
    /// The ledger's identity, with this open's epoch.
    pub meta: Meta,
    /// The latest record of each job kept, in sequence order.
    pub records: Vec<JobRecord>,
    /// The newest sequence number on disk.
    pub last_seq: u64,
}

/// The ledger file.
pub fn ledger_path(dir: &Path) -> PathBuf {
    dir.join("ledger.jsonl")
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("meta.json")
}

/// Reads (and compacts) the ledger in `dir`, creating it when missing, and
/// counts this open in its epoch.
pub fn load(dir: &Path, retention: Duration) -> std::io::Result<Loaded> {
    std::fs::create_dir_all(dir)?;
    let path = ledger_path(dir);
    let mut meta: Meta = std::fs::read(meta_path(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|m: &Meta| !m.ledger_id.is_empty())
        .unwrap_or_else(|| Meta {
            ledger_id: format!("{:016x}", rand::random::<u64>()),
            epoch: 0,
            last_seq: 0,
        });
    meta.epoch += 1;
    let regular = std::fs::metadata(&path).is_ok_and(|m| m.is_file());
    let mut latest: HashMap<String, JobRecord> = HashMap::new();
    let mut last_seq = 0;
    let mut lines = 0usize;
    if regular {
        let f = std::fs::File::open(&path)?;
        for line in std::io::BufReader::new(f).lines() {
            let Ok(line) = line else { break };
            lines += 1;
            let Ok(rec) = serde_json::from_str::<JobRecord>(&line) else {
                tracing::warn!(path = %path.display(), "skipped an unreadable job ledger line");
                continue;
            };
            last_seq = last_seq.max(rec.seq);
            match latest.get(&rec.job_id) {
                Some(old) if old.seq > rec.seq => {}
                _ => {
                    latest.insert(rec.job_id.clone(), rec);
                }
            }
        }
    }
    last_seq = last_seq.max(meta.last_seq);
    meta.last_seq = last_seq;
    write_atomic(&meta_path(dir), &serde_json::to_vec(&meta)?)?;
    let cutoff = now_ms() - retention.as_millis() as i64;
    let mut records: Vec<JobRecord> = latest
        .into_values()
        .filter(|r| !(r.state().is_terminal() && r.finished_at_ms > 0 && r.finished_at_ms < cutoff))
        .collect();
    records.sort_by_key(|r| r.seq);
    // Compact when it pays: superseded or expired lines.
    if regular && lines > records.len() {
        let mut buf = Vec::new();
        for r in &records {
            serde_json::to_writer(&mut buf, r)?;
            buf.push(b'\n');
        }
        write_atomic(&path, &buf)?;
    }
    Ok(Loaded {
        meta,
        records,
        last_seq,
    })
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Records to append as one group.
pub(crate) struct Batch {
    pub records: Vec<JobRecord>,
    /// Publish only if the write succeeded (an acceptance); otherwise
    /// publish either way (a result).
    pub durable_required: bool,
    pub done: tokio::sync::oneshot::Sender<Result<Vec<u64>, String>>,
}

/// Where the writer publishes: called under no lock of the writer's own,
/// in sequence order, after the write (or its failure).
pub(crate) type Publish = Arc<dyn Fn(Vec<JobRecord>, Option<String>) + Send + Sync>;

/// The writer thread's handle.
#[derive(Clone)]
pub(crate) struct Writer {
    tx: std::sync::mpsc::Sender<Batch>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl Writer {
    /// Starts the writer for `dir`'s ledger, numbering from `last_seq + 1`.
    pub fn start(dir: &Path, last_seq: u64, publish: Publish) -> std::io::Result<Self> {
        let path = ledger_path(dir);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let (tx, rx) = std::sync::mpsc::channel::<Batch>();
        let last_error = Arc::new(Mutex::new(None));
        let errors = last_error.clone();
        std::thread::Builder::new()
            .name("cua-job-ledger".into())
            .spawn(move || {
                let mut seq = last_seq;
                while let Ok(first) = rx.recv() {
                    let mut group = vec![first];
                    while group.len() < 256 {
                        match rx.try_recv() {
                            Ok(b) => group.push(b),
                            Err(_) => break,
                        }
                    }
                    let mut buf = Vec::new();
                    let mut seqs: Vec<Vec<u64>> = Vec::with_capacity(group.len());
                    for b in &mut group {
                        let mut mine = Vec::with_capacity(b.records.len());
                        for r in &mut b.records {
                            seq += 1;
                            r.seq = seq;
                            mine.push(seq);
                            if serde_json::to_writer(&mut buf, &*r).is_ok() {
                                buf.push(b'\n');
                            }
                        }
                        seqs.push(mine);
                    }
                    let written = append(&mut file, &buf);
                    let failure = written
                        .err()
                        .map(|e| format!("job ledger {}: {e}", path.display()));
                    *errors.lock().expect("ledger error") = failure.clone();
                    if let Some(f) = &failure {
                        tracing::warn!("{f}");
                    }
                    for (b, mine) in group.into_iter().zip(seqs) {
                        let ok = failure.is_none();
                        if ok || !b.durable_required {
                            publish(b.records, failure.clone());
                        }
                        let _ = b.done.send(if ok {
                            Ok(mine)
                        } else {
                            Err(failure.clone().unwrap_or_default())
                        });
                    }
                }
            })?;
        Ok(Self { tx, last_error })
    }

    /// Appends `records` (numbered by the writer) and waits for the write.
    pub async fn append(
        &self,
        records: Vec<JobRecord>,
        durable_required: bool,
    ) -> Result<Vec<u64>, String> {
        let (done, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Batch {
                records,
                durable_required,
                done,
            })
            .map_err(|_| "the job ledger writer stopped".to_string())?;
        rx.await
            .map_err(|_| "the job ledger writer stopped".to_string())?
    }

    /// The last write's failure, if it failed.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().expect("ledger error").clone()
    }
}

/// Appends `buf` and syncs it; on failure cuts the file back to where it
/// was, so no torn line stays.
fn append(file: &mut std::fs::File, buf: &[u8]) -> std::io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let regular = file.metadata().map(|m| m.is_file()).unwrap_or(false);
    let before = if regular { file.metadata()?.len() } else { 0 };
    let r = file.write_all(buf).and_then(|_| file.sync_data());
    if r.is_err() && regular {
        let _ = file.set_len(before);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, state: HostJobState, finished: i64) -> JobRecord {
        JobRecord {
            job_id: id.into(),
            attempt: 1,
            state: StateWord(state),
            finished_at_ms: finished,
            stdout: b"out".to_vec(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn records_survive_a_reopen_and_compact() {
        let dir = tempfile::tempdir().unwrap();
        let l = load(dir.path(), Duration::from_secs(3600)).unwrap();
        assert_eq!((l.meta.epoch, l.last_seq, l.records.len()), (1, 0, 0));
        let id = l.meta.ledger_id.clone();
        let published = Arc::new(Mutex::new(Vec::new()));
        let p = published.clone();
        let w = Writer::start(
            dir.path(),
            0,
            Arc::new(move |r, _| p.lock().unwrap().extend(r)),
        )
        .unwrap();
        let seqs = w
            .append(
                vec![
                    rec("a", HostJobState::Running, 0),
                    rec("b", HostJobState::Running, 0),
                ],
                true,
            )
            .await
            .unwrap();
        assert_eq!(seqs, vec![1, 2]);
        w.append(vec![rec("a", HostJobState::Succeeded, now_ms())], false)
            .await
            .unwrap();
        // An ended job older than the retention is dropped on open.
        w.append(vec![rec("old", HostJobState::Failed, 1)], false)
            .await
            .unwrap();
        assert_eq!(published.lock().unwrap().len(), 4);
        drop(w);
        let l = load(dir.path(), Duration::from_secs(3600)).unwrap();
        assert_eq!(l.meta.ledger_id, id);
        assert_eq!(l.meta.epoch, 2);
        assert_eq!(l.last_seq, 4);
        let got: Vec<(String, HostJobState, u64)> = l
            .records
            .iter()
            .map(|r| (r.job_id.clone(), r.state(), r.seq))
            .collect();
        assert_eq!(
            got,
            vec![
                ("b".into(), HostJobState::Running, 2),
                ("a".into(), HostJobState::Succeeded, 3)
            ]
        );
        assert_eq!(l.records[1].stdout, b"out");
        let lines = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        assert_eq!(lines.lines().count(), 2, "compacted");
        // Numbering never goes back below what a coordinator may have read:
        // the expired record (seq 4) is gone, the next open still says 4.
        let l = load(dir.path(), Duration::from_secs(3600)).unwrap();
        assert_eq!(l.last_seq, 4);
        std::fs::write(ledger_path(dir.path()), b"").unwrap();
        let l = load(dir.path(), Duration::from_secs(3600)).unwrap();
        assert_eq!(l.last_seq, 4, "even with the file emptied");
    }

    /// A full disk: the write fails, an acceptance is not published, a
    /// result is, and the file holds no torn line.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_full_disk_refuses_acceptances_but_reports_results() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/dev/full", ledger_path(dir.path())).unwrap();
        let l = load(dir.path(), Duration::from_secs(60)).unwrap();
        let published = Arc::new(Mutex::new(Vec::new()));
        let p = published.clone();
        let w = Writer::start(
            dir.path(),
            l.last_seq,
            Arc::new(move |r, e: Option<String>| {
                assert!(e.is_some());
                p.lock().unwrap().extend(r)
            }),
        )
        .unwrap();
        let e = w
            .append(vec![rec("a", HostJobState::Running, 0)], true)
            .await
            .unwrap_err();
        assert!(e.contains("No space left"), "{e}");
        assert!(published.lock().unwrap().is_empty());
        assert!(w.last_error().is_some());
        w.append(vec![rec("b", HostJobState::Succeeded, now_ms())], false)
            .await
            .unwrap_err();
        assert_eq!(published.lock().unwrap().len(), 1);
    }
}
