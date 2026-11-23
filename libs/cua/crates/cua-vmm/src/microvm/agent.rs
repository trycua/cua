//! The host side of the guest init's protocol ([`cua_microvm_init`]): over
//! Firecracker's vsock Unix socket (`CONNECT <port>`), or, for a guest with
//! no vsock (QEMU on Windows), over a loopback TCP port its user-mode network
//! forwards to the init, past an `AUTH <key>` line; then one JSON request per
//! connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use cua_microvm_init::{ExecFrame, Reply, Request};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{Result, VmmError};
use crate::exec::{ExecChunk, ExecOutput, ExecRequest, GuestExec};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A byte stream to the init.
pub trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Io for T {}

type Conn = BufReader<Box<dyn Io>>;

/// How the host reaches the init.
#[derive(Clone, Debug)]
enum Link {
    /// Firecracker's vsock Unix socket.
    Vsock(PathBuf),
    /// A loopback TCP port, and the key the init asks for.
    Tcp { addr: String, key: String },
}

/// The guest init of one VM.
#[derive(Clone, Debug)]
pub struct Agent {
    link: Link,
}

impl Agent {
    /// The init of a VM with a vsock socket at `sock`.
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Self {
            link: Link::Vsock(sock.into()),
        }
    }

    /// The init of a VM whose network forwards `addr` (`127.0.0.1:port`) to it.
    pub fn tcp(addr: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            link: Link::Tcp {
                addr: addr.into(),
                key: key.into(),
            },
        }
    }

    pub fn path(&self) -> &Path {
        match &self.link {
            Link::Vsock(p) => p,
            Link::Tcp { .. } => Path::new(""),
        }
    }

    /// One connection to the init, past its handshake.
    async fn connect(&self) -> Result<Conn> {
        match &self.link {
            Link::Vsock(sock) => {
                let mut s = super::connect_unix(sock)
                    .await
                    .map_err(|e| VmmError::other(format!("vsock {}: {e}", sock.display())))?;
                s.write_all(format!("CONNECT {}\n", cua_microvm_init::PORT).as_bytes())
                    .await?;
                let mut r: Conn = BufReader::new(Box::new(s));
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
            Link::Tcp { addr, key } => {
                let mut s = tokio::net::TcpStream::connect(addr)
                    .await
                    .map_err(|e| VmmError::other(format!("guest agent {addr}: {e}")))?;
                s.set_nodelay(true).ok();
                s.write_all(format!("AUTH {key}\n").as_bytes()).await?;
                Ok(BufReader::new(Box::new(s)))
            }
        }
    }

    async fn send(r: &mut Conn, req: &Request) -> Result<()> {
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
        // A TCP link goes through the VM's user-mode network, which holds a
        // connection to a guest that is not listening yet until it times out (seconds):
        // ask again soon, so the first answer is not up to 2 s late.
        let attempt = match self.link {
            Link::Tcp { .. } => Duration::from_millis(250),
            Link::Vsock(_) => Duration::from_secs(2),
        };
        loop {
            let last = match self.request(&Request::Ping, attempt).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A guest init that wants its key: the agent starts every TCP connection with
    /// `AUTH <key>` and an init given another key answers nothing.
    #[tokio::test]
    async fn the_tcp_link_starts_with_the_key() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (s, _) = l.accept().await.unwrap();
                let mut r = BufReader::new(s);
                let mut auth = String::new();
                r.read_line(&mut auth).await.unwrap();
                if auth.trim() != "AUTH secret" {
                    continue;
                }
                let mut req = String::new();
                r.read_line(&mut req).await.unwrap();
                let reply = serde_json::to_string(&Reply::ok()).unwrap();
                r.get_mut()
                    .write_all(
                        format!(
                            "{reply}
"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let good = Agent::tcp(addr.clone(), "secret");
        good.request(&Request::Ping, Duration::from_secs(5))
            .await
            .unwrap();
        let bad = Agent::tcp(addr, "wrong");
        assert!(
            bad.request(&Request::Ping, Duration::from_secs(2))
                .await
                .is_err()
        );
    }
}
