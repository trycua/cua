//! cua-driver, the background computer-use server agents launch as
//! `cua-driver mcp`: find the installed binary, check that it runs and
//! answers an MCP `initialize`, and install it with its own signed release
//! installer (`cua.ai/driver/install.sh`, which verifies each archive and,
//! on macOS, the CuaDriver.app signature) when it is missing.
//!
//! cua-driver is not bundled with cua or the Spaces apps: on macOS it is
//! its own app (`/Applications/CuaDriver.app`), which is what Accessibility
//! and Screen Recording are granted to.

use crate::{HostEnv, Os, is_executable};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// cua-driver's release installer for macOS and Linux.
pub const INSTALL_URL: &str = "https://cua.ai/driver/install.sh";

/// cua-driver's release installer for Windows.
pub const INSTALL_URL_WINDOWS: &str = "https://cua.ai/driver/install.ps1";

/// How long `cua-driver mcp` may take to answer `initialize`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the release installer may run.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The command a person runs to install cua-driver themselves.
pub fn install_command(os: Os) -> String {
    if os == Os::Windows {
        format!("& ([scriptblock]::Create((irm {INSTALL_URL_WINDOWS})))")
    } else {
        format!("/bin/bash -c \"$(curl -fsSL {INSTALL_URL})\"")
    }
}

fn exe_name(os: Os) -> &'static str {
    if os == Os::Windows {
        "cua-driver.exe"
    } else {
        "cua-driver"
    }
}

/// Where an installed cua-driver is, in order: on `env`'s PATH (which
/// includes `~/.local/bin`, where the installer links it), `~/.local/bin`,
/// then inside `CuaDriver.app` in the app folders (macOS).
pub fn locate(env: &HostEnv) -> Option<PathBuf> {
    let exe = exe_name(env.os);
    let mut candidates: Vec<PathBuf> = env.which("cua-driver").into_iter().collect();
    candidates.push(env.home.join(".local").join("bin").join(exe));
    if env.os == Os::Windows
        && let Some(local) = env.var("LOCALAPPDATA")
    {
        candidates.push(
            Path::new(local)
                .join("Programs")
                .join("Cua")
                .join("cua-driver")
                .join("bin")
                .join(exe),
        );
    }
    if env.os == Os::MacOs {
        for d in &env.app_dirs {
            candidates.push(d.join("CuaDriver.app/Contents/MacOS").join(exe));
        }
    }
    candidates.into_iter().find(|p| is_executable(p))
}

/// The absolute path `command` launches: itself when absolute, else the
/// match on `env`'s PATH. GUI agents (VS Code, Claude Desktop) start
/// servers without the user's shell PATH, so a bare name is not enough.
pub fn resolve(env: &HostEnv, command: &str) -> Option<PathBuf> {
    let p = Path::new(command);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else if command == "cua-driver" || command == "cua-driver.exe" {
        locate(env)
    } else {
        env.which(command)
    }
}

/// Checks that `command` runs and that `command mcp` answers an MCP
/// `initialize`. Ok carries the server's reported version (may be empty);
/// Err is a plain reason.
pub fn check(command: &Path) -> Result<String, String> {
    if !command.exists() {
        return Err(format!(
            "cua-driver isn't installed ({} doesn't exist)",
            command.display()
        ));
    }
    if !is_executable(command) {
        return Err(format!(
            "cua-driver at {} isn't an executable file",
            command.display()
        ));
    }
    let mut cmd = Command::new(command);
    no_window(&mut cmd);
    let mut child = cmd
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cua-driver at {} doesn't run: {e}", command.display()))?;
    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cua-agent-setup", "version": env!("CARGO_PKG_VERSION")},
        },
    });
    let mut stdin = child.stdin.take();
    let wrote = stdin
        .as_mut()
        .map(|s| writeln!(s, "{init}").and_then(|_| s.flush()))
        .unwrap_or(Ok(()));
    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if v.get("id") == Some(&serde_json::json!(1)) {
                    let _ = tx.send(v);
                    break;
                }
            }
        });
    }
    let answer = rx.recv_timeout(CHECK_TIMEOUT);
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let answer = match answer {
        Ok(v) => v,
        Err(_) => {
            let early = wrote
                .err()
                .map(|e| format!(" (it exited early: {e})"))
                .unwrap_or_default();
            return Err(format!(
                "`{} mcp` didn't answer an MCP initialize{early}",
                command.display()
            ));
        }
    };
    if let Some(e) = answer.get("error") {
        let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("error");
        return Err(format!(
            "`{} mcp` refused an MCP initialize: {msg}",
            command.display()
        ));
    }
    let result = answer.get("result").ok_or_else(|| {
        format!(
            "`{} mcp` sent an MCP answer without a result",
            command.display()
        )
    })?;
    Ok(result
        .pointer("/serverInfo/version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string())
}

/// Runs cua-driver's release installer (the same one `cua.ai/install.sh
/// --only cua-driver` runs) for `env`'s home. Err is a plain reason.
pub fn install(env: &HostEnv) -> Result<(), String> {
    let log_dir = env.cua_home().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("cua-driver-install.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|e| format!("can't write {}: {e}", log_path.display()))?;
    let log2 = log
        .try_clone()
        .map_err(|e| format!("can't write {}: {e}", log_path.display()))?;
    let mut script = None;
    let mut cmd = if env.os == Os::Windows {
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &install_command(Os::Windows),
        ]);
        c
    } else {
        // Download first, then run: a failed download never runs a partial
        // script (the shape cua.ai/install.sh uses).
        let file = std::env::temp_dir().join(format!(
            "cua-driver-install-{}-{}.sh",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let mut dl = Command::new("curl");
        dl.args(["-fsSL", "--retry", "2", "-o"])
            .arg(&file)
            .arg(INSTALL_URL)
            .stdin(Stdio::null());
        let got = dl
            .output()
            .map_err(|e| format!("couldn't run curl to download the cua-driver installer: {e}"))?;
        if !got.status.success() {
            let _ = std::fs::remove_file(&file);
            return Err(format!(
                "couldn't download the cua-driver installer from {INSTALL_URL}: {}",
                String::from_utf8_lossy(&got.stderr).trim()
            ));
        }
        let mut c = Command::new("/bin/bash");
        c.arg(&file);
        script = Some(file);
        c
    };
    no_window(&mut cmd);
    cmd.env("HOME", &env.home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log2));
    let result = run_bounded(cmd, INSTALL_TIMEOUT);
    if let Some(f) = script {
        let _ = std::fs::remove_file(f);
    }
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "the cua-driver installer failed{} (log: {})",
            last_error_line(&log_path)
                .map(|l| format!(": {l}"))
                .unwrap_or_default(),
            log_path.display()
        )),
        Err(e) => Err(format!("{e} (log: {})", log_path.display())),
    }
}

/// A usable cua-driver: the installed one when it passes [`check`], else
/// (when `install`) a fresh install from the release, checked again. Err is
/// a plain reason.
pub fn ensure(env: &HostEnv, install: bool) -> Result<PathBuf, String> {
    let first = match locate(env) {
        Some(p) => match check(&p) {
            Ok(_) => return Ok(p),
            Err(e) => e,
        },
        None => "cua-driver isn't installed".to_string(),
    };
    if !install {
        return Err(first);
    }
    self::install(env)?;
    let p = locate(env).ok_or_else(|| {
        "the cua-driver installer finished, but no cua-driver was found".to_string()
    })?;
    check(&p)?;
    Ok(p)
}

fn run_bounded(mut cmd: Command, limit: Duration) -> Result<bool, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("couldn't start the cua-driver installer: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the cua-driver installer didn't finish within {} minutes",
                    limit.as_secs() / 60
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(format!("the cua-driver installer: {e}")),
        }
    }
}

/// The installer's last `error:` line, else its last line.
fn last_error_line(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .find(|l| l.to_ascii_lowercase().starts_with("error"))
        .or(lines.last())
        .map(|l| l.chars().take(200).collect())
}

fn no_window(_cmd: &mut Command) {
    // The app runs this from a GUI process: no console window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _cmd.creation_flags(CREATE_NO_WINDOW);
    }
}
