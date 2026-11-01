//! A long-lived process in the guest that the gateway talks to over its
//! stdin and stdout (language servers, the code interpreter's workers).
//!
//! The process runs as the sandbox user through cua-spacesd; a task reads
//! its events into a channel so writes and reads never wait on each other.

use cua_spacesd_client::{ProcessEvent, SpacesdClient, pb};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

/// What the process produced.
#[derive(Debug)]
pub enum Out {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// The process ended (exit code; -1 when killed by a signal).
    Exit(#[allow(dead_code)] i32),
}

pub struct Stdio {
    c: SpacesdClient,
    pub pid: u32,
    pub rx: Mutex<mpsc::Receiver<Out>>,
}

impl Stdio {
    /// Starts `argv` as `user` in `cwd`.
    pub async fn start(
        c: &SpacesdClient,
        driver: &str,
        user: &str,
        argv: Vec<String>,
        env: std::collections::HashMap<String, String>,
        cwd: String,
    ) -> Result<Stdio, String> {
        let mut it = argv.into_iter();
        let (command, args, u, cwd) = crate::guest::as_user_in(
            driver,
            user,
            it.next().ok_or("empty command")?,
            it.collect(),
            cwd,
        );
        let mut cmd = cua_spacesd_client::Command::new(command)
            .args(args)
            .stdin(true);
        for (k, v) in env {
            cmd = cmd.env(k, v);
        }
        if !u.is_empty() {
            cmd = cmd.user(u);
        }
        if !cwd.is_empty() {
            cmd = cmd.cwd(cwd);
        }
        cmd.kill_on_disconnect = false;
        let mut h = c.spawn(cmd).await.map_err(|e| e.to_string())?;
        let pid = h.pid();
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            loop {
                match h.next_event().await {
                    Ok(Some(ProcessEvent::Stdout { data, .. })) => {
                        if tx.send(Out::Stdout(data.to_vec())).await.is_err() {
                            return;
                        }
                    }
                    Ok(Some(ProcessEvent::Stderr { data, .. })) => {
                        if tx.send(Out::Stderr(data.to_vec())).await.is_err() {
                            return;
                        }
                    }
                    Ok(Some(ProcessEvent::Exit(st))) => {
                        let _ = tx.send(Out::Exit(st.code.unwrap_or(-1))).await;
                        return;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => {
                        let _ = tx.send(Out::Exit(-1)).await;
                        return;
                    }
                }
            }
        });
        Ok(Stdio {
            c: c.clone(),
            pid,
            rx: Mutex::new(rx),
        })
    }

    fn selector(&self) -> pb::ProcessSelector {
        pb::ProcessSelector {
            selector: Some(pb::process_selector::Selector::Pid(self.pid)),
        }
    }

    pub async fn write(&self, data: Vec<u8>) -> Result<(), String> {
        self.c
            .process()
            .send_input(pb::SendInputRequest {
                process: Some(self.selector()),
                input: Some(pb::ProcessInput {
                    input: Some(pb::process_input::Input::Stdin(data)),
                }),
                sequence: 0,
                writer_id: String::new(),
            })
            .await
            .map(|_| ())
            .map_err(|e| e.message().to_string())
    }

    /// Sends `signal` to the process group.
    pub async fn signal(&self, signal: pb::Signal) {
        let _ = self
            .c
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(self.selector()),
                signal: signal as i32,
                process_group: true,
            })
            .await;
    }

    pub async fn kill(&self) {
        self.signal(pb::Signal::Kill).await;
    }

    /// Next output, or `None` after `t`.
    pub async fn next(&self, t: Duration) -> Option<Out> {
        let mut rx = self.rx.lock().await;
        tokio::time::timeout(t, rx.recv()).await.ok().flatten()
    }
}
