//! Commands and interactive PTY shells over cua-spacesd's Process
//! service.

use cua_sdk::{CuaError, PtySize, SpacesdClient, SpacesdCommand};
use std::{collections::HashMap, io::Write};

/// An `SpacesdCommand` for argv.
pub fn command_of(argv: &[String]) -> SpacesdCommand {
    SpacesdCommand {
        program: argv[0].clone(),
        args: argv[1..].to_vec(),
        env: HashMap::new(),
        cwd: None,
        user: None,
        timeout_ms: None,
        tag: None,
        stdin: false,
        pty: None,
    }
}

/// Runs argv to completion, streaming nothing; prints stdout/stderr and
/// returns the exit code.
pub async fn exec(
    env: &SpacesdClient,
    argv: &[String],
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let o = env.run(command_of(argv)).await?;
    out.write_all(&o.stdout).ok();
    std::io::stderr().write_all(&o.stderr).ok();
    Ok(exit_code(&o))
}

/// Exit code of a finished process.
pub fn exit_code(o: &cua_sdk::ProcessOutput) -> i32 {
    o.exit.code.unwrap_or(if o.exit.success { 0 } else { 1 })
}

fn terminal_size() -> (u32, u32) {
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ fills a plain struct.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                return (ws.ws_col as u32, ws.ws_row as u32);
            }
        }
    }
    (120, 40)
}

/// Puts the local terminal in raw mode until dropped (Unix, TTY only).
struct RawMode {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl RawMode {
    fn enable() -> Self {
        #[cfg(unix)]
        {
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                return Self { saved: None };
            }
            // SAFETY: termios calls on stdin with a zeroed, then filled struct.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                    return Self { saved: None };
                }
                let saved = t;
                libc::cfmakeraw(&mut t);
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
                Self { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        Self {}
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(t) = self.saved {
            // SAFETY: restores the attributes read in `enable`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
    }
}

/// An interactive PTY session: raw local terminal, window-size tracking,
/// stdin forwarded byte for byte. Runs `command` (default: the login
/// shell, `/bin/sh -i` fallback).
pub async fn interactive(
    env: &SpacesdClient,
    command: Option<Vec<String>>,
    cols: Option<u32>,
    rows: Option<u32>,
) -> Result<i32, CuaError> {
    let (c, r) = terminal_size();
    let argv = command.filter(|c| !c.is_empty()).unwrap_or_else(|| {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "exec ${SHELL:-/bin/sh} -i".into(),
        ]
    });
    let mut cmd = command_of(&argv);
    cmd.stdin = true;
    cmd.pty = Some(PtySize {
        cols: cols.unwrap_or(c),
        rows: rows.unwrap_or(r),
    });
    let p = env.spawn(cmd).await?;
    let raw = RawMode::enable();
    let writer = p.clone();
    let input = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut stdin = tokio::io::stdin();
        let mut buf = vec![0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    let _ = writer.write_pty(vec![4]).await; // EOT
                    break;
                }
                Ok(n) => {
                    if writer.write_pty(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    #[cfg(unix)]
    let resize = {
        let fixed = cols.is_some() || rows.is_some();
        let p = p.clone();
        tokio::spawn(async move {
            if fixed {
                return;
            }
            let Ok(mut s) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())
            else {
                return;
            };
            while s.recv().await.is_some() {
                let (c, r) = terminal_size();
                if p.resize(c, r).await.is_err() {
                    break;
                }
            }
        })
    };
    let mut code = 0;
    let result = async {
        while let Some(ev) = p.next_event().await? {
            if let Some(exit) = ev.exit {
                code = exit.code.unwrap_or(1);
                break;
            }
            let mut o = std::io::stdout();
            let _ = o.write_all(&ev.data);
            let _ = o.flush();
        }
        Ok::<(), CuaError>(())
    }
    .await;
    input.abort();
    #[cfg(unix)]
    resize.abort();
    drop(raw);
    result?;
    Ok(code)
}
