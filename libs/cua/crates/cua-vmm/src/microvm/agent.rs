//! The host side of the guest init's vsock protocol
//! ([`cua_microvm_init`]): Firecracker's vsock Unix socket, `CONNECT
//! <port>`, then one JSON request per connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use cua_microvm_init::{ExecFrame, Reply, Request};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{Result, VmmError};
use crate::exec::{ExecChunk, ExecOutput, ExecRequest, GuestExec};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// The guest init of one VM, through its vsock socket.
#[derive(Clone, Debug)]
pub struct Agent {
    sock: PathBuf,
}

impl Agent {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Self { sock: sock.into() }
    }

    pub fn path(&self) -> &Path {
        &self.sock
    }

    /// One connection to the init, past the `CONNECT` handshake.
    async fn connect(&self) -> Result<BufReader<super::UnixSock>> {
        let mut s = super::connect_unix(&self.sock)
            .await
            .map_err(|e| VmmError::other(format!("vsock {}: {e}", self.sock.display())))?;
        s.write_all(format!("CONNECT {}\n", cua_microvm_init::PORT).as_bytes())
            .await?;
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).await?;
        if !line.starts_with("OK ") {
            return Err(VmmError::other(format!(
                "the guest init is not listening yet ({})",
                line.trim()
            )));
        }
        Ok(r)
    }

    async fn send(r: &mut BufReader<super::UnixSock>, req: &Request) -> Result<()> {
        let mut b = serde_json::to_vec(req)?;
        b.push(b'\n');
        r.get_mut().write_all(&b).await?;
        Ok(())
    }

    /// Sends `req` and reads its [`Reply`] (within `timeout`).
    pub async fn request(&self, req: &Request, timeout: Duration) -> Result<Reply> {
        let fut = async {
            let mut r = self.connect().await?;
            Self::send(&mut r, req).await?;
            let mut line = String::new();
            if r.read_line(&mut line).await? == 0 {
                return Err(VmmError::other("the guest init closed the connection"));
            }
            let reply: Reply = serde_json::from_str(line.trim())?;
            if !reply.ok {
                return Err(VmmError::other(format!(
                    "guest init: {}",
                    reply.error.clone().unwrap_or_default()
                )));
            }
            Ok(reply)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| VmmError::other(format!("guest init: no answer within {timeout:?}")))?
    }

    /// Waits until the init answers a ping (it starts listening right
    /// after it mounts the root), polling every few milliseconds.
    pub async fn wait(&self, timeout: Duration, alive: impl Fn() -> bool) -> Result<Reply> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let last = match self.request(&Request::Ping, Duration::from_secs(2)).await {
                Ok(r) => return Ok(r),
                Err(e) => e.to_string(),
            };
            if !alive() {
                return Err(VmmError::other(format!(
                    "the microVM exited before its init answered ({last})"
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VmmError::other(format!(
                    "the microVM's init did not answer within {timeout:?} ({last})"
                )));
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    }

    /// Whether a TCP port listens in the guest.
    pub async fn listening(&self, port: u16) -> Result<bool> {
        Ok(self
            .request(&Request::Listening { port }, Duration::from_secs(5))
            .await?
            .listening
            .unwrap_or(false))
    }

    /// Runs `req`, forwarding output to `sink`; returns the exit code.
    pub async fn exec(
        &self,
        req: ExecRequest,
        sink: tokio::sync::mpsc::Sender<ExecChunk>,
    ) -> Result<i64> {
        let wire = cua_microvm_init::Exec {
            script: req.script,
            env: req.env,
            stdin: req.stdin.map(|b| B64.encode(b)),
            user: req.user,
            timeout_ms: req.timeout.map(|t| t.as_millis() as u64),
        };
        let mut r = self.connect().await?;
        Self::send(&mut r, &Request::Exec(wire)).await?;
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).await? == 0 {
                return Err(VmmError::other(
                    "the guest closed the exec stream before the command exited",
                ));
            }
            match serde_json::from_str::<ExecFrame>(line.trim())? {
                ExecFrame::Stdout(d) => {
                    let _ = sink
                        .send(ExecChunk::Stdout(B64.decode(d).unwrap_or_default()))
                        .await;
                }
                ExecFrame::Stderr(d) => {
                    let _ = sink
                        .send(ExecChunk::Stderr(B64.decode(d).unwrap_or_default()))
                        .await;
                }
                ExecFrame::Exit(code) => return Ok(code),
                ExecFrame::Error(e) => return Err(VmmError::other(format!("guest exec: {e}"))),
            }
        }
    }
}

/// [`GuestExec`] through the guest init (no sshd or agent in the image).
pub struct VsockExec {
    pub agent: Agent,
}

#[async_trait]
impl GuestExec for VsockExec {
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let collect = async move {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            while let Some(c) = rx.recv().await {
                match c {
                    ExecChunk::Stdout(b) => out.extend(b),
                    ExecChunk::Stderr(b) => err.extend(b),
                }
            }
            (out, err)
        };
        let (code, (stdout, stderr)) = tokio::join!(self.agent.exec(req, tx), collect);
        Ok(ExecOutput {
            exit_code: code?,
            stdout,
            stderr,
        })
    }

    async fn exec_streaming(
        &self,
        req: ExecRequest,
        sink: tokio::sync::mpsc::Sender<ExecChunk>,
    ) -> Result<i64> {
        self.agent.exec(req, sink).await
    }
}
