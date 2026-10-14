//! Guest commands in a macOS Lume VM over `lume ssh`.
//!
//! `lume ssh <vm> <command>` handles the guest's password login, but it
//! merges the command's stderr into stdout, appends a newline of its own
//! and does not forward stdin. [`LumeSshExec`] keeps the exec contract
//! (separate stdout and stderr, the command's exit code) by running the
//! command under a small `/bin/sh` wrapper in the guest: stdout streams
//! live; stderr goes to a guest temp file and follows a framed trailer
//! (exit code, base64 stderr) once the command ends.

use async_trait::async_trait;
use base64::Engine as _;
use tokio::sync::mpsc;

use crate::error::{Result, VmmError};
use crate::exec::{ExecChunk, ExecOutput, ExecRequest, GuestExec, shell_quote};

/// [`GuestExec`] over `lume ssh`. Stdin is not supported (`lume ssh` does
/// not forward it).
#[derive(Clone, Debug)]
pub struct LumeSshExec {
    /// The `lume` binary.
    pub program: String,
    /// VM name.
    pub vm: String,
}

impl LumeSshExec {
    pub fn new(program: impl Into<String>, vm: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            vm: vm.into(),
        }
    }

    /// The `lume` argv for `script` framed with `nonce`. Public for tests.
    pub fn argv(&self, req: &ExecRequest, nonce: &str) -> Vec<String> {
        let mut inner = String::new();
        for (k, v) in &req.env {
            inner.push_str(&format!("export {}={}; ", k, shell_quote(v)));
        }
        inner.push_str(&req.script);
        let wrapper = format!(
            "e=$(mktemp -t cua-exec) || exit 125; \
             ( /bin/sh -c {inner} ) 2>\"$e\"; rc=$?; \
             printf '\\036{marker}\\036%d\\036' \"$rc\"; \
             base64 < \"$e\"; rm -f \"$e\"; exit \"$rc\"",
            inner = shell_quote(&inner),
            marker = marker_name(nonce),
        );
        vec![
            "ssh".into(),
            // Our own timeout applies; lume's default (60 s) would cut
            // long commands short.
            "--timeout".into(),
            "0".into(),
            self.vm.clone(),
            format!("/bin/sh -c {}", shell_quote(&wrapper)),
        ]
    }
}

fn marker_name(nonce: &str) -> String {
    format!("CUA-EXEC-{nonce}")
}

fn nonce() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", t, std::process::id())
}

/// Splits the wrapper's stdout into the command's own stdout (forwarded as
/// it arrives) and the trailer after the marker.
struct Deframer {
    marker: Vec<u8>,
    /// Bytes not yet forwarded because they may start the marker.
    held: Vec<u8>,
    /// Everything after the marker, once seen.
    trailer: Option<Vec<u8>>,
}

impl Deframer {
    fn new(nonce: &str) -> Self {
        let mut marker = vec![0x1e];
        marker.extend_from_slice(marker_name(nonce).as_bytes());
        marker.push(0x1e);
        Self {
            marker,
            held: Vec::new(),
            trailer: None,
        }
    }

    /// Feeds stdout bytes; returns what can be forwarded now.
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if let Some(t) = self.trailer.as_mut() {
            t.extend_from_slice(data);
            return Vec::new();
        }
        self.held.extend_from_slice(data);
        if let Some(at) = find(&self.held, &self.marker) {
            let trailer = self.held.split_off(at + self.marker.len());
            self.held.truncate(at);
            self.trailer = Some(trailer);
            return std::mem::take(&mut self.held);
        }
        // Keep the longest suffix that is a prefix of the marker.
        let keep = (1..self.marker.len())
            .rev()
            .find(|&n| n <= self.held.len() && self.held.ends_with(&self.marker[..n]))
            .unwrap_or(0);
        let rest = self.held.split_off(self.held.len() - keep);
        std::mem::replace(&mut self.held, rest)
    }

    /// At EOF: the unforwarded stdout, and the parsed trailer
    /// (exit code, stderr) when the marker was seen.
    fn finish(self) -> (Vec<u8>, Option<(i64, Vec<u8>)>) {
        let Some(trailer) = self.trailer else {
            return (self.held, None);
        };
        let text = String::from_utf8_lossy(&trailer);
        let (code, b64) = text.split_once('\u{1e}').unwrap_or((&text, ""));
        let Ok(code) = code.trim().parse::<i64>() else {
            return (self.held, None);
        };
        let clean: String = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let stderr = base64::engine::general_purpose::STANDARD
            .decode(clean)
            .unwrap_or_default();
        (self.held, Some((code, stderr)))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[async_trait]
impl GuestExec for LumeSshExec {
    async fn exec(&self, req: ExecRequest) -> Result<ExecOutput> {
        let (tx, mut rx) = mpsc::channel(64);
        let run = self.exec_streaming(req, tx);
        let collect = async {
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            while let Some(c) = rx.recv().await {
                match c {
                    ExecChunk::Stdout(b) => stdout.extend(b),
                    ExecChunk::Stderr(b) => stderr.extend(b),
                }
            }
            (stdout, stderr)
        };
        let (code, (stdout, stderr)) = tokio::join!(run, collect);
        Ok(ExecOutput {
            exit_code: code?,
            stdout,
            stderr,
        })
    }

    async fn exec_streaming(&self, req: ExecRequest, sink: mpsc::Sender<ExecChunk>) -> Result<i64> {
        if req.stdin.is_some() {
            return Err(VmmError::Unsupported {
                backend: "lume",
                op: "stdin for commands over `lume ssh`",
            });
        }
        let nonce = nonce();
        let argv = self.argv(&req, &nonce);
        let (raw_tx, mut raw_rx) = mpsc::channel::<ExecChunk>(64);
        let program = self.program.clone();
        let timeout = req.timeout;
        let run = tokio::spawn(async move {
            crate::exec::run_streaming(&program, &argv, None, timeout, raw_tx).await
        });
        let mut de = Deframer::new(&nonce);
        while let Some(chunk) = raw_rx.recv().await {
            match chunk {
                ExecChunk::Stdout(b) => {
                    let out = de.feed(&b);
                    if !out.is_empty() && sink.send(ExecChunk::Stdout(out)).await.is_err() {
                        break;
                    }
                }
                // lume's own diagnostics (connection failures).
                ExecChunk::Stderr(b) => {
                    let _ = sink.send(ExecChunk::Stderr(b)).await;
                }
            }
        }
        let code = run
            .await
            .map_err(|e| VmmError::Other(format!("lume ssh: {e}")))??;
        let (rest, trailer) = de.finish();
        if !rest.is_empty() {
            let _ = sink.send(ExecChunk::Stdout(rest)).await;
        }
        match trailer {
            Some((rc, stderr)) => {
                if !stderr.is_empty() {
                    let _ = sink.send(ExecChunk::Stderr(stderr)).await;
                }
                Ok(rc)
            }
            // The wrapper never ran (lume could not log in): lume's code.
            None => Ok(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn deframe(chunks: &[&[u8]], nonce: &str) -> (Vec<u8>, Option<(i64, Vec<u8>)>) {
        let mut de = Deframer::new(nonce);
        let mut out = Vec::new();
        for c in chunks {
            out.extend(de.feed(c));
        }
        let (rest, t) = de.finish();
        out.extend(rest);
        (out, t)
    }

    #[test]
    fn deframes_a_marker_split_across_chunks() {
        let (out, t) = deframe(
            &[b"hello\n\x1eCUA-EX", b"EC-n1\x1e3\x1e", b"ZXJy\n", b"\n"],
            "n1",
        );
        assert_eq!(out, b"hello\n");
        assert_eq!(t, Some((3, b"err".to_vec())));
    }

    #[test]
    fn output_without_a_marker_passes_through() {
        let (out, t) = deframe(&[b"Error: VM not found\n"], "n1");
        assert_eq!(out, b"Error: VM not found\n");
        assert_eq!(t, None);
    }

    #[test]
    fn a_marker_prefix_in_output_is_not_swallowed() {
        let (out, t) = deframe(&[b"a\x1eCUA", b"-x\n\x1eCUA-EXEC-n1\x1e0\x1e\n"], "n1");
        assert_eq!(out, b"a\x1eCUA-x\n");
        assert_eq!(t, Some((0, vec![])));
    }

    /// A fake `lume` that runs the command it gets locally, the way
    /// `lume ssh` does: stderr merged into stdout, a trailing newline.
    fn fake_lume(dir: &std::path::Path) -> String {
        let p = dir.join("lume");
        std::fs::write(
            &p,
            "#!/bin/sh\n# lume ssh --timeout 0 <vm> <command>\n\
             [ \"$1\" = ssh ] || exit 64\n\
             [ \"$2\" = --timeout ] || exit 64\n\
             [ \"$4\" = cua-e2e-vm ] || { echo \"Error: VM '$4' not found\"; exit 1; }\n\
             /bin/sh -c \"$5\" 2>&1; rc=$?; echo; exit $rc\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p.display().to_string()
    }

    #[tokio::test]
    async fn separates_stderr_and_keeps_the_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let e = LumeSshExec::new(fake_lume(dir.path()), "cua-e2e-vm");
        let out = e
            .exec(
                ExecRequest::sh("echo out; echo \"it's $X\" >&2; exit 4")
                    .timeout(Duration::from_secs(20))
                    .env_var("X", "set"),
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 4);
        assert_eq!(out.stdout_str(), "out\n");
        assert_eq!(out.stderr_str(), "it's set\n");
    }

    #[tokio::test]
    async fn streams_stdout_before_the_command_ends() {
        let dir = tempfile::tempdir().unwrap();
        let e = LumeSshExec::new(fake_lume(dir.path()), "cua-e2e-vm");
        let (tx, mut rx) = mpsc::channel(16);
        let run = tokio::spawn(async move {
            e.exec_streaming(
                ExecRequest::sh("echo first; sleep 1; echo second")
                    .timeout(Duration::from_secs(20)),
                tx,
            )
            .await
        });
        let first = tokio::time::timeout(Duration::from_millis(900), rx.recv())
            .await
            .expect("stdout arrives while the command still runs");
        assert_eq!(first, Some(ExecChunk::Stdout(b"first\n".to_vec())));
        assert_eq!(run.await.unwrap().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_missing_vm_reports_lumes_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = LumeSshExec::new(fake_lume(dir.path()), "cua-e2e-gone");
        let out = e
            .exec(ExecRequest::sh("true").timeout(Duration::from_secs(20)))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 1);
        assert!(out.stdout_str().contains("not found"));
    }

    #[tokio::test]
    async fn stdin_is_refused() {
        let e = LumeSshExec::new("lume", "cua-e2e-vm");
        let err = e
            .exec(ExecRequest::sh("cat").stdin(b"x".to_vec()))
            .await
            .unwrap_err();
        assert!(matches!(err, VmmError::Unsupported { .. }));
    }
}
