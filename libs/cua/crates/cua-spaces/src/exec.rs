//! Shell and literal-file primitives over the spacesd's Process and
//! Filesystem services. No ssh, no `/cmd`, no base64-through-a-shell: bytes
//! travel as bytes and a command's exit status is a field, not a parse.

use crate::error::Result;
use crate::space::Space;
use cua_spacesd_client::{Command, UploadOptions, pb};
use std::time::Duration;

/// Characters of each output stream kept when rendering (the tail, where
/// results usually are), so a noisy install cannot flood a model's context.
pub const MAX_OUTPUT_CHARS: usize = 16_000;

/// Default `space_bash` timeout.
pub const DEFAULT_BASH_TIMEOUT: Duration = Duration::from_secs(60);

/// The tail of `text`, with a marker when something was dropped.
pub fn cap_output(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_OUTPUT_CHARS {
        return text.to_string();
    }
    let dropped = count - MAX_OUTPUT_CHARS;
    let tail: String = text.chars().skip(dropped).collect();
    format!("[… {dropped} chars truncated; showing the last {MAX_OUTPUT_CHARS} …]\n{tail}")
}

/// The result of [`Space::bash`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BashOutput {
    /// stdout (lossy UTF-8).
    pub stdout: String,
    /// stderr (lossy UTF-8).
    pub stderr: String,
    /// Exit code for a normal exit.
    pub exit_code: Option<i32>,
    /// Terminating signal, when killed.
    pub signal: Option<String>,
    /// Stopped by its timeout.
    pub timed_out: bool,
    /// Supervisor error (for example "executable not found").
    pub error: Option<String>,
}

impl BashOutput {
    /// Exit code 0.
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// The `space_bash` rendering: stdout, `[stderr]` + stderr, `[exit N]`,
    /// each stream capped at [`MAX_OUTPUT_CHARS`].
    pub fn render(&self) -> String {
        let mut parts = Vec::new();
        let out = cap_output(&self.stdout);
        if !out.trim().is_empty() {
            parts.push(out.trim_end().to_string());
        }
        if !self.stderr.trim().is_empty() {
            parts.push(format!("[stderr]\n{}", cap_output(&self.stderr).trim_end()));
        }
        if let Some(e) = &self.error {
            parts.push(format!("[error] {e}"));
        }
        if self.timed_out {
            parts.push("[timed out]".into());
        }
        match (self.exit_code, &self.signal) {
            (Some(code), _) => parts.push(format!("[exit {code}]")),
            (None, Some(sig)) => parts.push(format!("[signal {sig}]")),
            (None, None) => {}
        }
        if parts.is_empty() {
            "[no output]".into()
        } else {
            parts.join("\n")
        }
    }
}

/// The result of [`Space::write`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct WriteReport {
    /// Path written.
    pub path: String,
    /// Bytes written.
    pub bytes: u64,
    /// SHA-256 verified against the driver's.
    pub sha256: String,
}

impl Space {
    /// A shell command for this guest (`/bin/sh -c` or `cmd /C`).
    pub fn shell_command(&self, line: &str) -> Command {
        if self.is_windows() {
            Command::new("cmd.exe").arg("/C").arg(line)
        } else {
            Command::shell(line)
        }
    }

    /// Runs `command` in the Space's shell and collects its output.
    pub async fn bash(&self, command: &str, timeout: Duration) -> Result<BashOutput> {
        let out = self
            .spacesd()?
            .run(self.shell_command(command).timeout(timeout))
            .await?;
        Ok(BashOutput {
            stdout: out.stdout_str(),
            stderr: out.stderr_str(),
            exit_code: out.status.code,
            signal: out.status.signal.map(|s| format!("{s:?}").to_uppercase()),
            timed_out: out.status.timed_out,
            error: out.status.error.clone(),
        })
    }

    /// Writes `content` to `path` (parents created, existing file replaced),
    /// verified by SHA-256.
    pub async fn write(&self, path: &str, content: impl Into<bytes::Bytes>) -> Result<WriteReport> {
        let content: bytes::Bytes = content.into();
        let res = self
            .spacesd()?
            .upload(
                path,
                content,
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    create_parents: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(WriteReport {
            path: res
                .entry
                .as_ref()
                .map(|e| e.path.clone())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| path.to_string()),
            bytes: res.size,
            sha256: res.sha256,
        })
    }

    /// The Space user's home directory, as its shell sees it.
    pub async fn home(&self) -> Result<String> {
        let line = if self.is_windows() {
            "echo %USERPROFILE%"
        } else {
            "printf %s \"$HOME\""
        };
        let out = self.bash(line, Duration::from_secs(30)).await?;
        let home = out.stdout.trim().to_string();
        if !out.success() || home.is_empty() {
            return Err(crate::Error::Agent(format!(
                "could not read the Space user's home directory: {}",
                out.render()
            )));
        }
        Ok(home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_keep_the_tail() {
        let long = "x".repeat(MAX_OUTPUT_CHARS + 5) + "END";
        let capped = cap_output(&long);
        assert!(capped.starts_with("[… 8 chars truncated"));
        assert!(capped.ends_with("END"));
        assert_eq!(cap_output("short"), "short");
    }

    #[test]
    fn render_matches_the_python_shape() {
        let out = BashOutput {
            stdout: "hi\n".into(),
            stderr: "warn\n".into(),
            exit_code: Some(3),
            ..Default::default()
        };
        assert_eq!(out.render(), "hi\n[stderr]\nwarn\n[exit 3]");
        assert_eq!(
            BashOutput {
                exit_code: Some(0),
                ..Default::default()
            }
            .render(),
            "[exit 0]"
        );
    }
}
