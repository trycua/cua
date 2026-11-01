//! `modal-sandbox-fs-tools`, answered by the gateway.
//!
//! The Modal client implements `sandbox.filesystem.*` by running
//! `/__modal/.bin/modal-sandbox-fs-tools '<json command>'` in the guest and
//! reading its stdout and exit code (list/stat/read output, a JSON error
//! on stderr). A Cua Space has no such program, so the command router
//! recognises that command line and runs the operation itself, through the
//! Space's cua-spacesd file APIs, as the same exec: same stdout, same
//! stderr JSON, same exit codes. Nothing is installed in the guest.
//!
//! Commands (the client's JSON, an externally tagged enum):
//! `ReadFile`, `WriteFile` (content on stdin), `ListFiles`,
//! `MakeDirectory`, `Remove`, `Stat`, `Watch` (JSON event lines until the
//! timeout or stdin EOF).

use crate::modal::exec::{Exit, STDERR, STDOUT, Session};
use bytes::Bytes;
use cua_spacesd_client::{Error as FsErr, SpacesdClient, pb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub const TOOL_PATH: &str = "/__modal/.bin/modal-sandbox-fs-tools";

/// Largest file `ReadFile` serves (the real limit is not observable from
/// the client; see maps/modal-coverage.md).
pub const READ_LIMIT: u64 = 1 << 30;

/// The channel a `WriteFile` / `Watch` operation reads stdin from.
#[derive(Clone)]
pub struct StdinSink(mpsc::Sender<Option<Bytes>>);

pub struct SinkClosed;

impl StdinSink {
    pub async fn send(&self, chunk: Option<Bytes>) -> Result<(), SinkClosed> {
        self.0.send(chunk).await.map_err(|_| SinkClosed)
    }
}

pub fn channel() -> (StdinSink, mpsc::Receiver<Option<Bytes>>) {
    let (tx, rx) = mpsc::channel(8);
    (StdinSink(tx), rx)
}

/// Whether an exec's argv is the fs tool: `[TOOL_PATH, json]`.
pub fn is_tool(argv: &[String]) -> bool {
    argv.first().is_some_and(|a| a == TOOL_PATH)
}

#[derive(Debug, Deserialize)]
enum Cmd {
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
    },
    ListFiles {
        path: String,
    },
    MakeDirectory {
        path: String,
        #[serde(default)]
        parents: bool,
    },
    Remove {
        path: String,
        #[serde(default)]
        recursive: bool,
    },
    Stat {
        path: String,
    },
    Watch {
        path: String,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        filter: Option<Vec<String>>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
}

/// A failed operation, as the tool reports it: `error_kind` and message on
/// stderr as JSON, exit code 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub kind: &'static str,
    pub message: String,
    pub detail: String,
}

impl ToolError {
    fn new(kind: &'static str, errno_text: &str, errno: i32) -> ToolError {
        ToolError {
            kind,
            message: format!("{errno_text} (os error {errno})"),
            detail: String::new(),
        }
    }
    pub fn not_found() -> Self {
        Self::new("NotFound", "No such file or directory", 2)
    }
    pub fn permission() -> Self {
        Self::new("PermissionDenied", "Permission denied", 13)
    }
    pub fn not_dir() -> Self {
        Self::new("NotDirectory", "Not a directory", 20)
    }
    pub fn is_dir() -> Self {
        Self::new("IsDirectory", "Is a directory", 21)
    }
    pub fn exists() -> Self {
        Self::new("PathAlreadyExists", "File exists", 17)
    }
    pub fn not_empty() -> Self {
        Self::new("DirectoryNotEmpty", "Directory not empty", 39)
    }
    pub fn too_large(size: u64) -> Self {
        ToolError {
            kind: "FileTooLarge",
            message: format!(
                "File too large: {size} bytes exceeds the read limit of {READ_LIMIT} bytes"
            ),
            detail: String::new(),
        }
    }
    pub fn other(message: impl Into<String>) -> Self {
        ToolError {
            kind: "Other",
            message: message.into(),
            detail: String::new(),
        }
    }

    pub fn json(&self) -> String {
        json!({"error_kind": self.kind, "message": self.message, "detail": self.detail}).to_string()
    }
}

impl From<FsErr> for ToolError {
    fn from(e: FsErr) -> Self {
        match e {
            FsErr::PathNotFound(_) => Self::not_found(),
            FsErr::PathExists(_) => Self::exists(),
            FsErr::NotADirectory(_) => Self::not_dir(),
            FsErr::IsADirectory(_) => Self::is_dir(),
            FsErr::PermissionDenied(_) => Self::permission(),
            FsErr::DiskFull(_) => Self::other("No space left on device (os error 28)"),
            other => Self::other(other.to_string()),
        }
    }
}

impl From<tonic::Status> for ToolError {
    fn from(s: tonic::Status) -> Self {
        FsErr::from(s).into()
    }
}

/// The `permissions` string of an `ls -l` line: type and nine bits.
pub fn permissions_string(kind: &str, mode: u32) -> String {
    let t = match kind {
        "directory" => 'd',
        "symlink" => 'l',
        _ => '-',
    };
    let bit = |m: u32, r: char| if mode & m != 0 { r } else { '-' };
    let x =
        |m: u32, special: u32, lower: char, upper: char| match (mode & m != 0, mode & special != 0)
        {
            (true, true) => lower,
            (false, true) => upper,
            (true, false) => 'x',
            (false, false) => '-',
        };
    format!(
        "{t}{}{}{}{}{}{}{}{}{}",
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        x(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        x(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        x(0o001, 0o1000, 't', 'T'),
    )
}

/// `FileInfo` as the tool prints it.
pub fn entry_json(e: &pb::EntryInfo) -> Value {
    let kind = match pb::FileType::try_from(e.r#type).unwrap_or(pb::FileType::Unspecified) {
        pb::FileType::Directory => "directory",
        pb::FileType::Symlink => "symlink",
        _ => "file",
    };
    let type_bits = match kind {
        "directory" => 0o040000,
        "symlink" => 0o120000,
        _ => 0o100000,
    };
    let modified = e
        .modified_at
        .as_ref()
        .map(|t| t.seconds as f64 + f64::from(t.nanos) / 1e9)
        .unwrap_or(0.0);
    let mut v = json!({
        "name": e.name,
        "path": e.path,
        "type": kind,
        "size": e.size,
        "mode": type_bits | (e.mode & 0o7777),
        "permissions": permissions_string(kind, e.mode),
        "owner": e.owner,
        "group": e.group,
        "modified_time": modified,
    });
    if kind == "symlink" && !e.symlink_target.is_empty() {
        v["symlink_target"] = json!(e.symlink_target);
    }
    v
}

/// What an operation runs against.
#[derive(Clone)]
pub struct Target {
    pub client: SpacesdClient,
    /// The user cua-spacesd runs as.
    pub driver: String,
    /// The user the tool's file writes belong to (root, as in a Modal
    /// container); `driver` when the guest is not Linux.
    pub user: String,
}

/// Runs the tool's command and ends `session` with its exit.
pub async fn run(
    target: Target,
    json_cmd: String,
    session: Arc<Session>,
    stdin: Option<mpsc::Receiver<Option<Bytes>>>,
) {
    let result = match serde_json::from_str::<Cmd>(&json_cmd) {
        Ok(cmd) => dispatch(&target, cmd, &session, stdin).await,
        Err(e) => Err(ToolError {
            kind: "InvalidCommand",
            message: format!("invalid command: {e}"),
            detail: String::new(),
        }),
    };
    match result {
        Ok(()) => session.finish(Exit::code(0)),
        Err(e) => {
            session.push(STDERR, e.json().as_bytes());
            session.finish(Exit::code(1));
        }
    }
}

// ---- a guest whose cua-spacesd is not root ---------------------------------
//
// The file API acts as cua-spacesd's own user. In a guest where that user is
// not root (a container Space), paths only root can reach answer permission
// denied; the tool then does the operation as root with the commands any
// guest has, which is what Modal's tool (running as root) would have done.

fn denied(s: &tonic::Status) -> bool {
    s.code() == tonic::Code::PermissionDenied
}

async fn root_sh(
    t: &Target,
    script: &str,
    args: &[&str],
) -> Result<cua_spacesd_client::Output, ToolError> {
    crate::guest::run_as(&t.client, &t.driver, "root", script, args)
        .await
        .map_err(ToolError::other)
}

fn tool_error_of(stderr: &str) -> ToolError {
    let m = stderr.to_ascii_lowercase();
    if m.contains("no such file") {
        ToolError::not_found()
    } else if m.contains("not a directory") {
        ToolError::not_dir()
    } else if m.contains("is a directory") {
        ToolError::is_dir()
    } else if m.contains("file exists") {
        ToolError::exists()
    } else if m.contains("not empty") {
        ToolError::not_empty()
    } else if m.contains("permission denied") {
        ToolError::permission()
    } else {
        ToolError::other(stderr.trim().to_string())
    }
}

fn entry_of(path: &str, fields: &[&str], target: &str) -> Option<pb::EntryInfo> {
    // %f (raw mode, hex)  %s  %U  %G  %Y
    let raw = u32::from_str_radix(fields.first()?, 16).ok()?;
    let ty = match raw & 0o170000 {
        0o040000 => pb::FileType::Directory,
        0o120000 => pb::FileType::Symlink,
        0o100000 => pb::FileType::File,
        _ => pb::FileType::Other,
    };
    Some(pb::EntryInfo {
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        path: path.to_string(),
        r#type: ty as i32,
        size: fields.get(1)?.parse().ok()?,
        mode: raw & 0o7777,
        owner: fields.get(2)?.to_string(),
        group: fields.get(3)?.to_string(),
        modified_at: Some(pbjson_types::Timestamp {
            seconds: fields.get(4)?.parse().ok()?,
            nanos: 0,
        }),
        symlink_target: target.to_string(),
        ..Default::default()
    })
}

async fn root_stat(
    t: &Target,
    path: &str,
    follow: bool,
) -> Result<Option<pb::EntryInfo>, ToolError> {
    let flag = if follow { "-L" } else { "" };
    let script = format!(
        "stat {flag} -c '%f %s %U %G %Y' -- \"$1\" 2>/dev/null || exit 3; [ -L \"$1\" ] && [ -z '{flag}' ] && readlink -- \"$1\"; true"
    );
    let out = root_sh(t, &script, &[path]).await?;
    if out.status.code == Some(3) {
        return Ok(None);
    }
    let text = out.stdout_str();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let fields: Vec<&str> = first.split(' ').collect();
    Ok(entry_of(path, &fields, lines.next().unwrap_or_default()))
}

async fn root_list(t: &Target, path: &str) -> Result<Vec<pb::EntryInfo>, ToolError> {
    let out = root_sh(
        t,
        "find \"$1\" -mindepth 1 -maxdepth 1 -printf '%p\\t%m\\t%s\\t%u\\t%g\\t%T@\\t%y\\t%l\\n'",
        &[path],
    )
    .await?;
    if !out.success() {
        return Err(tool_error_of(&out.stderr_str()));
    }
    let mut v = Vec::new();
    for l in out.stdout_str().lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 8 {
            continue;
        }
        let ty = match f[6] {
            "d" => pb::FileType::Directory,
            "l" => pb::FileType::Symlink,
            "f" => pb::FileType::File,
            _ => pb::FileType::Other,
        };
        v.push(pb::EntryInfo {
            name: f[0].rsplit('/').next().unwrap_or(f[0]).to_string(),
            path: f[0].to_string(),
            r#type: ty as i32,
            size: f[2].parse().unwrap_or(0),
            mode: u32::from_str_radix(f[1], 8).unwrap_or(0),
            owner: f[3].to_string(),
            group: f[4].to_string(),
            modified_at: Some(pbjson_types::Timestamp {
                seconds: f[5]
                    .split('.')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                nanos: 0,
            }),
            symlink_target: f[7].to_string(),
            ..Default::default()
        });
    }
    Ok(v)
}

async fn lstat_t(t: &Target, path: &str) -> Result<Option<pb::EntryInfo>, ToolError> {
    match t
        .client
        .filesystem()
        .stat(pb::StatRequest {
            path: path.into(),
            no_follow_symlinks: true,
            ..Default::default()
        })
        .await
    {
        Ok(r) => Ok(r.into_inner().entry),
        Err(s) if s.code() == tonic::Code::NotFound => Ok(None),
        Err(s) if denied(&s) => root_stat(t, path, false).await,
        Err(s) => Err(s.into()),
    }
}

async fn stat_follow_t(t: &Target, path: &str) -> Result<Option<pb::EntryInfo>, ToolError> {
    match t
        .client
        .filesystem()
        .stat(pb::StatRequest {
            path: path.into(),
            no_follow_symlinks: false,
            ..Default::default()
        })
        .await
    {
        Ok(r) => Ok(r.into_inner().entry),
        Err(s) if s.code() == tonic::Code::NotFound => Ok(None),
        Err(s) if denied(&s) => root_stat(t, path, true).await,
        Err(s) => Err(s.into()),
    }
}

async fn list_t(t: &Target, path: &str) -> Result<Vec<pb::EntryInfo>, ToolError> {
    match t.client.list_dir(path, 1).await {
        Ok(v) => Ok(v),
        Err(e) if matches!(&e, cua_spacesd_client::Error::PermissionDenied(_)) => {
            root_list(t, path).await
        }
        Err(e) => Err(e.into()),
    }
}

fn is_dir(e: &pb::EntryInfo) -> bool {
    e.r#type == pb::FileType::Directory as i32
}

/// The nearest existing ancestor of `path` is not a directory.
async fn blocked_by_file(t: &Target, path: &str) -> Result<bool, ToolError> {
    let mut cur = path.to_string();
    while let Some((parent, _)) = cur.rsplit_once('/') {
        let parent = if parent.is_empty() { "/" } else { parent };
        if let Some(e) = stat_follow_t(t, parent).await? {
            return Ok(!is_dir(&e));
        }
        if parent == "/" {
            break;
        }
        cur = parent.to_string();
    }
    Ok(false)
}

async fn dispatch(
    t: &Target,
    cmd: Cmd,
    s: &Arc<Session>,
    stdin: Option<mpsc::Receiver<Option<Bytes>>>,
) -> Result<(), ToolError> {
    let c = &t.client;
    match cmd {
        Cmd::Stat { path } => {
            if blocked_by_file(t, &path).await? {
                return Err(ToolError::not_dir());
            }
            let e = lstat_t(t, &path).await?.ok_or_else(ToolError::not_found)?;
            s.push(STDOUT, entry_json(&e).to_string().as_bytes());
            Ok(())
        }
        Cmd::ListFiles { path } => {
            if blocked_by_file(t, &path).await? {
                return Err(ToolError::not_dir());
            }
            let e = stat_follow_t(t, &path)
                .await?
                .ok_or_else(ToolError::not_found)?;
            if !is_dir(&e) {
                return Err(ToolError {
                    kind: "NotDirectory",
                    message: "Not a directory (os error 20)".into(),
                    detail: String::new(),
                });
            }
            let entries = list_t(t, &path).await?;
            let list: Vec<Value> = entries.iter().map(entry_json).collect();
            s.push(STDOUT, Value::Array(list).to_string().as_bytes());
            Ok(())
        }
        Cmd::ReadFile { path } => {
            if blocked_by_file(t, &path).await? {
                return Err(ToolError::not_dir());
            }
            let e = stat_follow_t(t, &path)
                .await?
                .ok_or_else(ToolError::not_found)?;
            if is_dir(&e) {
                return Err(ToolError::is_dir());
            }
            if e.size > READ_LIMIT {
                return Err(ToolError::too_large(e.size));
            }
            let mut sink = SessionSink(s.clone());
            match c.download_with(&path, Default::default(), &mut sink).await {
                Ok(_) => Ok(()),
                Err(cua_spacesd_client::Error::PermissionDenied(_)) => {
                    let out = root_sh(t, "cat -- \"$1\"", &[&path]).await?;
                    if out.success() {
                        s.push(STDOUT, &out.stdout);
                        Ok(())
                    } else {
                        Err(tool_error_of(&out.stderr_str()))
                    }
                }
                Err(e) => Err(e.into()),
            }
        }
        Cmd::WriteFile { path } => write_file(t, &path, s, stdin).await,
        Cmd::MakeDirectory { path, parents } => {
            if blocked_by_file(t, &path).await? {
                return Err(ToolError::not_dir());
            }
            let existing = lstat_t(t, &path).await?;
            if let Some(e) = existing {
                // `mkdir -p` succeeds on a directory; a plain mkdir does not.
                return if parents && is_dir(&e) {
                    Ok(())
                } else {
                    Err(ToolError::exists())
                };
            }
            if !parents {
                let parent = path
                    .rsplit_once('/')
                    .map_or("/", |(p, _)| if p.is_empty() { "/" } else { p });
                if stat_follow_t(t, parent).await?.is_none() {
                    return Err(ToolError::not_found());
                }
            }
            match c
                .filesystem()
                .make_dir(pb::MakeDirRequest {
                    path: path.clone(),
                    parents,
                    mode: 0o755,
                    ..Default::default()
                })
                .await
            {
                Ok(_) => Ok(()),
                Err(st) if denied(&st) => {
                    let out = root_sh(
                        t,
                        if parents {
                            "mkdir -p -- \"$1\""
                        } else {
                            "mkdir -- \"$1\""
                        },
                        &[&path],
                    )
                    .await?;
                    if out.success() {
                        Ok(())
                    } else {
                        Err(tool_error_of(&out.stderr_str()))
                    }
                }
                Err(st) => Err(st.into()),
            }
        }
        Cmd::Remove { path, recursive } => {
            let e = lstat_t(t, &path).await?.ok_or_else(ToolError::not_found)?;
            if is_dir(&e) && !recursive {
                let kids = list_t(t, &path).await?;
                if !kids.is_empty() {
                    return Err(ToolError::not_empty());
                }
            }
            match c
                .filesystem()
                .remove(pb::RemoveRequest {
                    path: path.clone(),
                    recursive,
                    missing_ok: false,
                    ..Default::default()
                })
                .await
            {
                Ok(_) => Ok(()),
                Err(st) if denied(&st) => {
                    let out = root_sh(
                        t,
                        if recursive {
                            "rm -rf -- \"$1\""
                        } else {
                            "rm -f -- \"$1\" 2>&1 || rmdir -- \"$1\""
                        },
                        &[&path],
                    )
                    .await?;
                    if out.success() {
                        Ok(())
                    } else {
                        Err(tool_error_of(&out.stderr_str()))
                    }
                }
                Err(st) => Err(st.into()),
            }
        }
        Cmd::Watch {
            path,
            recursive,
            filter,
            timeout_secs,
        } => watch(t, &path, recursive, filter, timeout_secs, s, stdin).await,
    }
}

struct SessionSink(Arc<Session>);

impl cua_spacesd_client::DownloadSink for SessionSink {
    async fn write(&mut self, chunk: Bytes) -> cua_spacesd_client::Result<()> {
        self.0.push(STDOUT, &chunk);
        Ok(())
    }
}

async fn write_file(
    t: &Target,
    path: &str,
    _s: &Arc<Session>,
    stdin: Option<mpsc::Receiver<Option<Bytes>>>,
) -> Result<(), ToolError> {
    let c = &t.client;
    if blocked_by_file(t, path).await? {
        return Err(ToolError::not_dir());
    }
    if let Some(e) = stat_follow_t(t, path).await?
        && is_dir(&e)
    {
        return Err(ToolError::is_dir());
    }
    let mut rx = stdin.ok_or_else(|| ToolError::other("stdin is required"))?;
    let mut w = crate::guestfs::Writer::begin(c, path, &t.user, &t.driver)
        .await
        .map_err(fs_error)?;
    loop {
        match rx.recv().await {
            Some(Some(chunk)) => w.write(chunk).await.map_err(fs_error)?,
            Some(None) => break,
            // The writer went away without EOF: leave nothing behind.
            None => return Err(ToolError::other("stdin closed before end of file")),
        }
    }
    w.finish().await.map_err(fs_error)?;
    Ok(())
}

fn fs_error(e: crate::guestfs::FsError) -> ToolError {
    let m = e.1.to_ascii_lowercase();
    match e.0 {
        http::StatusCode::NOT_FOUND => ToolError::not_found(),
        http::StatusCode::FORBIDDEN => ToolError::permission(),
        _ if m.contains("not a directory") => ToolError::not_dir(),
        _ if m.contains("is a directory") => ToolError::is_dir(),
        _ => ToolError::other(e.1),
    }
}

/// Maps spacesd event types to the tool's `event_type` strings.
fn event_type(t: i32) -> &'static str {
    match pb::FsEventType::try_from(t).unwrap_or(pb::FsEventType::Unspecified) {
        pb::FsEventType::Create => "Create",
        pb::FsEventType::Remove => "Remove",
        pb::FsEventType::Rename => "Rename",
        pb::FsEventType::Write | pb::FsEventType::Chmod => "Modify",
        pb::FsEventType::Unspecified => "Unknown",
    }
}

async fn watch(
    t: &Target,
    path: &str,
    recursive: bool,
    filter: Option<Vec<String>>,
    timeout_secs: Option<u64>,
    s: &Arc<Session>,
    stdin: Option<mpsc::Receiver<Option<Bytes>>>,
) -> Result<(), ToolError> {
    let c = &t.client;
    if blocked_by_file(t, path).await? {
        return Err(ToolError::not_dir());
    }
    let target = stat_follow_t(t, path)
        .await?
        .ok_or_else(ToolError::not_found)?;
    // A file is watched through its directory.
    let (dir, only) = if is_dir(&target) {
        (target.path.clone(), None)
    } else {
        let parent = target
            .path
            .rsplit_once('/')
            .map_or("/", |(p, _)| if p.is_empty() { "/" } else { p });
        (parent.to_string(), Some(target.path.clone()))
    };
    let mut stream = c
        .filesystem()
        .watch_dir(pb::WatchDirRequest {
            path: dir.clone(),
            recursive,
            keepalive_interval: None,
            ..Default::default()
        })
        .await?
        .into_inner();
    let deadline = timeout_secs.map(|n| tokio::time::Instant::now() + Duration::from_secs(n));
    let mut stdin = stdin;
    loop {
        let sleep = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        let eof = async {
            match stdin.as_mut() {
                // EOF (or a closed channel): the watcher is done.
                Some(rx) => while let Some(Some(_)) = rx.recv().await {},
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = sleep => return Ok(()),
            _ = eof => return Ok(()),
            msg = stream.message() => {
                let Some(msg) = msg? else { return Ok(()) };
                use pb::watch_dir_response::Message;
                let Some(Message::Event(ev)) = msg.message else { continue };
                let kind = event_type(ev.r#type);
                if let Some(only) = &only
                    && ev.path != *only
                    && ev.old_path != *only
                {
                    continue;
                }
                if let Some(f) = &filter
                    && !f.iter().any(|x| x == kind)
                {
                    continue;
                }
                let mut paths = vec![ev.path.clone()];
                if kind == "Rename" && !ev.old_path.is_empty() {
                    paths = vec![ev.old_path.clone(), ev.path.clone()];
                }
                let line = json!({"event_type": kind, "paths": paths});
                s.push(STDOUT, format!("{line}\n").as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_tool_command_line() {
        assert!(is_tool(&[TOOL_PATH.into(), "{}".into()]));
        assert!(!is_tool(&["/bin/ls".into()]));
        assert!(!is_tool(&[]));
    }

    #[test]
    fn parses_every_client_command() {
        for (j, ok) in [
            (r#"{"ReadFile":{"path":"/a"}}"#, true),
            (r#"{"WriteFile":{"path":"/a"}}"#, true),
            (r#"{"ListFiles":{"path":"/a"}}"#, true),
            (r#"{"MakeDirectory":{"path":"/a","parents":true}}"#, true),
            (r#"{"Remove":{"path":"/a","recursive":false}}"#, true),
            (r#"{"Stat":{"path":"/a"}}"#, true),
            (
                r#"{"Watch":{"path":"/a","recursive":true,"filter":["Create","Modify","Rename"],"timeout_secs":null}}"#,
                true,
            ),
            (r#"{"Nope":{}}"#, false),
        ] {
            assert_eq!(serde_json::from_str::<Cmd>(j).is_ok(), ok, "{j}");
        }
    }

    #[test]
    fn permissions_look_like_ls() {
        assert_eq!(permissions_string("file", 0o644), "-rw-r--r--");
        assert_eq!(permissions_string("directory", 0o755), "drwxr-xr-x");
        assert_eq!(permissions_string("file", 0o4755), "-rwsr-xr-x");
        assert_eq!(permissions_string("directory", 0o1777), "drwxrwxrwt");
        assert_eq!(permissions_string("symlink", 0o777), "lrwxrwxrwx");
    }

    #[test]
    fn errors_serialise_as_the_client_expects() {
        let v: Value = serde_json::from_str(&ToolError::not_found().json()).unwrap();
        assert_eq!(v["error_kind"], "NotFound");
        assert!(v["message"].as_str().unwrap().contains("os error 2"));
        assert!(!v["message"].as_str().unwrap().trim().is_empty());
    }
}
