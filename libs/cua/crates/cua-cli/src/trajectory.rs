//! Trajectories: every `cua do` action is recorded into
//! `~/.cua/trajectories/<machine>/<YYYYMMDD-HHMMSS>/turn_NNN/` in the
//! TrajectoryViewer format (`turn_NNN_agent_response.json` plus an optional
//! `screenshot.png`). `cua trajectory` lists, zips, serves and cleans them.

use crate::util::{self, internal, line};
use cua_sdk::CuaError;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

/// `~/.cua/trajectories`.
pub fn root() -> PathBuf {
    util::cua_home().join("trajectories")
}

fn pid_file() -> PathBuf {
    util::cua_home().join("trajectory_server.pid")
}

/// A new session directory for `machine`.
pub fn new_session(machine: &str) -> Result<PathBuf, CuaError> {
    let dir = root()
        .join(sanitize(machine))
        .join(util::timestamp("%Y%m%d-%H%M%S"));
    std::fs::create_dir_all(&dir).map_err(internal)?;
    Ok(dir)
}

fn sanitize(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s == "." || s == ".." {
        "unknown".into()
    } else {
        s
    }
}

fn next_turn(session: &Path) -> u32 {
    std::fs::read_dir(session)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_prefix("turn_")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// Records one turn.
pub fn record_turn(
    session: &Path,
    action_type: &str,
    params: serde_json::Value,
    screenshot: Option<&[u8]>,
) -> Result<PathBuf, CuaError> {
    let n = next_turn(session);
    let name = format!("turn_{n:03}");
    let dir = session.join(&name);
    std::fs::create_dir_all(&dir).map_err(internal)?;
    if let Some(png) = screenshot {
        std::fs::write(dir.join("screenshot.png"), png).map_err(internal)?;
    }
    let mut action = serde_json::json!({ "type": action_type });
    if let (Some(a), serde_json::Value::Object(p)) = (action.as_object_mut(), params) {
        a.extend(p);
    }
    let call_id = format!("call_{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff);
    let now = chrono::Utc::now();
    let v = serde_json::json!({
        "model": "cua-cli",
        "response": {
            "id": format!("resp_{}", now.timestamp_millis()),
            "object": "response",
            "created_at": now.timestamp(),
            "status": "completed",
            "model": "cua-cli",
            "output": [{
                "type": "computer_call",
                "id": call_id,
                "call_id": call_id,
                "action": action,
                "pending_safety_checks": [],
                "status": "completed",
            }],
        },
    });
    std::fs::write(
        dir.join(format!("{name}_agent_response.json")),
        serde_json::to_vec_pretty(&v)?,
    )
    .map_err(internal)?;
    Ok(dir)
}

/// A session as listed.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Session {
    /// Machine name.
    pub machine: String,
    /// Session timestamp.
    pub session: String,
    /// Directory.
    pub path: String,
    /// Recorded turns.
    pub turns: usize,
    /// ISO creation time.
    pub created: String,
}

fn created_of(dir: &Path) -> chrono::NaiveDateTime {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    chrono::NaiveDateTime::parse_from_str(name, "%Y%m%d-%H%M%S").unwrap_or_else(|_| {
        std::fs::metadata(dir)
            .and_then(|m| m.modified())
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).naive_local())
            .unwrap_or_default()
    })
}

fn sorted_dirs(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

/// Sessions, oldest first (optionally of one machine).
pub fn list(machine: Option<&str>) -> Vec<Session> {
    let machines = match machine {
        Some(m) => vec![root().join(m)],
        None => sorted_dirs(&root()),
    };
    let mut out = vec![];
    for m in machines {
        for s in sorted_dirs(&m) {
            let turns = sorted_dirs(&s)
                .iter()
                .filter(|t| {
                    t.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("turn_"))
                })
                .count();
            out.push(Session {
                machine: m
                    .file_name()
                    .map(|n| n.to_string_lossy().into())
                    .unwrap_or_default(),
                session: s
                    .file_name()
                    .map(|n| n.to_string_lossy().into())
                    .unwrap_or_default(),
                path: s.display().to_string(),
                turns,
                created: created_of(&s).format("%Y-%m-%dT%H:%M:%S").to_string(),
            });
        }
    }
    out
}

// -------------------------------------------------------------------- zip

/// Zips a session directory next to it (`<session>.zip`, deflate).
pub fn zip_session(session: &Path) -> Result<PathBuf, CuaError> {
    let name = session
        .file_name()
        .ok_or_else(|| CuaError::InvalidArgument("bad session path".into()))?;
    let zip = session.with_file_name(format!("{}.zip", name.to_string_lossy()));
    let mut files = vec![];
    collect_files(session, session, &mut files)?;
    files.sort();
    let mut w = ZipWriter::default();
    for rel in files {
        let data = std::fs::read(session.join(&rel)).map_err(internal)?;
        w.add(&rel.to_string_lossy().replace('\\', "/"), &data)?;
    }
    std::fs::write(&zip, w.finish()).map_err(internal)?;
    Ok(zip)
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), CuaError> {
    for e in std::fs::read_dir(dir).map_err(internal)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(base, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// A minimal ZIP (PKWARE APPNOTE 4.3) writer: deflate entries, no zip64
/// (trajectory sessions are far below 4 GiB).
#[derive(Default)]
pub struct ZipWriter {
    buf: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl ZipWriter {
    /// Adds a file.
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), CuaError> {
        use flate2::{Compression, write::DeflateEncoder};
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).map_err(internal)?;
        let comp = enc.finish().map_err(internal)?;
        let crc = crc32fast::hash(data);
        let offset = self.buf.len() as u32;
        let (size, csize) = (data.len() as u32, comp.len() as u32);
        let n = name.as_bytes();
        // DOS time: 1980-01-01 00:00 (deterministic).
        let (time, date) = (0u16, (1u16 << 5) | 1);
        let mut local = vec![];
        local.extend(0x0403_4b50u32.to_le_bytes());
        local.extend(20u16.to_le_bytes()); // version needed
        local.extend(0x0800u16.to_le_bytes()); // UTF-8 names
        local.extend(8u16.to_le_bytes()); // deflate
        local.extend(time.to_le_bytes());
        local.extend(date.to_le_bytes());
        local.extend(crc.to_le_bytes());
        local.extend(csize.to_le_bytes());
        local.extend(size.to_le_bytes());
        local.extend((n.len() as u16).to_le_bytes());
        local.extend(0u16.to_le_bytes());
        local.extend(n);
        self.buf.extend(local);
        self.buf.extend(&comp);
        let c = &mut self.central;
        c.extend(0x0201_4b50u32.to_le_bytes());
        c.extend(20u16.to_le_bytes()); // version made by
        c.extend(20u16.to_le_bytes());
        c.extend(0x0800u16.to_le_bytes());
        c.extend(8u16.to_le_bytes());
        c.extend(time.to_le_bytes());
        c.extend(date.to_le_bytes());
        c.extend(crc.to_le_bytes());
        c.extend(csize.to_le_bytes());
        c.extend(size.to_le_bytes());
        c.extend((n.len() as u16).to_le_bytes());
        c.extend([0u8; 8]); // extra, comment, disk, internal attrs
        c.extend(0u32.to_le_bytes()); // external attrs
        c.extend(offset.to_le_bytes());
        c.extend(n);
        self.count += 1;
        Ok(())
    }

    /// The archive bytes.
    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buf.len() as u32;
        let cd_size = self.central.len() as u32;
        self.buf.extend(&self.central);
        self.buf.extend(0x0605_4b50u32.to_le_bytes());
        self.buf.extend([0u8; 4]);
        self.buf.extend(self.count.to_le_bytes());
        self.buf.extend(self.count.to_le_bytes());
        self.buf.extend(cd_size.to_le_bytes());
        self.buf.extend(cd_offset.to_le_bytes());
        self.buf.extend(0u16.to_le_bytes());
        self.buf
    }
}

// --------------------------------------------------------------- commands

/// `cua trajectory ls`.
pub fn cmd_ls(machine: Option<String>, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let sessions = list(machine.as_deref());
    if json {
        util::json_line(out, &serde_json::to_value(&sessions)?);
        return Ok(0);
    }
    if sessions.is_empty() {
        match machine {
            Some(m) => line(out, format!("No trajectory sessions found for '{m}'.")),
            None => line(out, "No trajectory sessions found."),
        }
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<20} {:<18} {:>5}  Created",
            "Machine", "Session", "Turns"
        ),
    );
    line(out, "-".repeat(70));
    for s in sessions {
        line(
            out,
            format!(
                "{:<20} {:<18} {:>5}  {}",
                s.machine, s.session, s.turns, s.created
            ),
        );
    }
    Ok(0)
}

fn resolve(target: Option<&str>) -> Option<PathBuf> {
    if let Some(t) = target
        && Path::new(t).is_dir()
    {
        return Some(PathBuf::from(t));
    }
    let sessions = list(None);
    let pick = match target {
        None => sessions.last(),
        Some(t) => sessions
            .iter()
            .rev()
            .find(|s| s.machine == t)
            .or_else(|| sessions.iter().find(|s| s.session == t)),
    };
    pick.map(|s| PathBuf::from(&s.path))
}

/// `cua trajectory view`: zips the session, serves its directory on
/// loopback (a detached `cua trajectory serve` process) and opens the
/// hosted viewer.
pub fn cmd_view(target: Option<String>, port: u16, out: &mut dyn Write) -> Result<i32, CuaError> {
    let Some(session) = resolve(target.as_deref()) else {
        let label = target.map(|t| format!(" for '{t}'")).unwrap_or_default();
        return Err(CuaError::NotFound(format!(
            "no trajectory session found{label}"
        )));
    };
    let zip = zip_session(&session)?;
    stop_server(true);
    let dir = zip.parent().unwrap_or(Path::new(".")).to_path_buf();
    let exe = std::env::current_exe().map_err(internal)?;
    let child = std::process::Command::new(exe)
        .args(["trajectory", "serve", "--port", &port.to_string()])
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(internal)?;
    util::write_private(
        &pid_file(),
        serde_json::json!({"pid": child.id(), "port": port})
            .to_string()
            .as_bytes(),
    )?;
    let zip_name = zip
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let zip_url = format!("http://localhost:{port}/{zip_name}");
    let viewer = format!(
        "https://cua.ai/trajectory-viewer?zip={}",
        url::form_urlencoded::byte_serialize(zip_url.as_bytes()).collect::<String>()
    );
    let machine = session
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let ts = session
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    line(out, format!("Serving: {machine}/{ts}"));
    line(out, format!("Viewer:  {viewer}"));
    line(out, "Stop with: cua trajectory stop");
    util::open_browser(&viewer);
    Ok(0)
}

/// Stops the file server started by `view`. Returns whether one ran.
pub fn stop_server(quiet: bool) -> bool {
    let p = pid_file();
    let Ok(raw) = std::fs::read_to_string(&p) else {
        if !quiet {
            eprintln!("No file server is running.");
        }
        return false;
    };
    let _ = std::fs::remove_file(&p);
    let Some(pid) = serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v["pid"].as_u64())
    else {
        return false;
    };
    let stopped = kill_own_server(pid as u32);
    if !quiet {
        if stopped {
            eprintln!("Stopped file server (pid {pid}).");
        } else {
            eprintln!("Server was not running (stale PID file removed).");
        }
    }
    stopped
}

/// Terminates `pid` only if it is our own `cua trajectory serve` process.
fn kill_own_server(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(o) = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "args="])
            .output()
        else {
            return false;
        };
        let args = String::from_utf8_lossy(&o.stdout);
        if !(args.contains("trajectory") && args.contains("serve")) {
            return false;
        }
        // SAFETY: plain kill(2) on a pid we verified is our server.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) == 0 }
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// `cua trajectory serve DIR`: a CORS-enabled static file server on
/// 127.0.0.1 for the hosted viewer (GET, HEAD, OPTIONS; no directory
/// listings; paths never escape DIR).
pub async fn serve(dir: PathBuf, port: u16) -> Result<i32, CuaError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = dir.canonicalize().map_err(internal)?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(internal)?;
    loop {
        let Ok((mut s, _)) = listener.accept().await else {
            continue;
        };
        let dir = dir.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let mut n = 0;
            // Bounded header read.
            while n < buf.len() {
                match s.read(&mut buf[n..]).await {
                    Ok(0) | Err(_) => return,
                    Ok(k) => n += k,
                }
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf[..n]);
            let mut parts = req.split_whitespace();
            let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
            let cors = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\n";
            let resp = |status: &str, ctype: &str, len: usize| {
                format!(
                    "HTTP/1.1 {status}\r\n{cors}Content-Type: {ctype}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                )
            };
            if method == "OPTIONS" {
                let _ = s
                    .write_all(resp("204 No Content", "text/plain", 0).as_bytes())
                    .await;
                return;
            }
            let rel = path.split('?').next().unwrap_or("").trim_start_matches('/');
            let rel: String = url::form_urlencoded::parse(format!("p={rel}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let file = dir
                .join(&rel)
                .canonicalize()
                .ok()
                .filter(|f| f.starts_with(&dir) && f.is_file());
            match (method, file) {
                ("GET" | "HEAD", Some(f)) => {
                    let data = tokio::fs::read(&f).await.unwrap_or_default();
                    let ctype = if rel.ends_with(".zip") {
                        "application/zip"
                    } else {
                        "application/octet-stream"
                    };
                    let _ = s
                        .write_all(resp("200 OK", ctype, data.len()).as_bytes())
                        .await;
                    if method == "GET" {
                        let _ = s.write_all(&data).await;
                    }
                }
                _ => {
                    let _ = s
                        .write_all(resp("404 Not Found", "text/plain", 9).as_bytes())
                        .await;
                    let _ = s.write_all(b"not found").await;
                }
            }
        });
    }
}

/// `cua trajectory clean`.
pub fn cmd_clean(
    older_than: Option<i64>,
    machine: Option<String>,
    yes: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let mut sessions = list(machine.as_deref());
    if sessions.is_empty() {
        line(out, "No trajectory sessions to clean.");
        return Ok(0);
    }
    if let Some(days) = older_than {
        let cutoff = chrono::Local::now().naive_local() - chrono::Duration::days(days);
        sessions.retain(|s| created_of(Path::new(&s.path)) < cutoff);
    }
    if sessions.is_empty() {
        line(out, "No sessions match the criteria.");
        return Ok(0);
    }
    if !yes {
        eprintln!("Will delete {} session(s):", sessions.len());
        for s in &sessions {
            eprintln!("  {}/{} ({} turns)", s.machine, s.session, s.turns);
        }
        if !util::confirm("Continue?", false) {
            line(out, "Cancelled.");
            return Ok(1);
        }
    }
    let mut n = 0;
    for s in &sessions {
        let p = PathBuf::from(&s.path);
        if std::fs::remove_dir_all(&p).is_ok() {
            n += 1;
        }
        let _ = std::fs::remove_file(p.with_file_name(format!("{}.zip", s.session)));
        if let Some(m) = p.parent()
            && std::fs::read_dir(m)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(m);
        }
    }
    line(out, format!("Deleted {n} session(s)."));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_round_trips_through_the_system_unzip() {
        let d = tempfile::tempdir().unwrap();
        let s = d.path().join("20260101-000000");
        std::fs::create_dir_all(s.join("turn_001")).unwrap();
        std::fs::write(s.join("turn_001/a.json"), b"{\"a\":1}").unwrap();
        std::fs::write(s.join("turn_001/screenshot.png"), vec![7u8; 5000]).unwrap();
        let z = zip_session(&s).unwrap();
        let bytes = std::fs::read(&z).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        // Cross-check with the platform unzip when present (read-only listing).
        if let Ok(o) = std::process::Command::new("unzip")
            .arg("-t")
            .arg(&z)
            .output()
        {
            let t = String::from_utf8_lossy(&o.stdout);
            assert!(t.contains("No errors detected"), "{t}");
        }
    }

    #[test]
    fn sanitize_machine_names() {
        assert_eq!(sanitize("a/b"), "a_b");
        assert_eq!(sanitize(".."), "unknown");
    }
}
