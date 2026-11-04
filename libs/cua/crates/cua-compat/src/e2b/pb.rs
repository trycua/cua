//! envd's `process` and `filesystem` messages in proto3 JSON form (the
//! SDK's codec): lowerCamel field names (snake_case accepted), int64 as
//! strings, bytes as base64, enums by name. Hand-written from the public
//! protos; only the fields envd defines.

use base64::Engine;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn unb64(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(s))
        .map_err(|e| format!("invalid base64: {e}"))
}

fn de_bytes<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    s.map(|s| unb64(&s).map_err(serde::de::Error::custom))
        .transpose()
}

/// A uint32 that proto3 JSON may send as a number or a string.
fn de_u32<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = Value::deserialize(d)?;
    match v {
        Value::Null => Ok(0),
        Value::Number(n) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| serde::de::Error::custom("expected uint32")),
        Value::String(s) => s.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("expected uint32")),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProcessConfig {
    pub cmd: String,
    pub args: Vec<String>,
    pub envs: BTreeMap<String, String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PtySize {
    #[serde(deserialize_with = "de_u32")]
    pub cols: u32,
    #[serde(deserialize_with = "de_u32")]
    pub rows: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Pty {
    pub size: Option<PtySize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StartRequest {
    pub process: Option<ProcessConfig>,
    pub pty: Option<Pty>,
    pub tag: Option<String>,
    pub stdin: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default)]
pub struct ProcessSelector {
    #[serde(deserialize_with = "de_opt_u32")]
    pub pid: Option<u32>,
    pub tag: Option<String>,
}

fn de_opt_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    let v = Value::deserialize(d)?;
    match v {
        Value::Null => Ok(None),
        other => de_u32(other).map(Some).map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConnectRequest {
    pub process: Option<ProcessSelector>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProcessInput {
    #[serde(deserialize_with = "de_bytes")]
    pub stdin: Option<Vec<u8>>,
    #[serde(deserialize_with = "de_bytes")]
    pub pty: Option<Vec<u8>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SendInputRequest {
    pub process: Option<ProcessSelector>,
    pub input: Option<ProcessInput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SendSignalRequest {
    pub process: Option<ProcessSelector>,
    pub signal: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SelectorRequest {
    pub process: Option<ProcessSelector>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpdateRequest {
    pub process: Option<ProcessSelector>,
    pub pty: Option<Pty>,
}

/// One `StreamInput` client message.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StreamInputRequest {
    pub start: Option<SelectorRequest>,
    pub data: Option<StreamInputData>,
    pub keepalive: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct StreamInputData {
    pub input: Option<ProcessInput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PathRequest {
    pub path: String,
    #[serde(deserialize_with = "de_u32")]
    pub depth: u32,
    pub recursive: bool,
    #[serde(alias = "includeEntry")]
    pub include_entry: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MoveRequest {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WatcherRequest {
    #[serde(alias = "watcherId")]
    pub watcher_id: String,
}

/// envd's `Signal` enum value (name or number).
pub fn signal_number(v: &Option<Value>) -> i32 {
    match v {
        Some(Value::String(s)) => match s.as_str() {
            "SIGNAL_SIGKILL" => 9,
            "SIGNAL_SIGTERM" => 15,
            _ => 0,
        },
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) as i32,
        _ => 0,
    }
}

/// `ProcessEvent.end` the way envd reports it (Go's `ProcessState`):
/// `status` is `exit status N` or `signal: <name>`; `error` repeats it for
/// a non-zero exit or a signal.
pub fn end_event(
    exit_code: Option<i32>,
    signal: i32,
    signal_number: i32,
    error: &str,
    lost: bool,
) -> Value {
    let mut end = Map::new();
    let (code, exited, status) = match exit_code {
        Some(c) => (c, true, format!("exit status {c}")),
        None if signal != 0 => (-1, false, format!("signal: {}", go_signal_name(signal))),
        // A signal the contract has no name for: the raw number names it.
        None if signal_number != 0 => (
            -1,
            false,
            format!("signal: {}", go_signal_name(signal_number)),
        ),
        None if lost => (-1, false, "process lost".to_string()),
        None => (
            -1,
            false,
            if error.is_empty() {
                "unknown".into()
            } else {
                error.to_string()
            },
        ),
    };
    end.insert("exitCode".into(), json!(code));
    end.insert("exited".into(), json!(exited));
    end.insert("status".into(), json!(status));
    let err = if !error.is_empty() && exit_code.is_none() && signal == 0 && signal_number == 0 {
        Some(error.to_string())
    } else if code != 0 || !exited {
        Some(status.clone())
    } else {
        None
    };
    if let Some(e) = err {
        end.insert("error".into(), json!(e));
    }
    json!({"event": {"end": Value::Object(end)}})
}

/// Go's `syscall.Signal.String()` names (what envd's status carries).
pub fn go_signal_name(sig: i32) -> String {
    match sig {
        1 => "hangup".into(),
        2 => "interrupt".into(),
        3 => "quit".into(),
        4 => "illegal instruction".into(),
        5 => "trace/breakpoint trap".into(),
        6 => "aborted".into(),
        7 => "bus error".into(),
        8 => "floating point exception".into(),
        9 => "killed".into(),
        10 => "user defined signal 1".into(),
        11 => "segmentation fault".into(),
        12 => "user defined signal 2".into(),
        13 => "broken pipe".into(),
        14 => "alarm clock".into(),
        15 => "terminated".into(),
        n => format!("signal {n}"),
    }
}

/// Go's `fs.FileMode.String()` for a type and mode bits: the flag letters
/// (`d` directory, `L` symlink, `D` device, `u` setuid, `g` setgid, `c`
/// character device, `t` sticky, in Go's order; `-` when there are none)
/// followed by `rwxrwxrwx`. `-rw-r--r--`, `drwxr-xr-x`, `dtrwxrwxrwx`,
/// `Lrwxrwxrwx`.
pub fn permissions_string(file_type: i32, mode: u32) -> String {
    let mut s = String::with_capacity(12);
    if file_type == 2 {
        s.push('d');
    }
    if file_type == 3 {
        s.push('L');
    }
    if file_type == 4 {
        // Devices (cua.env.v1 reports sockets, FIFOs and devices alike).
        s.push('D');
    }
    if mode & 0o4000 != 0 {
        s.push('u');
    }
    if mode & 0o2000 != 0 {
        s.push('g');
    }
    if file_type == 4 {
        s.push('c');
    }
    if mode & 0o1000 != 0 {
        s.push('t');
    }
    if s.is_empty() {
        s.push('-');
    }
    for (i, c) in "rwxrwxrwx".chars().enumerate() {
        s.push(if mode & (1 << (8 - i)) != 0 { c } else { '-' });
    }
    s
}

/// An RFC 3339 timestamp from a protobuf `Timestamp`.
pub fn timestamp(t: &pbjson_types::Timestamp) -> String {
    let secs = t.seconds;
    let nanos = t.nanos.max(0) as u32;
    let dt = time::OffsetDateTime::from_unix_timestamp(secs)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        + time::Duration::nanoseconds(i64::from(nanos));
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// envd `EventType` name for a cua.env.v1 `FsEventType` number.
pub fn event_type_name(t: i32) -> &'static str {
    match t {
        1 => "EVENT_TYPE_CREATE",
        2 => "EVENT_TYPE_WRITE",
        3 => "EVENT_TYPE_REMOVE",
        4 => "EVENT_TYPE_RENAME",
        5 => "EVENT_TYPE_CHMOD",
        _ => "EVENT_TYPE_UNSPECIFIED",
    }
}

/// An envd `FilesystemEvent`: `name` is relative to the watched directory.
pub fn fs_event(e: &cua_spacesd_client::pb::FsEvent, watched: &str) -> Value {
    let base = watched.trim_end_matches('/');
    let name = e
        .path
        .strip_prefix(base)
        .map(|p| p.trim_start_matches('/'))
        .filter(|p| !p.is_empty())
        .unwrap_or(&e.path);
    json!({"name": name, "type": event_type_name(e.r#type)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn out_of_range_uint32_reads_as_protojson_says() {
        let err = crate::connect::with_request_type("process.SendSignalRequest".into(), async {
            crate::connect::decode_json::<SendSignalRequest>(
                br#"{"process":{"pid":-5},"signal":15}"#,
            )
            .map(|_| ())
        })
        .await
        .unwrap_err();
        assert_eq!(
            err.message,
            "unmarshal message: unmarshal into *process.SendSignalRequest: proto: (line 1:19): invalid value for uint32 field pid: -5"
        );
    }

    #[test]
    fn requests_take_protojson_names_in_both_spellings() {
        // Protojson (what the SDKs send) is lowerCamelCase; the proto field
        // name is accepted too.
        for body in [json!({"watcherId": "w1"}), json!({"watcher_id": "w1"})] {
            let r: WatcherRequest = serde_json::from_value(body).unwrap();
            assert_eq!(r.watcher_id, "w1");
        }
        for body in [
            json!({"includeEntry": true}),
            json!({"include_entry": true}),
        ] {
            let r: PathRequest = serde_json::from_value(body).unwrap();
            assert!(r.include_entry);
        }
    }

    #[test]
    fn end_events_match_envd() {
        assert_eq!(
            end_event(Some(0), 0, 0, "", false),
            json!({"event":{"end":{"exitCode":0,"exited":true,"status":"exit status 0"}}})
        );
        assert_eq!(
            end_event(Some(5), 0, 0, "", false)["event"]["end"]["error"],
            "exit status 5"
        );
        let k = end_event(None, 9, 9, "", false);
        assert_eq!(k["event"]["end"]["exitCode"], -1);
        assert_eq!(k["event"]["end"]["error"], "signal: killed");
        assert_eq!(
            end_event(None, 15, 15, "", false)["event"]["end"]["status"],
            "signal: terminated"
        );
        // SIGSEGV has no name in the contract; its number does.
        let segv = end_event(None, 0, 11, "", false);
        assert_eq!(segv["event"]["end"]["error"], "signal: segmentation fault");
        assert_eq!(segv["event"]["end"]["exitCode"], -1);
    }

    #[test]
    fn permissions_like_go() {
        assert_eq!(permissions_string(1, 0o644), "-rw-r--r--");
        assert_eq!(permissions_string(2, 0o755), "drwxr-xr-x");
        // Go prints special bits as letters before the rwx triplets.
        assert_eq!(permissions_string(2, 0o1777), "dtrwxrwxrwx");
        assert_eq!(permissions_string(1, 0o4755), "urwxr-xr-x");
        assert_eq!(permissions_string(1, 0o6755), "ugrwxr-xr-x");
        assert_eq!(permissions_string(3, 0o777), "Lrwxrwxrwx");
        assert_eq!(permissions_string(4, 0o666), "Dcrw-rw-rw-");
    }

    #[test]
    fn decodes_sdk_requests() {
        let r: StartRequest = serde_json::from_str(
            r#"{"process":{"cmd":"/bin/bash","args":["-l","-c","echo"],"envs":{"A":"1"},"cwd":"/tmp"},"tag":"t","stdin":false}"#,
        )
        .unwrap();
        let p = r.process.unwrap();
        assert_eq!(p.cmd, "/bin/bash");
        assert_eq!(p.cwd.as_deref(), Some("/tmp"));
        let s: SendInputRequest =
            serde_json::from_str(r#"{"process":{"pid":5},"input":{"stdin":"eA=="}}"#).unwrap();
        assert_eq!(s.process.unwrap().pid, Some(5));
        assert_eq!(s.input.unwrap().stdin.unwrap(), b"x");
        let sig: SendSignalRequest =
            serde_json::from_str(r#"{"process":{"pid":5},"signal":"SIGNAL_SIGKILL"}"#).unwrap();
        assert_eq!(signal_number(&sig.signal), 9);
    }

    #[test]
    fn fs_event_names_are_relative() {
        let e = cua_spacesd_client::pb::FsEvent {
            path: "/tmp/w/a.txt".into(),
            r#type: 1,
            ..Default::default()
        };
        assert_eq!(
            fs_event(&e, "/tmp/w/"),
            json!({"name":"a.txt","type":"EVENT_TYPE_CREATE"})
        );
    }
}
