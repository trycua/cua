//! The host side of the Function container bridge (`_cua_bridge.py`).
//!
//! A Function container is a Space running Modal's own entrypoint, which
//! dials the gateway. Rather than give the Space a route to the host's
//! network, the entrypoint runs under a small script whose stdin and stdout
//! carry framed connections: the script listens on the guest's loopback, and
//! this side connects each one to the gateway's own API address and to
//! nothing else. The script also reports the entrypoint's output and exit.

use crate::modal::containers::LogSink;
use bytes::{Buf, Bytes, BytesMut};
use cua_proto::env::v1 as pb;
use cua_spacesd_client::{ProcessEvent, ProcessHandle, SpacesdClient};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// The guest side, placed at `/__modal/_cua_bridge.py`.
pub const SCRIPT: &str = include_str!("_cua_bridge.py");

const OPEN: u8 = 1;
const DATA: u8 = 2;
const CLOSE: u8 = 3;
const LOG: u8 = 4;
const EXIT: u8 = 5;

/// One frame: type, connection, payload length (big endian), payload.
pub fn frame(kind: u8, conn: u32, payload: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(9 + payload.len());
    b.extend_from_slice(&[kind]);
    b.extend_from_slice(&conn.to_be_bytes());
    b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    b.extend_from_slice(payload);
    b.freeze()
}

/// Splits a byte stream into frames.
#[derive(Default)]
pub struct Decoder {
    buf: BytesMut,
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_frame(&mut self) -> Option<(u8, u32, Bytes)> {
        if self.buf.len() < 9 {
            return None;
        }
        let kind = self.buf[0];
        let conn = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]);
        let len = u32::from_be_bytes([self.buf[5], self.buf[6], self.buf[7], self.buf[8]]) as usize;
        if self.buf.len() < 9 + len {
            return None;
        }
        self.buf.advance(9);
        Some((kind, conn, self.buf.split_to(len).freeze()))
    }
}

fn feed(buf: &mut String, data: &[u8], fd: usize, logs: &LogSink) {
    buf.push_str(&String::from_utf8_lossy(data));
    while let Some(i) = buf.find('\n') {
        let line: String = buf.drain(..=i).collect();
        logs(fd, &line);
    }
}

/// What the bridge process produced.
pub enum Out {
    /// Standard output (the frames).
    Stdout(Bytes),
    /// Standard error (the script's own diagnostics).
    Stderr(Bytes),
    /// The process ended with this status.
    Exit(Option<i32>),
}

/// Runs the bridge on a guest process until it ends; the entrypoint's exit
/// code (`None`: lost).
pub async fn run(
    client: SpacesdClient,
    mut handle: ProcessHandle,
    api: String,
    logs: LogSink,
) -> Option<i32> {
    let pid = handle.pid();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Out>();
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Bytes>();
    // Frames to the guest are written to the process's stdin in order.
    let writer = tokio::spawn(async move {
        let writer_id = format!("bridge-{pid}");
        let mut seq = 0u64;
        while let Some(b) = in_rx.recv().await {
            seq += 1;
            let req = pb::SendInputRequest {
                process: Some(pb::ProcessSelector {
                    selector: Some(pb::process_selector::Selector::Pid(pid)),
                }),
                input: Some(pb::ProcessInput {
                    input: Some(pb::process_input::Input::Stdin(b.to_vec())),
                }),
                sequence: seq,
                writer_id: writer_id.clone(),
            };
            if client.process().send_input(req).await.is_err() {
                return;
            }
        }
    });
    let events = tokio::spawn(async move {
        loop {
            match handle.next_event().await {
                Ok(Some(ProcessEvent::Stdout { data, .. })) => {
                    let _ = out_tx.send(Out::Stdout(data));
                }
                Ok(Some(ProcessEvent::Stderr { data, .. })) => {
                    let _ = out_tx.send(Out::Stderr(data));
                }
                Ok(Some(ProcessEvent::Pty { .. })) => {}
                Ok(Some(ProcessEvent::Exit(st))) => {
                    let _ = out_tx.send(Out::Exit(st.code.or(Some(137))));
                    return;
                }
                Ok(None) | Err(_) => {
                    let _ = out_tx.send(Out::Exit(None));
                    return;
                }
            }
        }
    });
    let code = serve(&mut out_rx, in_tx, api, logs).await;
    events.abort();
    writer.abort();
    code
}

/// The bridge protocol over channels: `out` is what the process printed, `to_guest`
/// what is written to its stdin.
pub async fn serve(
    out: &mut mpsc::UnboundedReceiver<Out>,
    to_guest: mpsc::UnboundedSender<Bytes>,
    api: String,
    logs: LogSink,
) -> Option<i32> {
    let mut dec = Decoder::default();
    let mut conns: HashMap<u32, mpsc::UnboundedSender<Option<Bytes>>> = HashMap::new();
    let mut lines = [String::new(), String::new()];
    let mut entry_exit: Option<i32> = None;
    let mut bridge_exit: Option<i32> = None;
    while let Some(ev) = out.recv().await {
        match ev {
            Out::Stdout(data) => {
                dec.push(&data);
                while let Some((kind, conn, payload)) = dec.next_frame() {
                    match kind {
                        OPEN => {
                            let (tx, rx) = mpsc::unbounded_channel();
                            conns.insert(conn, tx);
                            tokio::spawn(connection(conn, rx, api.clone(), to_guest.clone()));
                        }
                        DATA => {
                            if let Some(tx) = conns.get(&conn) {
                                let _ = tx.send(Some(payload));
                            }
                        }
                        CLOSE => {
                            if let Some(tx) = conns.remove(&conn) {
                                let _ = tx.send(None);
                            }
                        }
                        LOG if !payload.is_empty() => {
                            let fd = usize::from(payload[0] == 1);
                            feed(&mut lines[fd], &payload[1..], fd, &logs);
                        }
                        EXIT if payload.len() == 4 => {
                            entry_exit = Some(i32::from_be_bytes([
                                payload[0], payload[1], payload[2], payload[3],
                            ]));
                        }
                        _ => {}
                    }
                }
            }
            // The bridge's own stderr (a traceback, for one) is the container's log.
            Out::Stderr(data) => feed(&mut lines[1], &data, 1, &logs),
            Out::Exit(code) => {
                bridge_exit = code;
                break;
            }
        }
    }
    for (fd, rest) in lines.iter().enumerate() {
        if !rest.is_empty() {
            logs(fd, rest);
        }
    }
    for (_, tx) in conns.drain() {
        let _ = tx.send(None);
    }
    // The entrypoint's own code wins; a bridge that died without one lost it.
    entry_exit.or(bridge_exit.filter(|c| *c != 0))
}

/// One guest connection, relayed to the gateway's API address.
async fn connection(
    id: u32,
    mut rx: mpsc::UnboundedReceiver<Option<Bytes>>,
    api: String,
    out: mpsc::UnboundedSender<Bytes>,
) {
    let Ok(sock) = TcpStream::connect(&api).await else {
        let _ = out.send(frame(CLOSE, id, &[]));
        return;
    };
    let _ = sock.set_nodelay(true);
    let (mut r, mut w) = sock.into_split();
    let up = tokio::spawn(async move {
        while let Some(Some(b)) = rx.recv().await {
            if w.write_all(&b).await.is_err() {
                return;
            }
        }
        let _ = w.shutdown().await;
    });
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.send(frame(DATA, id, &buf[..n])).is_err() {
                    break;
                }
            }
        }
    }
    let _ = out.send(frame(CLOSE, id, &[]));
    up.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_across_arbitrary_splits() {
        let mut all = Vec::new();
        for (k, c, p) in [
            (OPEN, 1u32, &b""[..]),
            (DATA, 1, b"hello"),
            (LOG, 0, b"\x01err"),
            (CLOSE, 7, b""),
        ] {
            all.extend_from_slice(&frame(k, c, p));
        }
        for step in [1usize, 2, 3, 7, 64] {
            let mut d = Decoder::default();
            let mut got = Vec::new();
            for chunk in all.chunks(step) {
                d.push(chunk);
                while let Some(f) = d.next_frame() {
                    got.push(f);
                }
            }
            assert_eq!(got.len(), 4, "step {step}");
            assert_eq!(got[1], (DATA, 1, Bytes::from_static(b"hello")));
            assert_eq!(got[3].1, 7);
        }
    }

    /// The real guest script, run as a local process, against `serve`: a
    /// connection to its port reaches the "gateway" (an echo server here),
    /// its entrypoint's output arrives as log lines, and its exit code is
    /// returned.
    #[tokio::test]
    async fn the_guest_script_relays_connections_output_and_exit() {
        use std::sync::{Arc, Mutex};
        use tokio::io::AsyncReadExt as _;
        let py = std::env::var("CUA_MODAL_LOCAL_RUNTIME").ok().or_else(|| {
            ["python3", "python"]
                .into_iter()
                .find(|p| {
                    std::process::Command::new(p)
                        .arg("--version")
                        .output()
                        .is_ok()
                })
                .map(String::from)
        });
        let Some(py) = py else {
            eprintln!("no python: skipped");
            return;
        };
        // The "gateway": echoes what it reads, upper-cased.
        let gw = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = gw.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = gw.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    loop {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                let up = buf[..n].to_ascii_uppercase();
                                if s.write_all(&up).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let dir = std::env::temp_dir().join(format!("cua-bridge-test-{free}"));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("_cua_bridge.py");
        std::fs::write(&script, SCRIPT).unwrap();
        // The "entrypoint": talks to the bridge's port, prints, exits 3.
        let prog = format!(
            "import socket,sys; s=socket.create_connection(('127.0.0.1',{free}),timeout=10); s.sendall(b'ping'); print('got', s.recv(10).decode()); print('to stderr', file=sys.stderr); sys.exit(3)"
        );
        let mut child = tokio::process::Command::new(&py)
            .arg(&script)
            .args(["--port", &free.to_string(), "--", &py, "-c", &prog])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Bytes>();
        let tx = out_tx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let _ = tx.send(Out::Stdout(Bytes::copy_from_slice(&buf[..n])));
                    }
                }
            }
        });
        tokio::spawn(async move {
            while let Some(b) = in_rx.recv().await {
                if stdin.write_all(&b).await.is_err() {
                    return;
                }
            }
        });
        let waiter = tokio::spawn(async move {
            let st = child.wait().await.unwrap();
            let _ = out_tx.send(Out::Exit(st.code()));
        });
        let lines: Arc<Mutex<Vec<(usize, String)>>> = Arc::default();
        let sink = lines.clone();
        let logs: LogSink = Arc::new(move |fd, l| sink.lock().unwrap().push((fd, l.to_string())));
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            serve(&mut out_rx, in_tx, api, logs),
        )
        .await
        .expect("the bridge finished");
        waiter.abort();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, Some(3), "the entrypoint's exit code");
        let got = lines.lock().unwrap().clone();
        assert!(got.contains(&(0, "got PING\n".to_string())), "{got:?}");
        assert!(got.contains(&(1, "to stderr\n".to_string())), "{got:?}");
    }

    #[test]
    fn the_guest_script_speaks_the_same_frames() {
        assert!(SCRIPT.contains("HDR = struct.Struct(\"!BII\")"));
        assert!(SCRIPT.contains("OPEN, DATA, CLOSE, LOG, EXIT = 1, 2, 3, 4, 5"));
    }
}
