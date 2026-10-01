//! Agentless guest command execution.
//!
//! [`GuestExec`] is deliberately small so other crates can implement it (the
//! cua-spacesd client will add one later). Implementations here:
//!
//! * [`SshExec`]  — OpenSSH client against a forwarded/bridged guest sshd.
//! * `container::DockerExec` — Docker Engine exec API.
//! * [`PrefixExec`] — any host command that runs a shell string in the guest
//!   (used for `lume ssh <vm> --`).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::error::{Result, VmmError};

/// One command to run in the guest.
#[derive(Clone, Debug, Default)]
pub struct ExecRequest {
    /// Shell script run by `/bin/sh -c` in the guest.
    pub script: String,
    /// Extra environment for this command.
    pub env: Vec<(String, String)>,
    /// Bytes to feed on stdin.
    pub stdin: Option<Vec<u8>>,
    /// Run as this user (backends that support it; SSH logs in as its user).
    pub user: Option<String>,
    pub timeout: Option<Duration>,
}

impl ExecRequest {
    pub fn sh(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            ..Default::default()
        }
    }
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
    pub fn user(mut self, u: impl Into<String>) -> Self {
        self.user = Some(u.into());
        self
    }
}

/// Result of [`GuestExec::exec`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
    /// Turn a non-zero exit into an error carrying stderr.
    pub fn check(self, what: &str) -> Result<Self> {
        if self.success() {
            Ok(self)
        } else {
            Err(VmmError::Command {
                cmd: what.to_string(),
                code: Some(self.exit_code as i32),
                stderr: tail(&self.stderr_str(), 4000),
            })
        }
    }
}

/// A piece of guest command output, forwarded as it arrives by
/// [`GuestExec::exec_streaming`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecChunk {
    /// Bytes the command wrote to stdout.
    Stdout(Vec<u8>),
    /// Bytes the command wrote to stderr.
    Stderr(Vec<u8>),
}

/// Run commands and place files inside a guest without an in-guest agent.
#[async_trait]
pub trait GuestExec: Send + Sync {
    /// Run `req.script` with `/bin/sh -c`.
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput>;

    /// [`GuestExec::exec`], forwarding output to `sink` as it arrives, and
    /// returning the exit code. The default runs [`GuestExec::exec`] and
    /// forwards its buffered output once it finishes; implementations
    /// backed by a host process override it to stream.
    async fn exec_streaming(
        &self,
        req: ExecRequest,
        sink: tokio::sync::mpsc::Sender<ExecChunk>,
    ) -> Result<i64> {
        let out = self.exec(req).await?;
        if !out.stdout.is_empty() {
            let _ = sink.send(ExecChunk::Stdout(out.stdout)).await;
        }
        if !out.stderr.is_empty() {
            let _ = sink.send(ExecChunk::Stderr(out.stderr)).await;
        }
        Ok(out.exit_code)
    }

    /// Whether the session is root (so callers know whether to prefix `sudo`).
    async fn is_root(&self) -> Result<bool> {
        Ok(self
            .exec(ExecRequest::sh("id -u"))
            .await?
            .stdout_str()
            .trim()
            == "0")
    }

    /// Write `contents` to `path` in the guest (parents created, mode applied).
    /// The default streams the bytes over stdin, so it works for any
    /// implementation of [`GuestExec::exec`] that forwards stdin.
    async fn put_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<()> {
        let sudo = if self.is_root().await? { "" } else { "sudo " };
        let q = shell_quote(path);
        let script = format!(
            "set -e; {sudo}mkdir -p \"$(dirname {q})\"; {sudo}tee {q} >/dev/null; {sudo}chmod {mode:o} {q}"
        );
        self.exec(ExecRequest::sh(script).stdin(contents.to_vec()))
            .await?
            .check("put_file")?;
        Ok(())
    }
}

/// Single-quote `s` for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub(crate) fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut start = s.len() - max;
        while !s.is_char_boundary(start) {
            start += 1;
        }
        format!("…{}", &s[start..])
    }
}

/// OpenSSH-based [`GuestExec`]. Uses the system `ssh` client so it honours the
/// user's ssh config/agent and needs no extra crypto dependencies.
#[derive(Clone, Debug)]
pub struct SshExec {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub private_key: Option<PathBuf>,
    pub connect_timeout: Duration,
}

impl SshExec {
    pub fn new(host: impl Into<String>, port: u16, user: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            private_key: None,
            connect_timeout: Duration::from_secs(10),
        }
    }

    pub fn key(mut self, key: impl Into<PathBuf>) -> Self {
        self.private_key = Some(key.into());
        self
    }

    /// Build from an [`crate::SshEndpoint`].
    pub fn from_endpoint(ep: &crate::types::SshEndpoint) -> Self {
        let mut s = Self::new(ep.host.clone(), ep.port, ep.user.clone());
        s.private_key = ep.private_key.clone();
        s
    }

    /// The `ssh` argv (without the remote command). Public for tests/debugging.
    pub fn ssh_args(&self) -> Vec<String> {
        let mut args = vec![
            "-p".into(),
            self.port.to_string(),
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "UserKnownHostsFile=/dev/null".into(),
            "-o".into(),
            "LogLevel=ERROR".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            format!("ConnectTimeout={}", self.connect_timeout.as_secs().max(1)),
            "-o".into(),
            "ServerAliveInterval=15".into(),
        ];
        if let Some(k) = &self.private_key {
            args.push("-o".into());
            args.push("IdentitiesOnly=yes".into());
            args.push("-i".into());
            args.push(k.display().to_string());
        }
        args.push(format!("{}@{}", self.user, self.host));
        args
    }

    /// Poll until an SSH login succeeds (sshd up and key accepted).
    pub async fn wait_until_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last = String::new();
        while tokio::time::Instant::now() < deadline {
            match self
                .exec(ExecRequest::sh("true").timeout(Duration::from_secs(20)))
                .await
            {
                Ok(o) if o.success() => return Ok(()),
                Ok(o) => last = o.stderr_str(),
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(VmmError::Timeout {
            name: format!("{}@{}:{}", self.user, self.host, self.port),
            secs: timeout.as_secs(),
            detail: format!("ssh login never succeeded: {}", last.trim()),
        })
    }
}

impl SshExec {
    fn argv(&self, req: &ExecRequest) -> Vec<String> {
        let mut argv = self.ssh_args();
        argv.push("--".into());
        argv.push(format!("/bin/sh -c {}", shell_quote(&remote_script(req))));
        argv
    }
}

/// `req.script` with its environment exported first.
fn remote_script(req: &ExecRequest) -> String {
    let mut remote = String::new();
    for (k, v) in &req.env {
        remote.push_str(&format!("export {}={}; ", k, shell_quote(v)));
    }
    remote.push_str(&req.script);
    remote
}

#[async_trait]
impl GuestExec for SshExec {
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput> {
        let argv = self.argv(&req);
        run_with_stdin("ssh", &argv, req.stdin.as_deref(), req.timeout).await
    }

    async fn exec_streaming(
        &self,
        req: ExecRequest,
        sink: tokio::sync::mpsc::Sender<ExecChunk>,
    ) -> Result<i64> {
        let argv = self.argv(&req);
        run_streaming("ssh", &argv, req.stdin.as_deref(), req.timeout, sink).await
    }
}

/// Runs guest commands through a host command prefix, e.g.
/// `["lume", "ssh", "my-vm", "--"]`; the shell script is appended as one arg.
#[derive(Clone, Debug)]
pub struct PrefixExec {
    pub program: String,
    pub prefix: Vec<String>,
}

impl PrefixExec {
    fn argv(&self, req: &ExecRequest) -> Vec<String> {
        let mut argv = self.prefix.clone();
        argv.push(remote_script(req));
        argv
    }
}

#[async_trait]
impl GuestExec for PrefixExec {
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput> {
        let argv = self.argv(&req);
        run_with_stdin(&self.program, &argv, req.stdin.as_deref(), req.timeout).await
    }

    async fn exec_streaming(
        &self,
        req: ExecRequest,
        sink: tokio::sync::mpsc::Sender<ExecChunk>,
    ) -> Result<i64> {
        let argv = self.argv(&req);
        run_streaming(
            &self.program,
            &argv,
            req.stdin.as_deref(),
            req.timeout,
            sink,
        )
        .await
    }
}

fn spawn_host(
    program: &str,
    args: &[String],
    stdin: Option<&[u8]>,
) -> Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            VmmError::missing(
                program,
                format!("install `{program}` and make sure it is on PATH"),
            )
        } else {
            e.into()
        }
    })?;
    if let Some(data) = stdin {
        let mut pipe = child.stdin.take().expect("stdin piped");
        let data = data.to_vec();
        tokio::spawn(async move {
            let _ = pipe.write_all(&data).await;
            let _ = pipe.shutdown().await;
        });
    }
    Ok(child)
}

/// Forwards everything `pipe` yields to `sink` in chunks of at most 64 KiB.
/// Stops at EOF, a read error, or when the receiver is gone.
async fn pump(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    sink: tokio::sync::mpsc::Sender<ExecChunk>,
    wrap: fn(Vec<u8>) -> ExecChunk,
) {
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if sink.send(wrap(buf[..n].to_vec())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Spawn a host program, feed stdin, forward stdout and stderr to `sink` as
/// they arrive, and return the exit code (`-1` when killed by a signal).
/// On timeout the program is killed and [`VmmError::Timeout`] returned.
pub(crate) async fn run_streaming(
    program: &str,
    args: &[String],
    stdin: Option<&[u8]>,
    timeout: Option<Duration>,
    sink: tokio::sync::mpsc::Sender<ExecChunk>,
) -> Result<i64> {
    let mut child = spawn_host(program, args, stdin)?;
    let out = child.stdout.take().expect("stdout piped");
    let err = child.stderr.take().expect("stderr piped");
    let mut pumps = tokio::spawn({
        let sink = sink.clone();
        async move {
            tokio::join!(
                pump(out, sink.clone(), ExecChunk::Stdout),
                pump(err, sink, ExecChunk::Stderr)
            );
        }
    });
    let status = match timeout {
        Some(t) => match tokio::time::timeout(t, child.wait()).await {
            Ok(s) => s?,
            Err(_) => {
                let _ = child.kill().await;
                pumps.abort();
                return Err(VmmError::Timeout {
                    name: program.to_string(),
                    secs: t.as_secs(),
                    detail: "guest command timed out".into(),
                });
            }
        },
        None => child.wait().await?,
    };
    // Output written just before exit is still in the pipes. A background
    // process the command left behind can hold them open: don't wait on it.
    if tokio::time::timeout(Duration::from_secs(5), &mut pumps)
        .await
        .is_err()
    {
        pumps.abort();
    }
    Ok(status.code().map(i64::from).unwrap_or(-1))
}

/// Spawn a host program, feed stdin, collect output, honour a timeout.
pub(crate) async fn run_with_stdin(
    program: &str,
    args: &[String],
    stdin: Option<&[u8]>,
    timeout: Option<Duration>,
) -> Result<ExecOutput> {
    let child = spawn_host(program, args, stdin)?;
    let fut = child.wait_with_output();
    let out = match timeout {
        Some(t) => tokio::time::timeout(t, fut)
            .await
            .map_err(|_| VmmError::Timeout {
                name: program.to_string(),
                secs: t.as_secs(),
                detail: "guest command timed out".into(),
            })??,
        None => fut.await?,
    };
    Ok(ExecOutput {
        exit_code: out.status.code().map(i64::from).unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn ssh_args_pin_key_and_disable_host_checks() {
        let s = SshExec::new("127.0.0.1", 2222, "debian").key("/tmp/k");
        let a = s.ssh_args();
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(a.windows(2).any(|w| w == ["-i", "/tmp/k"]));
        assert!(a.contains(&"StrictHostKeyChecking=no".to_string()));
        assert_eq!(a.last().unwrap(), "debian@127.0.0.1");
    }

    #[tokio::test]
    async fn prefix_exec_runs_host_shell() {
        let e = PrefixExec {
            program: "/bin/sh".into(),
            prefix: vec!["-c".into()],
        };
        let out = e
            .exec(ExecRequest::sh("echo hi; echo err >&2; exit 3"))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 3);
        assert_eq!(out.stdout_str(), "hi\n");
        assert_eq!(out.stderr_str(), "err\n");
        let out = e
            .exec(ExecRequest::sh("cat").stdin(b"payload".to_vec()))
            .await
            .unwrap();
        assert_eq!(out.stdout_str(), "payload");
    }

    #[tokio::test]
    async fn prefix_exec_streams_output_and_exit_code() {
        let e = PrefixExec {
            program: "/bin/sh".into(),
            prefix: vec!["-c".into()],
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let code = e
            .exec_streaming(ExecRequest::sh("echo one; echo two >&2; exit 7"), tx)
            .await
            .unwrap();
        assert_eq!(code, 7);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // Bounded: the sender is gone, so this ends after the buffered chunks.
        for _ in 0..64 {
            match rx.recv().await {
                Some(ExecChunk::Stdout(b)) => out.extend(b),
                Some(ExecChunk::Stderr(b)) => err.extend(b),
                None => break,
            }
        }
        assert_eq!(out, b"one\n");
        assert_eq!(err, b"two\n");
    }

    #[tokio::test]
    async fn streaming_timeout_kills_the_program() {
        let e = PrefixExec {
            program: "/bin/sh".into(),
            prefix: vec!["-c".into()],
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let err = e
            .exec_streaming(
                ExecRequest::sh("sleep 30").timeout(Duration::from_millis(200)),
                tx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, VmmError::Timeout { .. }), "{err}");
    }
}
