// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `.cua/setup` and `.cua/resume`: a Space's own preparation scripts.
//!
//! - **`setup`** runs when a member's Space is created, and when a durable
//!   Space is re-placed from scratch (`restored: "from-scratch"`): install
//!   what the work needs (packages, a checkout, a build). Timeout 20 min.
//! - **`resume`** runs when a suspended or paused Space resumes, and before
//!   a durable re-drive on a Space that survived: restart what a resume
//!   loses (a dev server, a database). Timeout 60 s.
//!
//! Lookup, first found wins: `<dir>/.cua/<hook>` in each of the agent's
//! working directories in order (its repository root, else `<home>/work`),
//! then the Space's Volume folder `spaces/<S>/.cua/<hook>` (copied into the
//! Space first). A hook always runs **inside the Space**, through its
//! spacesd, as the Space's user, with `/bin/sh`; never on the host.
//!
//! The result is a [`HookReport`]: whether it worked, and the last 40 lines
//! of its output (stdout and stderr together) with anything that looks like
//! a secret blanked, ready for a system note in the Space's thread
//! ([`HookReport::note`]). A failed `setup` means the member is marked
//! failed and gets no tasks.

use std::time::{Duration, Instant};

use serde::Serialize;

use cua_spaces::{Error, Result};
use cua_spacesd_client::{Command, ProcessEvent, SpacesdClient, UploadOptions, pb};
use cua_volume::Backend;

/// Lines of output a report keeps.
pub const TAIL_LINES: usize = 40;
/// Bytes of output kept while a hook runs (the tail is cut from these).
const KEEP_BYTES: usize = 256 * 1024;

/// Which hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    Setup,
    Resume,
}

impl HookKind {
    /// The file name under `.cua/`.
    pub fn name(self) -> &'static str {
        match self {
            HookKind::Setup => "setup",
            HookKind::Resume => "resume",
        }
    }

    /// How long it may run.
    pub fn timeout(self) -> Duration {
        match self {
            HookKind::Setup => Duration::from_secs(20 * 60),
            HookKind::Resume => Duration::from_secs(60),
        }
    }
}

/// Where to look for a hook.
#[derive(Clone, Debug, Default)]
pub struct Lookup {
    /// The agent's working directories in the Space, in order (its
    /// repository root first, then `<home>/work`).
    pub workdirs: Vec<String>,
    /// The Space's Volume folder: a Space id or a durable Space's stable
    /// name (see [`cua_volume::path::space_folder_name`]); `None`: do not
    /// look on the Volume.
    pub volume_space: Option<String>,
    /// Where a hook from the Volume is put in the Space before it runs
    /// (default `/tmp/cua-hooks`).
    pub stage_dir: Option<String>,
}

/// A hook that was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The script's path in the Space.
    pub path: String,
    /// Where it came from, for people: the Space path, or
    /// `volume:spaces/<S>/.cua/<hook>`.
    pub source: String,
    /// Its working directory.
    pub cwd: String,
}

/// What a hook did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookReport {
    pub kind: HookKind,
    pub source: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last [`TAIL_LINES`] lines of output, secrets blanked.
    pub tail: Vec<String>,
    /// Lines in which something that looked like a secret was blanked.
    pub redacted: usize,
    pub millis: u64,
}

impl HookReport {
    /// The text of the system thread note (author `system:hooks`).
    pub fn note(&self) -> String {
        let what = format!(".cua/{}", self.kind.name());
        let head = if self.ok {
            format!(
                "{what} finished ({} s, from {}).",
                self.millis / 1000,
                self.source
            )
        } else if self.timed_out {
            format!(
                "{what} timed out after {} s (from {}).",
                self.kind.timeout().as_secs(),
                self.source
            )
        } else {
            format!(
                "{what} failed with exit code {} (from {}).",
                self.exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "none".into()),
                self.source
            )
        };
        if self.tail.is_empty() {
            return head;
        }
        format!(
            "{head}\nLast {} lines of output:\n{}",
            self.tail.len(),
            self.tail.join("\n")
        )
    }
}

/// The last `n` lines of `out`, with secrets blanked (and how many lines
/// had one).
pub fn tail_redacted(out: &[u8], n: usize) -> (Vec<String>, usize) {
    let text = String::from_utf8_lossy(out);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let lines: Vec<&str> = if text.is_empty() {
        vec![]
    } else {
        text.split('\n').collect()
    };
    let mut redacted = 0;
    let tail = lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| {
            let mut b = l.trim_end_matches('\r').as_bytes().to_vec();
            if !cua_volume::scan::redact_line(&mut b).is_empty() {
                redacted += 1;
            }
            String::from_utf8_lossy(&b).into_owned()
        })
        .collect();
    (tail, redacted)
}

/// How a hook ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookOutput {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// stdout and stderr together (the end of it, bounded).
    pub output: Vec<u8>,
}

/// The Space a hook runs in.
#[async_trait::async_trait]
pub trait HookSpace: Send + Sync {
    /// Whether `path` exists in the Space.
    async fn exists(&self, path: &str) -> Result<bool>;
    /// Writes an executable script at `path` in the Space.
    async fn put_script(&self, path: &str, body: Vec<u8>) -> Result<()>;
    /// Runs `/bin/sh <script>` in `cwd`, killed after `timeout`.
    async fn run(&self, script: &str, cwd: &str, timeout: Duration) -> Result<HookOutput>;
}

#[async_trait::async_trait]
impl HookSpace for SpacesdClient {
    async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(cua_spacesd_client::Error::PathNotFound(_)) => Ok(false),
            Err(e) => Err(Error::Env(e)),
        }
    }

    async fn put_script(&self, path: &str, body: Vec<u8>) -> Result<()> {
        self.upload(
            path,
            body,
            UploadOptions {
                mode: pb::WriteMode::Overwrite,
                create_parents: true,
                permissions: 0o700,
                ..Default::default()
            },
        )
        .await
        .map_err(Error::Env)?;
        Ok(())
    }

    async fn run(&self, script: &str, cwd: &str, timeout: Duration) -> Result<HookOutput> {
        // One stream for both: the note shows them in the order they came.
        let cmd = Command::new("/bin/sh")
            .arg("-c")
            .arg("exec /bin/sh \"$1\" 2>&1")
            .arg("cua-hook")
            .arg(script)
            .cwd(cwd)
            .timeout(timeout);
        let mut h = self.spawn(cmd).await.map_err(Error::Env)?;
        let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(5);
        let mut out = HookOutput::default();
        let keep = |out: &mut HookOutput, data: &[u8]| {
            out.output.extend_from_slice(data);
            if out.output.len() > KEEP_BYTES {
                let cut = out.output.len() - KEEP_BYTES;
                out.output.drain(..cut);
            }
        };
        loop {
            match tokio::time::timeout_at(deadline, h.next_event()).await {
                Err(_) => {
                    // The server did not enforce the timeout: kill it here.
                    let _ = h.signal_group(pb::Signal::Kill).await;
                    out.timed_out = true;
                    break;
                }
                Ok(Err(e)) => return Err(Error::Env(e)),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(ProcessEvent::Stdout { data, .. })))
                | Ok(Ok(Some(ProcessEvent::Stderr { data, .. }))) => keep(&mut out, &data),
                Ok(Ok(Some(ProcessEvent::Exit(s)))) => {
                    out.exit_code = s.code;
                    out.timed_out |= s.timed_out;
                    if let Some(e) = s.error.filter(|e| !e.is_empty()) {
                        keep(&mut out, format!("{e}\n").as_bytes());
                    }
                    break;
                }
                Ok(Ok(Some(_))) => {}
            }
        }
        Ok(out)
    }
}

/// The Volume key of the Space's own hook.
pub fn volume_key(space: &str, kind: HookKind) -> String {
    format!(
        "{}.cua/{}",
        cua_volume::path::space_folder(space),
        kind.name()
    )
}

/// Finds `kind` (see the module docs for the order). A hook from the Volume
/// is copied into the Space.
pub async fn find(
    space: &dyn HookSpace,
    backend: Option<&dyn Backend>,
    kind: HookKind,
    lookup: &Lookup,
) -> Result<Option<Found>> {
    let mut first_dir = None;
    for dir in &lookup.workdirs {
        let dir = dir.trim_end_matches('/');
        if dir.is_empty() {
            continue;
        }
        let path = format!("{dir}/.cua/{}", kind.name());
        if space.exists(&path).await? {
            return Ok(Some(Found {
                path: path.clone(),
                source: path,
                cwd: dir.to_string(),
            }));
        }
        if first_dir.is_none() && space.exists(dir).await? {
            first_dir = Some(dir.to_string());
        }
    }
    let (Some(b), Some(s)) = (backend, &lookup.volume_space) else {
        return Ok(None);
    };
    let key = volume_key(s, kind);
    let body = match b.get(&key, None).await {
        Ok((body, _)) => body,
        Err(cua_volume::Error::NotFound(_)) => return Ok(None),
        Err(e) => return Err(crate::drive_err(e)),
    };
    let stage = lookup
        .stage_dir
        .clone()
        .unwrap_or_else(|| "/tmp/cua-hooks".into());
    let path = format!(
        "{}/{}-{}",
        stage.trim_end_matches('/'),
        cua_volume::path::space_folder_name(s),
        kind.name()
    );
    space.put_script(&path, body).await?;
    Ok(Some(Found {
        path,
        source: format!("volume:{key}"),
        cwd: first_dir.unwrap_or_else(|| "/".into()),
    }))
}

/// Finds and runs `kind` in the Space. `None`: there is no such hook.
/// `timeout` overrides [`HookKind::timeout`].
pub async fn run(
    space: &dyn HookSpace,
    backend: Option<&dyn Backend>,
    kind: HookKind,
    lookup: &Lookup,
    timeout: Option<Duration>,
) -> Result<Option<HookReport>> {
    let Some(found) = find(space, backend, kind, lookup).await? else {
        return Ok(None);
    };
    let t0 = Instant::now();
    let out = space
        .run(&found.path, &found.cwd, timeout.unwrap_or(kind.timeout()))
        .await?;
    let (tail, redacted) = tail_redacted(&out.output, TAIL_LINES);
    Ok(Some(HookReport {
        kind,
        source: found.source,
        ok: !out.timed_out && out.exit_code == Some(0),
        exit_code: out.exit_code,
        timed_out: out.timed_out,
        tail,
        redacted,
        millis: t0.elapsed().as_millis() as u64,
    }))
}

/// Runs `.cua/setup` in a durable Space's current copy (`id`:
/// `direct:<host>:<port>` with its `token`, or `relay:<machine>`), looked
/// up in `<home>/work` and on the Volume under the durable Space's `name`.
/// `None`: it has no setup hook. For the session coordinator's setup step.
pub async fn setup_durable_space(
    spaces: &cua_spaces::Spaces,
    name: &str,
    id: &str,
    token: &str,
) -> Result<Option<HookReport>> {
    if id.starts_with("direct:") {
        spaces
            .add(id, Some(token.to_string()), Some(name.to_string()))
            .await?;
    } else {
        let _ = spaces.relay_machines().await;
    }
    let space = spaces.space(id).await?;
    let guest = space.spacesd()?.clone();
    let out = guest
        .run(Command::shell("printf %s \"$HOME\"").timeout(Duration::from_secs(30)))
        .await
        .map_err(Error::Env)?;
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let lookup = Lookup {
        workdirs: if home.starts_with('/') {
            vec![format!("{home}/work")]
        } else {
            vec![]
        },
        volume_space: Some(name.to_string()),
        stage_dir: None,
    };
    let backend = spaces
        .extension::<crate::DriveExtension>()
        .map(|e| e.drive().backend());
    run(&guest, backend.as_deref(), HookKind::Setup, &lookup, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    /// A Space whose files are a set and whose hooks "run" by script.
    #[derive(Default)]
    struct FakeSpace {
        files: Mutex<BTreeSet<String>>,
        scripts: Mutex<BTreeMap<String, Vec<u8>>>,
        ran: Mutex<Vec<(String, String, Duration)>>,
        result: Mutex<HookOutput>,
    }

    #[async_trait::async_trait]
    impl HookSpace for FakeSpace {
        async fn exists(&self, path: &str) -> Result<bool> {
            Ok(self.files.lock().unwrap().contains(path))
        }
        async fn put_script(&self, path: &str, body: Vec<u8>) -> Result<()> {
            self.scripts.lock().unwrap().insert(path.into(), body);
            Ok(())
        }
        async fn run(&self, script: &str, cwd: &str, timeout: Duration) -> Result<HookOutput> {
            self.ran
                .lock()
                .unwrap()
                .push((script.into(), cwd.into(), timeout));
            Ok(self.result.lock().unwrap().clone())
        }
    }

    fn space(files: &[&str]) -> FakeSpace {
        let s = FakeSpace::default();
        s.files
            .lock()
            .unwrap()
            .extend(files.iter().map(|f| f.to_string()));
        *s.result.lock().unwrap() = HookOutput {
            exit_code: Some(0),
            ..Default::default()
        };
        s
    }

    fn lookup() -> Lookup {
        Lookup {
            workdirs: vec![
                "/home/cua/repo".into(),
                "/home/cua/agents/claude/work".into(),
            ],
            volume_space: Some("local:dev".into()),
            stage_dir: Some("/home/cua/.cua/hooks".into()),
        }
    }

    #[tokio::test]
    async fn the_repo_comes_first_then_the_work_dir_then_the_volume() {
        let d = tempfile::tempdir().unwrap();
        let b = cua_volume::fs::FsBackend::new(d.path());
        b.put(
            &volume_key("local:dev", HookKind::Setup),
            b"echo from volume\n".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();
        let all = space(&[
            "/home/cua/repo",
            "/home/cua/repo/.cua/setup",
            "/home/cua/agents/claude/work/.cua/setup",
        ]);
        let f = find(&all, Some(&b), HookKind::Setup, &lookup())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.path, "/home/cua/repo/.cua/setup");
        assert_eq!(f.cwd, "/home/cua/repo");
        let work = space(&[
            "/home/cua/agents/claude/work",
            "/home/cua/agents/claude/work/.cua/setup",
        ]);
        let f = find(&work, Some(&b), HookKind::Setup, &lookup())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.path, "/home/cua/agents/claude/work/.cua/setup");
        assert_eq!(f.cwd, "/home/cua/agents/claude/work");
        // Only on the Volume: copied into the Space, run in the first
        // working directory that exists.
        let vol = space(&["/home/cua/agents/claude/work"]);
        let f = find(&vol, Some(&b), HookKind::Setup, &lookup())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.path, "/home/cua/.cua/hooks/local-dev-setup");
        assert_eq!(f.source, "volume:spaces/local-dev/.cua/setup");
        assert_eq!(f.cwd, "/home/cua/agents/claude/work");
        assert_eq!(
            vol.scripts.lock().unwrap()[&f.path],
            b"echo from volume\n".to_vec()
        );
        // No resume hook anywhere.
        assert!(
            find(&vol, Some(&b), HookKind::Resume, &lookup())
                .await
                .unwrap()
                .is_none()
        );
        // Without the Volume, nothing.
        assert!(
            find(&vol, None, HookKind::Setup, &lookup())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn timeouts_are_20_minutes_and_60_seconds_and_a_timeout_fails() {
        assert_eq!(HookKind::Setup.timeout(), Duration::from_secs(1200));
        assert_eq!(HookKind::Resume.timeout(), Duration::from_secs(60));
        let s = space(&["/w/.cua/resume"]);
        let lk = Lookup {
            workdirs: vec!["/w".into()],
            ..Default::default()
        };
        let r = run(&s, None, HookKind::Resume, &lk, None)
            .await
            .unwrap()
            .unwrap();
        assert!(r.ok);
        assert_eq!(s.ran.lock().unwrap()[0].2, Duration::from_secs(60));
        *s.result.lock().unwrap() = HookOutput {
            exit_code: None,
            timed_out: true,
            output: b"starting\n".to_vec(),
        };
        let r = run(&s, None, HookKind::Resume, &lk, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!r.ok && r.timed_out);
        assert_eq!(
            r.note(),
            ".cua/resume timed out after 60 s (from /w/.cua/resume).\nLast 1 lines of output:\nstarting"
        );
        *s.result.lock().unwrap() = HookOutput {
            exit_code: Some(3),
            ..Default::default()
        };
        let r = run(&s, None, HookKind::Resume, &lk, None)
            .await
            .unwrap()
            .unwrap();
        assert!(!r.ok);
        assert_eq!(
            r.note(),
            ".cua/resume failed with exit code 3 (from /w/.cua/resume)."
        );
    }

    #[test]
    fn the_note_keeps_the_last_40_lines_with_secrets_blanked() {
        let key = format!("sk-{}-{}", "ant", "api03-ABCDEFGHIJKLMNOPQRSTUVWXYZabcd");
        let mut out = String::new();
        for i in 1..=100 {
            out.push_str(&format!("line {i}\n"));
        }
        out.push_str(&format!("export ANTHROPIC_API_KEY={key}\r\ndone\n"));
        let (tail, redacted) = tail_redacted(out.as_bytes(), TAIL_LINES);
        assert_eq!(tail.len(), 40);
        assert_eq!(tail[0], "line 63");
        assert_eq!(tail[39], "done");
        assert_eq!(redacted, 1);
        assert!(tail[38].starts_with("export ANTHROPIC_API_KEY=****"));
        assert!(!tail.join("\n").contains("sk-ant"));
        assert_eq!(tail_redacted(b"", 40), (vec![], 0));
        assert_eq!(tail_redacted(b"one", 40).0, ["one"]);
    }

    /// Against the in-process spacesd: the lookup reads the guest's files,
    /// a Volume hook lands in the guest as an executable script, and the
    /// hook runs through spacesd (the mock runs no shell, so it reports the
    /// program as not found: a failed hook, with the reason in the note).
    #[tokio::test]
    async fn through_spacesd() {
        use cua_spacesd_client::testing::{MockAuth, MockServer};
        let srv = MockServer::start(MockAuth::default()).await;
        let guest = SpacesdClient::connect_url(&format!("http://{}", srv.addr), None)
            .await
            .unwrap();
        srv.state.put_file("/home/cua/agents/a/work/.keep", vec![]);
        let d = tempfile::tempdir().unwrap();
        let b = cua_volume::fs::FsBackend::new(d.path());
        b.put(
            &volume_key("local:dev", HookKind::Setup),
            b"#!/bin/sh\necho hi\n".to_vec(),
            cua_volume::Condition::None,
        )
        .await
        .unwrap();
        let lk = Lookup {
            workdirs: vec!["/home/cua/agents/a/work".into()],
            volume_space: Some("local:dev".into()),
            stage_dir: Some("/home/cua/.cua/hooks".into()),
        };
        let found = find(&guest, Some(&b), HookKind::Setup, &lk)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.path, "/home/cua/.cua/hooks/local-dev-setup");
        assert_eq!(
            srv.state.file(&found.path).unwrap(),
            b"#!/bin/sh\necho hi\n"
        );
        srv.state
            .put_file("/home/cua/agents/a/work/.cua/setup", b"echo x\n".to_vec());
        let r = run(
            &guest,
            Some(&b),
            HookKind::Setup,
            &lk,
            Some(Duration::from_secs(5)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(r.source, "/home/cua/agents/a/work/.cua/setup");
        assert!(!r.ok);
        assert!(r.note().starts_with(".cua/setup failed"), "{}", r.note());
    }
}
