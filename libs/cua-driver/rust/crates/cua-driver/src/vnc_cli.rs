// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver permissions vnc shot|click|type-env`: approve Cua Driver's
//! macOS permission prompts through the Mac's Screen Sharing (VNC) server,
//! for a Mac nobody is sitting at. See "Operator-authorized hosts" in the
//! skill's MACOS.md for when this may be used.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zeroize::Zeroizing;

use crate::vnc::{Credentials, RfbClient};

const TIMEOUT: Duration = Duration::from_secs(30);
const USAGE: &str = "usage:
  cua-driver permissions vnc shot <out.png> [--server <host[:port]>]
  cua-driver permissions vnc click <x> <y> --label <what this approves> [--server <host[:port]>]
  cua-driver permissions vnc type-env <NAME> [--server <host[:port]>]

The server defaults to $CUA_VNC_SERVER, port 5900. Credentials come only from
the environment: CUA_VNC_USER and CUA_VNC_PASSWORD (a macOS account), or
CUA_VNC_PASSWORD alone for a VNC password. Each click and typed variable is
logged to $CUA_VNC_LOG, default ~/.cache/cua-driver/vnc-approvals.log, before
it is sent; an unwritable log stops it.

VNC encrypts only the Apple account credentials, not the session: screenshots
and typed text cross the network in the clear. Reach a remote Mac through an
SSH tunnel or a network you trust.";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Shot { out: PathBuf },
    Click { x: u16, y: u16, label: String },
    TypeEnv { name: String },
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    action: Action,
    server: Option<String>,
}

/// `args` are the words after `permissions vnc`.
pub fn run(args: &[String]) {
    let invocation = match parse(args) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("cua-driver permissions vnc: {message}\n\n{USAGE}");
            process::exit(64);
        }
    };
    if let Err(message) = execute(invocation) {
        eprintln!("cua-driver permissions vnc: {message}");
        process::exit(1);
    }
}

fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut positionals = Vec::new();
    let mut server = None;
    let mut label = None;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (word.as_str(), None),
        };
        let mut value = |option: &str| {
            inline
                .clone()
                .or_else(|| words.next().cloned())
                .ok_or_else(|| format!("{option} needs a value"))
        };
        match name {
            "--server" => server = Some(value("--server")?),
            "--label" => label = Some(value("--label")?),
            option if option.starts_with("--") => return Err(format!("unknown option '{option}'")),
            _ => positionals.push(word.as_str()),
        }
    }
    let coordinate = |text: &str| {
        text.parse::<u16>()
            .map_err(|_| format!("'{text}' is not a pixel coordinate"))
    };
    if label.is_some() && positionals.first() != Some(&"click") {
        return Err("--label applies to click only".into());
    }
    let action = match positionals.as_slice() {
        ["shot", out] => Action::Shot {
            out: PathBuf::from(out),
        },
        ["click", x, y] => {
            let label = label
                .filter(|label| !label.trim().is_empty())
                .ok_or("click needs --label naming the prompt it approves")?;
            Action::Click {
                x: coordinate(x)?,
                y: coordinate(y)?,
                label,
            }
        }
        ["type-env", name] => Action::TypeEnv {
            name: (*name).to_owned(),
        },
        [] => return Err("missing subcommand".into()),
        [other, ..] if !["shot", "click", "type-env"].contains(other) => {
            return Err(format!("unknown subcommand '{other}'"))
        }
        _ => return Err("wrong number of arguments".into()),
    };
    Ok(Invocation { action, server })
}

fn execute(invocation: Invocation) -> Result<(), String> {
    let server = invocation
        .server
        .or_else(|| std::env::var("CUA_VNC_SERVER").ok())
        .filter(|server| !server.trim().is_empty())
        .ok_or("set CUA_VNC_SERVER or pass --server <host[:port]>")?;
    let secret = |name: &str| std::env::var(name).ok().map(Zeroizing::new);
    let credentials = Credentials {
        username: secret("CUA_VNC_USER"),
        password: secret("CUA_VNC_PASSWORD"),
    };
    // Read the secret before connecting, so a missing variable fails fast.
    let typed = match &invocation.action {
        Action::TypeEnv { name } => {
            Some(secret(name).ok_or_else(|| format!("environment variable {name} is not set"))?)
        }
        _ => None,
    };
    // Open the approval log before connecting, so an unwritable log fails
    // before any input is sent.
    let mut log = match invocation.action {
        Action::Shot { .. } => None,
        _ => Some(ApprovalLog::open()?),
    };
    let mut client =
        RfbClient::connect(&server, &credentials, TIMEOUT).map_err(|e| e.to_string())?;
    match invocation.action {
        Action::Shot { out } => {
            let frame = client.screenshot().map_err(|e| e.to_string())?;
            let png = frame.to_png().map_err(|e| e.to_string())?;
            write_private(&out, &png)
                .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
            println!(
                "Saved the {}x{} screen of {server} to {}. Click coordinates are pixels in this image.",
                frame.width,
                frame.height,
                out.display()
            );
        }
        Action::Click { x, y, label } => {
            let log = log.as_mut().expect("opened above");
            log.record(&format!("{server} click {x},{y} {label}"))?;
            client
                .click(x, y)
                .map_err(|e| log.failed(&server, e.to_string()))?;
            println!("Clicked ({x}, {y}) on {server}: {label}");
        }
        Action::TypeEnv { name } => {
            let text = typed.expect("read above");
            let log = log.as_mut().expect("opened above");
            log.record(&format!("{server} type ${name} (value not logged)"))?;
            client
                .type_line(&text)
                .map_err(|e| log.failed(&server, e.to_string()))?;
            println!("Typed ${name} and Return on {server}.");
        }
    }
    Ok(())
}

/// A screenshot shows the whole screen, so only its owner may read it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    open_private(&mut options, path)?.write_all(bytes)
}

/// Open with `options` as an owner-only file. The mode applies only when the
/// file is created, so an existing file is also set to owner-only before
/// anything is written to it.
fn open_private(options: &mut std::fs::OpenOptions, path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(options, 0o600);
    let file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    #[cfg(windows)]
    crate::extension_manager::windows_secure_handle(&file)
        .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
    Ok(file)
}

/// The append-only log of every click and typed variable, written before
/// the input is sent.
struct ApprovalLog {
    path: PathBuf,
    file: std::fs::File,
}

impl ApprovalLog {
    fn open() -> Result<Self, String> {
        let path = std::env::var_os("CUA_VNC_LOG")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|home| PathBuf::from(home).join(".cache/cua-driver/vnc-approvals.log"))
            })
            .ok_or("no home directory for the approval log; set CUA_VNC_LOG")?;
        Self::at(path)
    }

    fn at(path: PathBuf) -> Result<Self, String> {
        let file = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let mut options = std::fs::OpenOptions::new();
                options.append(true).create(true);
                open_private(&mut options, &path)
            })
            .map_err(|error| format!("cannot open the approval log {}: {error}", path.display()))?;
        Ok(Self { path, file })
    }

    fn record(&mut self, entry: &str) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let line = format!("{} {entry}\n", crate::version_check::iso8601(now));
        self.file.write_all(line.as_bytes()).map_err(|error| {
            format!(
                "cannot write the approval log {}: {error}",
                self.path.display()
            )
        })
    }

    /// Note that the input just logged did not go through, and return the
    /// error to report.
    fn failed(&mut self, server: &str, error: String) -> String {
        if let Err(log_error) = self.record(&format!("{server} failed: {error}")) {
            eprintln!("warning: {log_error}");
        }
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[cfg(unix)]
    #[test]
    fn an_overwritten_screenshot_becomes_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("cua-vnc-shot-{}.png", std::process::id()));
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let content = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((mode, content.as_slice()), (0o600, &b"new"[..]));
    }

    #[test]
    fn approval_log_refuses_an_unwritable_path_and_appends_privately() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"").unwrap();
        let unwritable = blocker.join("vnc-approvals.log");
        let error = ApprovalLog::at(unwritable.clone()).err().unwrap();
        assert!(
            error.starts_with(&format!(
                "cannot open the approval log {}: ",
                unwritable.display()
            )),
            "{error}"
        );

        let path = dir.path().join("logs/vnc-approvals.log");
        let mut log = ApprovalLog::at(path.clone()).unwrap();
        log.record("mac click 1,2 CuaDriver: Accessibility")
            .unwrap();
        assert_eq!(log.failed("mac", "refused".into()), "refused");
        let lines = std::fs::read_to_string(&path).unwrap();
        let entries: Vec<_> = lines
            .lines()
            .map(|line| line.split_once(' ').unwrap().1)
            .collect();
        assert_eq!(
            entries,
            [
                "mac click 1,2 CuaDriver: Accessibility",
                "mac failed: refused"
            ]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn parses_each_action() {
        assert_eq!(
            parse(&args(&["shot", "s.png", "--server", "mac:5901"])).unwrap(),
            Invocation {
                action: Action::Shot {
                    out: "s.png".into()
                },
                server: Some("mac:5901".into())
            }
        );
        assert_eq!(
            parse(&args(&[
                "click",
                "10",
                "20",
                "--label=CuaDriver: Accessibility"
            ]))
            .unwrap(),
            Invocation {
                action: Action::Click {
                    x: 10,
                    y: 20,
                    label: "CuaDriver: Accessibility".into()
                },
                server: None
            }
        );
        assert_eq!(
            parse(&args(&["type-env", "VM_PASSWORD"])).unwrap().action,
            Action::TypeEnv {
                name: "VM_PASSWORD".into()
            }
        );
    }

    #[test]
    fn refuses_unlabelled_clicks_and_bad_input() {
        assert_eq!(
            parse(&args(&["click", "1", "2"])).unwrap_err(),
            "click needs --label naming the prompt it approves"
        );
        assert_eq!(
            parse(&args(&["click", "1", "-2", "--label", "x"])).unwrap_err(),
            "'-2' is not a pixel coordinate"
        );
        assert_eq!(
            parse(&args(&["shot", "s.png", "--label", "x"])).unwrap_err(),
            "--label applies to click only"
        );
        assert_eq!(
            parse(&args(&["paste", "x"])).unwrap_err(),
            "unknown subcommand 'paste'"
        );
        assert_eq!(
            parse(&args(&["shot"])).unwrap_err(),
            "wrong number of arguments"
        );
        assert_eq!(
            parse(&args(&["shot", "s.png", "--password", "x"])).unwrap_err(),
            "unknown option '--password'"
        );
    }
}
