//! Best-effort ffmpeg installer for the `install_ffmpeg` tool.
//!
//! The Linux/Windows video-recording backend shells out to the ffmpeg
//! *binary* (macOS records natively via ScreenCaptureKit and needs no
//! ffmpeg). When ffmpeg is absent, `start_recording(record_video: true)`
//! can't produce an mp4. This module detects the platform package manager,
//! reports the exact install command, and — only on explicit confirmation —
//! runs it. We never link ffmpeg; this installs the same user-provided
//! binary the subprocess backend already looks for via `find_ffmpeg`.

use std::process::Command;

/// A resolved install action: a human-readable manager name + the argv to run.
pub struct InstallPlan {
    pub manager: String,
    pub argv: Vec<String>,
}

impl InstallPlan {
    pub fn display(&self) -> String {
        self.argv.join(" ")
    }
}

/// Is `name` an executable on PATH?
fn cmd_exists(name: &str) -> bool {
    #[cfg(target_os = "windows")]
    let probe = Command::new("where").arg(name).output();
    #[cfg(not(target_os = "windows"))]
    let probe = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output();
    probe.map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    // Avoid a libc dependency: ask `id -u`.
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

/// Detect how to install ffmpeg on this platform, or `None` if no supported
/// package manager is available (caller should then tell the user to install
/// ffmpeg manually).
pub fn install_plan() -> Option<InstallPlan> {
    #[cfg(target_os = "linux")]
    {
        // (manager binary, install argv) — first match on PATH wins.
        let candidates: &[(&str, &[&str])] = &[
            ("apt-get", &["apt-get", "install", "-y", "ffmpeg"]),
            ("dnf", &["dnf", "install", "-y", "ffmpeg"]),
            ("yum", &["yum", "install", "-y", "ffmpeg"]),
            (
                "zypper",
                &["zypper", "--non-interactive", "install", "ffmpeg"],
            ),
            ("pacman", &["pacman", "-S", "--noconfirm", "ffmpeg"]),
            ("apk", &["apk", "add", "ffmpeg"]),
            ("snap", &["snap", "install", "ffmpeg"]),
        ];
        for (bin, argv) in candidates {
            if !cmd_exists(bin) {
                continue;
            }
            let mut full: Vec<String> = Vec::new();
            // System package managers need root. If we're not root and sudo
            // is available, run it non-interactively — a password-required
            // sudo fails fast rather than hanging the (TTY-less) daemon.
            if !is_root() && cmd_exists("sudo") {
                full.push("sudo".into());
                full.push("-n".into());
            }
            full.extend(argv.iter().map(|s| (*s).to_string()));
            return Some(InstallPlan {
                manager: (*bin).into(),
                argv: full,
            });
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        if cmd_exists("brew") {
            return Some(InstallPlan {
                manager: "brew".into(),
                argv: vec!["brew".into(), "install".into(), "ffmpeg".into()],
            });
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        if cmd_exists("winget") {
            return Some(InstallPlan {
                manager: "winget".into(),
                argv: [
                    "winget",
                    "install",
                    "-e",
                    "--id",
                    "Gyan.FFmpeg",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                ]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            });
        }
        if cmd_exists("choco") {
            return Some(InstallPlan {
                manager: "choco".into(),
                argv: ["choco", "install", "ffmpeg", "-y"]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            });
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Run an install plan. Returns `(command_succeeded, combined_output_tail)`.
pub fn run_install(plan: &InstallPlan) -> anyhow::Result<(bool, String)> {
    let out = Command::new(&plan.argv[0])
        .args(&plan.argv[1..])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to spawn `{}`: {e}", plan.argv[0]))?;
    let mut buf = String::new();
    buf.push_str(&String::from_utf8_lossy(&out.stdout));
    buf.push_str(&String::from_utf8_lossy(&out.stderr));
    let tail = if buf.len() > 3000 {
        format!("…{}", &buf[buf.len() - 3000..])
    } else {
        buf
    };
    Ok((out.status.success(), tail))
}
