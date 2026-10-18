// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `process`: `ProcessService` runs commands with env, reports exit codes,
//! carries stdin, sizes a PTY and delivers signals.

use std::time::Duration;

use bytes::Bytes;
use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, Command, ProcessEvent};

use crate::{Ctx, Recorder};

/// Most events read from one process stream.
const MAX_EVENTS: usize = 2_000;

fn shell(ctx: &Ctx, line: &str) -> Command {
    if ctx.os() == "windows" {
        Command::new("cmd").args(["/C", line])
    } else {
        Command::new("/bin/sh").args(["-c", line])
    }
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("process") {
        return;
    }
    let nonce = ctx.nonce.clone();
    rec.run("process.run", &["core"], Duration::from_secs(20), async {
        let line = if ctx.os() == "windows" {
            "echo %CUA_DOCTOR%"
        } else {
            "echo \"$CUA_DOCTOR\""
        };
        let cmd = shell(ctx, line)
            .env("CUA_DOCTOR", &nonce)
            .timeout(Duration::from_secs(15));
        match ctx.client.run(cmd).await {
            Ok(out) => {
                let got = out.stdout_str();
                Check::new(
                    "process.run",
                    super::verdict(out.success() && got.trim() == nonce),
                    format!("exit {:?}, stdout {:?}", out.status.code, got.trim()),
                )
            }
            Err(error) => Check::new(
                "process.run",
                Status::Fail,
                format!("StartProcess: {error}"),
            ),
        }
    })
    .await;

    rec.run(
        "process.exit_code",
        &["core"],
        Duration::from_secs(20),
        async {
            match ctx
                .client
                .run(shell(ctx, "exit 7").timeout(Duration::from_secs(15)))
                .await
            {
                Ok(out) => Check::new(
                    "process.exit_code",
                    super::verdict(out.status.code == Some(7)),
                    format!("`exit 7` reported {:?}", out.status.code),
                ),
                Err(error) => Check::new("process.exit_code", Status::Fail, error.to_string()),
            }
        },
    )
    .await;

    if ctx.os() != "windows" {
        rec.run("process.stdin", &["core"], Duration::from_secs(20), async {
            let cmd = Command::new("cat")
                .stdin(true)
                .timeout(Duration::from_secs(15));
            let handle = match ctx.client.spawn(cmd).await {
                Ok(h) => h,
                Err(error) => return Check::new("process.stdin", Status::Fail, error.to_string()),
            };
            if let Err(error) = handle
                .write_stdin(Bytes::from_static(b"doctor-stdin\n"))
                .await
            {
                return Check::new(
                    "process.stdin",
                    Status::Fail,
                    format!("stdin write: {error}"),
                );
            }
            if let Err(error) = handle.close_stdin().await {
                return Check::new(
                    "process.stdin",
                    Status::Fail,
                    format!("close stdin: {error}"),
                );
            }
            match handle.wait().await {
                Ok(out) => Check::new(
                    "process.stdin",
                    super::verdict(out.stdout_str() == "doctor-stdin\n" && out.success()),
                    format!("cat echoed {:?}", out.stdout_str()),
                ),
                Err(error) => Check::new("process.stdin", Status::Fail, error.to_string()),
            }
        })
        .await;

        rec.run(
            "process.pty",
            &["feature:pty"],
            Duration::from_secs(20),
            async {
                let cmd = Command::new("/bin/sh")
                    .args(["-c", "sleep 0.5; stty size"])
                    .pty(80, 24)
                    .timeout(Duration::from_secs(15));
                let mut handle = match ctx.client.spawn(cmd).await {
                    Ok(h) => h,
                    Err(error) => {
                        return Check::new("process.pty", Status::Fail, error.to_string())
                    }
                };
                if let Err(error) = handle.resize(132, 43).await {
                    return Check::new("process.pty", Status::Fail, format!("ResizePty: {error}"));
                }
                let mut text = Vec::new();
                for _ in 0..MAX_EVENTS {
                    match handle.next_event().await {
                        Ok(Some(ProcessEvent::Pty { data, .. })) => {
                            text.extend_from_slice(&data);
                            if text.len() > 64 * 1024 {
                                break;
                            }
                        }
                        Ok(Some(ProcessEvent::Exit(_))) | Ok(None) => break,
                        Ok(Some(_)) => {}
                        Err(error) => {
                            return Check::new("process.pty", Status::Fail, error.to_string())
                        }
                    }
                }
                let text = String::from_utf8_lossy(&text);
                Check::new(
                    "process.pty",
                    super::verdict(text.contains("43 132")),
                    format!("stty size after resize to 132x43: {:?}", text.trim()),
                )
            },
        )
        .await;

        rec.run(
            "process.signal",
            &["core"],
            Duration::from_secs(20),
            async {
                let cmd = Command::new("sleep")
                    .arg("30")
                    .timeout(Duration::from_secs(40));
                let handle = match ctx.client.spawn(cmd).await {
                    Ok(h) => h,
                    Err(error) => {
                        return Check::new("process.signal", Status::Fail, error.to_string())
                    }
                };
                if let Err(error) = handle.signal(pb::Signal::Term).await {
                    return Check::new(
                        "process.signal",
                        Status::Fail,
                        format!("SignalProcess: {error}"),
                    );
                }
                match tokio::time::timeout(Duration::from_secs(10), handle.wait()).await {
                    Ok(Ok(out)) => Check::new(
                        "process.signal",
                        super::verdict(out.status.signal == Some(pb::Signal::Term)),
                        format!(
                            "sleep ended with {:?} / exit {:?}",
                            out.status.signal, out.status.code
                        ),
                    ),
                    Ok(Err(error)) => Check::new("process.signal", Status::Fail, error.to_string()),
                    Err(_) => Check::new(
                        "process.signal",
                        Status::Fail,
                        "SIGTERM did not stop `sleep 30` within 10 s",
                    ),
                }
            },
        )
        .await;
    }
}
