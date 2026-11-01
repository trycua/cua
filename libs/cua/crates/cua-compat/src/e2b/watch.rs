//! `WatchDir` / `CreateWatcher`, with envd's event semantics.
//!
//! envd watches with Go's fsnotify, whose event types follow the kernel's
//! inotify bits: a created entry or the destination of a move is `create`,
//! `IN_MODIFY` is `write`, `IN_ATTRIB` is `chmod` (envd's own `chown` after
//! it writes a file shows up as one), a deleted entry is `remove`, and the
//! source of a move is `rename`. cua-spacesd's watcher folds attribute and
//! content changes together and reports the source of a move as a removal,
//! so the watch runs as a small root `python3` inotify reader in the guest
//! (which also reaches directories the Cua guest agent cannot read). A guest
//! without `python3` falls back to cua-spacesd's own watch.

use crate::connect::RpcError;
use cua_spacesd_client::{SpacesdClient, pb};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;

/// What a watch yields.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchItem {
    Started,
    Keepalive,
    Event {
        /// Relative to the watched directory.
        name: String,
        /// envd's `EventType` name.
        kind: &'static str,
        /// The absolute path.
        path: String,
    },
}

pub type Items = Pin<Box<dyn Stream<Item = Result<WatchItem, RpcError>> + Send>>;

/// The guest reader: prints `READY` once armed, then one JSON object per
/// event (`{"t": "create|write|remove|rename|chmod", "n": "<relative>"}`).
const READER: &str = r#"
import ctypes, json, os, sys
libc = ctypes.CDLL(None, use_errno=True)
IN_MODIFY, IN_ATTRIB, IN_MOVED_FROM, IN_MOVED_TO = 0x2, 0x4, 0x40, 0x80
IN_CREATE, IN_DELETE, IN_Q_OVERFLOW, IN_ISDIR = 0x100, 0x200, 0x4000, 0x40000000
MASK = IN_MODIFY | IN_ATTRIB | IN_MOVED_FROM | IN_MOVED_TO | IN_CREATE | IN_DELETE
root, rec = os.path.abspath(sys.argv[1]), sys.argv[2] == "1"
fd = libc.inotify_init1(os.O_CLOEXEC)
wds = {}
def add(p):
    wd = libc.inotify_add_watch(fd, p.encode(), MASK)
    if wd >= 0:
        wds[wd] = p
def walk(p):
    add(p)
    if rec:
        try:
            for e in os.scandir(p):
                if e.is_dir(follow_symlinks=False):
                    walk(e.path)
        except OSError:
            pass
walk(root)
sys.stdout.write("READY\n"); sys.stdout.flush()
names = {IN_CREATE: "create", IN_MODIFY: "write", IN_DELETE: "remove", IN_MOVED_FROM: "rename", IN_MOVED_TO: "create", IN_ATTRIB: "chmod"}
import struct
buf = b""
while True:
    data = os.read(fd, 65536)
    buf += data
    while len(buf) >= 16:
        wd, mask, cookie, ln = struct.unpack_from("iIII", buf)
        if len(buf) < 16 + ln:
            break
        name = buf[16:16 + ln].split(b"\0", 1)[0].decode("utf-8", "surrogateescape")
        buf = buf[16 + ln:]
        base = wds.get(wd)
        if base is None or not name:
            continue
        full = os.path.join(base, name)
        rel = os.path.relpath(full, root)
        if mask & IN_ISDIR and mask & (IN_CREATE | IN_MOVED_TO) and rec:
            walk(full)
        for bit, t in names.items():
            if mask & bit:
                sys.stdout.write(json.dumps({"t": t, "n": rel, "p": full}) + "\n")
        sys.stdout.flush()
"#;

fn kind_name(t: &str) -> &'static str {
    match t {
        "create" => "EVENT_TYPE_CREATE",
        "write" => "EVENT_TYPE_WRITE",
        "remove" => "EVENT_TYPE_REMOVE",
        "rename" => "EVENT_TYPE_RENAME",
        _ => "EVENT_TYPE_CHMOD",
    }
}

/// A polling-API watcher (`CreateWatcher`): the events it saw since the
/// last `GetWatcherEvents`.
pub struct Watcher {
    events: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Watcher {
    /// `entry`: the guest to stat changed paths in, for `include_entry`
    /// (events carry the entry when the path still exists).
    pub fn spawn(_path: String, mut items: Items, entry: Option<(SpacesdClient, String)>) -> Self {
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let task = tokio::spawn(async move {
            while let Some(Ok(item)) = items.next().await {
                if let WatchItem::Event { name, kind, path } = item {
                    let mut ev = serde_json::json!({"name": name, "type": kind});
                    if let Some((c, driver)) = &entry
                        && kind != "EVENT_TYPE_REMOVE"
                    {
                        let fs = super::fsops::Fs { c, driver };
                        if let Ok(en) = fs.stat(&path).await {
                            ev["entry"] = en.json();
                        }
                    }
                    sink.lock().expect("watcher events").push(ev);
                }
            }
        });
        Self { events, task }
    }

    /// The events since the last call.
    pub fn drain(&self) -> Vec<serde_json::Value> {
        std::mem::take(&mut *self.events.lock().expect("watcher events"))
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts watching `path`. Errors when the guest reader cannot start.
pub async fn start(
    c: &SpacesdClient,
    driver: &str,
    path: &str,
    recursive: bool,
) -> Result<Items, RpcError> {
    let (command, args, user, cwd) = crate::guest::as_user_in(
        driver,
        "root",
        "python3".into(),
        vec![
            "-u".into(),
            "-c".into(),
            READER.into(),
            path.to_string(),
            if recursive { "1" } else { "0" }.into(),
        ],
        String::new(),
    );
    let req = pb::StartProcessRequest {
        config: Some(pb::ProcessConfig {
            command,
            args,
            user,
            cwd,
            ..Default::default()
        }),
        kill_on_disconnect: true,
        ..Default::default()
    };
    let mut stream = c.process().start_process(req).await?.into_inner();
    // The first line says whether it armed.
    let mut pending = String::new();
    loop {
        let Some(msg) = stream.next().await else {
            return Err(RpcError::internal("the watch reader ended before it armed"));
        };
        let msg = msg?;
        match msg.event.and_then(|e| e.event) {
            Some(pb::process_event::Event::Data(d)) => {
                if let Some(pb::process_data::Output::Stdout(b)) = d.output {
                    pending.push_str(&String::from_utf8_lossy(&b));
                    if let Some(i) = pending.find('\n')
                        && pending[..i].trim() == "READY"
                    {
                        pending.drain(..=i);
                        break;
                    }
                }
            }
            Some(pb::process_event::Event::End(e)) => {
                return Err(RpcError::internal(format!(
                    "the watch reader failed to start (exit {:?})",
                    e.exit_code
                )));
            }
            _ => {}
        }
    }
    let root = path.to_string();
    let lines =
        futures_util::stream::unfold((stream, pending), |(mut stream, mut pending)| async move {
            loop {
                if let Some(i) = pending.find('\n') {
                    let line: String = pending.drain(..=i).collect();
                    return Some((line, (stream, pending)));
                }
                match stream.next().await {
                    Some(Ok(m)) => {
                        if let Some(pb::process_event::Event::Data(d)) =
                            m.event.and_then(|e| e.event)
                            && let Some(pb::process_data::Output::Stdout(b)) = d.output
                        {
                            pending.push_str(&String::from_utf8_lossy(&b));
                        }
                    }
                    _ => return None,
                }
            }
        });
    let started = futures_util::stream::iter([Ok(WatchItem::Started)]);
    let events = lines.filter_map(move |line| {
        let root = root.clone();
        async move {
            let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            Some(Ok(WatchItem::Event {
                name: v["n"].as_str()?.to_string(),
                kind: kind_name(v["t"].as_str()?),
                path: v["p"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| root.clone()),
            }))
        }
    });
    Ok(Box::pin(started.chain(events)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_envd_names() {
        assert_eq!(kind_name("rename"), "EVENT_TYPE_RENAME");
        assert_eq!(kind_name("chmod"), "EVENT_TYPE_CHMOD");
        assert_eq!(kind_name("create"), "EVENT_TYPE_CREATE");
    }
}
