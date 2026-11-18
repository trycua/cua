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
CUA_VNC_PASSWORD alone for a VNC password. Clicks and typing are logged to
$CUA_VNC_LOG, default ~/.cache/cua-driver/vnc-approvals.log.";

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
            client.click(x, y).map_err(|e| e.to_string())?;
            record(&format!("{server} click {x},{y} {label}"));
            println!("Clicked ({x}, {y}) on {server}: {label}");
        }
        Action::TypeEnv { name } => {
            let text = typed.expect("read above");
            client.type_line(&text).map_err(|e| e.to_string())?;
            record(&format!("{server} typed ${name} (value not logged)"));
            println!("Typed ${name} and Return on {server}.");
        }
    }
    Ok(())
}

/// A screenshot shows the whole screen, so only its owner may read it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Append one line to the approval log. A log that cannot be written is
/// reported but does not undo the action that already happened.
fn record(entry: &str) {
    let path = std::env::var_os("CUA_VNC_LOG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".cache/cua-driver/vnc-approvals.log"))
        });
    let Some(path) = path else {
        eprintln!("warning: no home directory, so this approval was not logged");
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let line = format!("{} {entry}\n", crate::version_check::iso8601(now));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            let mut options = std::fs::OpenOptions::new();
            options.append(true).create(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(&path)?.write_all(line.as_bytes())
        });
    if let Err(error) = written {
        eprintln!(
            "warning: cannot write the approval log {}: {error}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
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
