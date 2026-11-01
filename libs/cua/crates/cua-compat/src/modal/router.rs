//! `modal.task_command_router.TaskCommandRouter`: exec, stdio, stdin and
//! the task-level operations, on the same listener as the control plane.
//! Each call carries the bearer JWT `TaskGetCommandRouterAccess` minted;
//! the auth layer verified it and attached its task.

use crate::modal::exec::{self, Exit, STDERR, STDOUT, Session, StdinErr};
use crate::modal::pb::api as ap;
use crate::modal::pb::tcr::{self as pb, task_command_router_server::TaskCommandRouter};
use crate::modal::sandbox::{ExecReq, SandboxRec, SbState};
use crate::modal::{Modal, errors, fstools};
use cua_spacesd_client::pb as env_pb;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tonic::{Request, Response, Status};

/// The task a verified command-router token is for.
#[derive(Clone, Debug)]
pub struct TaskClaim(pub String);

pub struct Router_(pub Arc<Modal>);

type Resp<T> = Result<Response<T>, Status>;

fn claim<T>(r: &Request<T>) -> String {
    r.extensions()
        .get::<TaskClaim>()
        .map(|c| c.0.clone())
        .unwrap_or_default()
}

impl Router_ {
    /// The running sandbox of the token's task, checking the request names
    /// the same task.
    fn task(&self, token_task: &str, req_task: &str) -> Result<SandboxRec, Status> {
        if token_task != req_task {
            return Err(errors::denied(format!(
                "the command router token is not for task {req_task}"
            )));
        }
        let rec = self
            .0
            .db
            .read(|d| {
                d.sandboxes
                    .values()
                    .find(|s| s.task_id == req_task)
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Task {req_task} not found")))?;
        if rec.state == SbState::Finished {
            return Err(errors::not_found(format!(
                "Task {req_task} has already finished"
            )));
        }
        Ok(rec)
    }

    /// Like [`Router_::task`] but also for a finished sandbox: its execs
    /// keep their output and exit status for a while.
    fn task_any(&self, token_task: &str, req_task: &str) -> Result<SandboxRec, Status> {
        if token_task != req_task {
            return Err(errors::denied(format!(
                "the command router token is not for task {req_task}"
            )));
        }
        self.0
            .db
            .read(|d| {
                d.sandboxes
                    .values()
                    .find(|s| s.task_id == req_task)
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Task {req_task} not found")))
    }

    async fn session(&self, task: &str, exec_id: &str) -> Result<Arc<Session>, Status> {
        self.0
            .live
            .execs
            .lock()
            .await
            .get(&(task.to_string(), exec_id.to_string()))
            .cloned()
            .ok_or_else(|| errors::not_found(format!("exec {exec_id} not found")))
    }
}

fn exit_code(e: &Exit) -> (Option<i32>, Option<i32>) {
    e.status()
}

fn stdin_status(e: StdinErr) -> Status {
    match e {
        StdinErr::Gap { expected } => errors::conflict(format!(
            "stdin write offset does not follow the bytes written (expected offset {expected})"
        )),
        StdinErr::Closed => errors::conflict("stdin is closed"),
        StdinErr::Gone => errors::conflict("the process has exited; its stdin is closed"),
        StdinErr::Rpc(s) => {
            errors::conflict(format!("could not write to the process: {}", s.message()))
        }
    }
}

#[tonic::async_trait]
impl TaskCommandRouter for Router_ {
    async fn task_exec_start(
        &self,
        r: Request<pb::TaskExecStartRequest>,
    ) -> Resp<pb::TaskExecStartResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let m = &self.0;
        if q.command_args.is_empty() {
            return Err(errors::invalid("an exec needs a command"));
        }
        if !q.container_id.is_empty() {
            return Err(errors::unsupported(
                "exec in an additional container",
                "Cua Spaces have one container per sandbox (sidecars are not available)",
            ));
        }
        let key = (q.task_id.clone(), q.exec_id.clone());
        // A retried start (same exec id) starts nothing new.
        if m.live.execs.lock().await.contains_key(&key) {
            return Ok(Response::new(pb::TaskExecStartResponse {}));
        }
        m.touch(&rec.id).await;
        let cfg = exec::Config {
            discard_stdout: q.stdout_config == pb::TaskExecStdoutConfig::Devnull as i32,
            discard_stderr: q.stderr_config == pb::TaskExecStderrConfig::Devnull as i32,
            merge_stderr: q.stderr_config == pb::TaskExecStderrConfig::Stdout as i32,
        };
        let session = if fstools::is_tool(&q.command_args) {
            let json = q.command_args.get(1).cloned().unwrap_or_default();
            let (sink, rx) = fstools::channel();
            let s = exec::internal(Some(sink), &cfg);
            let client = m.spacesd(&rec).await.map_err(errors::from_backend)?;
            let (driver, linux) = m.guest_of(&rec, &client).await;
            let target = fstools::Target {
                client,
                user: if linux { "root".into() } else { driver.clone() },
                driver,
            };
            let s2 = s.clone();
            tokio::spawn(async move { fstools::run(target, json, s2, Some(rx)).await });
            s
        } else {
            let mut env: BTreeMap<String, String> = q.env.clone().into_iter().collect();
            let from_sources = m.resolve_secret_sources(&rec, &q.secret_sources)?;
            env.extend(from_sources);
            env.extend(q.env.clone());
            let pty = q.pty_info.as_ref().filter(|p| p.enabled).map(|p| {
                if !p.env_term.is_empty() {
                    env.insert("TERM".into(), p.env_term.clone());
                }
                if !p.env_colorterm.is_empty() {
                    env.insert("COLORTERM".into(), p.env_colorterm.clone());
                }
                (p.winsz_cols.max(1), p.winsz_rows.max(1))
            });
            let req = ExecReq {
                argv: q.command_args.clone(),
                env,
                cwd: q.workdir.clone().filter(|w| !w.is_empty()),
                user: None,
                timeout: q
                    .timeout_secs
                    .filter(|t| *t > 0)
                    .map(|t| Duration::from_secs(u64::from(t))),
                pty,
                stdin: true,
                tag: None,
            };
            m.launch(&rec, req, cfg).await?
        };
        m.live.execs.lock().await.insert(key, session);
        Ok(Response::new(pb::TaskExecStartResponse {}))
    }

    async fn task_exec_wait(
        &self,
        r: Request<pb::TaskExecWaitRequest>,
    ) -> Resp<pb::TaskExecWaitResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        self.task_any(&token_task, &q.task_id)?;
        let s = self.session(&q.task_id, &q.exec_id).await?;
        let exit = s.wait().await;
        let (code, signal) = exit_code(&exit);
        Ok(Response::new(pb::TaskExecWaitResponse {
            exit_status: match (code, signal) {
                (Some(c), _) => Some(pb::task_exec_wait_response::ExitStatus::Code(c)),
                (_, Some(s)) => Some(pb::task_exec_wait_response::ExitStatus::Signal(s)),
                _ => Some(pb::task_exec_wait_response::ExitStatus::Code(127)),
            },
        }))
    }

    async fn task_exec_poll(
        &self,
        r: Request<pb::TaskExecPollRequest>,
    ) -> Resp<pb::TaskExecPollResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        self.task_any(&token_task, &q.task_id)?;
        let s = self.session(&q.task_id, &q.exec_id).await?;
        let exit_status = s.exit().map(|e| match exit_code(&e) {
            (Some(c), _) => pb::task_exec_poll_response::ExitStatus::Code(c),
            (_, Some(g)) => pb::task_exec_poll_response::ExitStatus::Signal(g),
            _ => pb::task_exec_poll_response::ExitStatus::Code(127),
        });
        Ok(Response::new(pb::TaskExecPollResponse { exit_status }))
    }

    async fn task_exec_stdio_read(
        &self,
        r: Request<pb::TaskExecStdioReadRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::TaskExecStdioReadResponse>> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task_any(&token_task, &q.task_id)?;
        let s = self.session(&q.task_id, &q.exec_id).await?;
        let fd = if q.file_descriptor == pb::TaskExecStdioFileDescriptor::Stderr as i32 {
            STDERR
        } else {
            STDOUT
        };
        let m = self.0.clone();
        let stream = futures_util::stream::unfold(
            (s, q.offset, m, rec.id),
            move |(s, off, m, sb)| async move {
                match s.read(fd, off).await {
                    Ok(Some(chunk)) => {
                        m.touch(&sb).await;
                        let n = chunk.len() as u64;
                        Some((
                            Ok(pb::TaskExecStdioReadResponse { data: chunk }),
                            (s, off + n, m, sb),
                        ))
                    }
                    Ok(None) => None,
                    Err(exec::ReadErr::Evicted { start }) => Some((
                        Err(errors::invalid(format!(
                            "output before offset {start} is no longer retained"
                        ))),
                        (s, u64::MAX, m, sb),
                    )),
                }
            },
        );
        // Stop after an error item.
        let stream = futures_util::StreamExt::take_while(stream, {
            let mut failed = false;
            move |item| {
                let go = !failed;
                if item.is_err() {
                    failed = true;
                }
                futures_util::future::ready(go)
            }
        });
        Ok(Response::new(Box::pin(stream)))
    }

    async fn task_exec_stdin_write(
        &self,
        r: Request<pb::TaskExecStdinWriteRequest>,
    ) -> Resp<pb::TaskExecStdinWriteResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let s = self.session(&q.task_id, &q.exec_id).await?;
        if s.exit().is_some() {
            return Err(errors::conflict(
                "the process has exited; its stdin is closed",
            ));
        }
        self.0.touch(&rec.id).await;
        s.write_stdin(q.offset, &q.data, q.eof)
            .await
            .map_err(stdin_status)?;
        Ok(Response::new(pb::TaskExecStdinWriteResponse {}))
    }

    async fn task_exec_stdin_status(
        &self,
        r: Request<pb::TaskExecStdinStatusRequest>,
    ) -> Resp<pb::TaskExecStdinStatusResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        self.task_any(&token_task, &q.task_id)?;
        let s = self.session(&q.task_id, &q.exec_id).await?;
        let (written, closed) = s.stdin_status().await;
        Ok(Response::new(pb::TaskExecStdinStatusResponse {
            num_bytes_written: written,
            closed,
        }))
    }

    async fn task_exec_stdin_write_stream(
        &self,
        r: Request<tonic::Streaming<pb::TaskExecStdinWriteStreamRequest>>,
    ) -> Resp<pb::TaskExecStdinWriteStreamResponse> {
        let token_task = claim(&r);
        let mut st = r.into_inner();
        use pb::task_exec_stdin_write_stream_request::Payload;
        let first = st
            .message()
            .await?
            .and_then(|m| m.payload)
            .ok_or_else(|| errors::invalid("the stream must start with a start message"))?;
        let Payload::Start(start) = first else {
            return Err(errors::invalid(
                "the stream must start with a start message",
            ));
        };
        let rec = self.task(&token_task, &start.task_id)?;
        let s = self.session(&start.task_id, &start.exec_id).await?;
        let mut offset = start.offset;
        loop {
            let Some(msg) = st.message().await? else {
                // The client went away without End: stdin stays open and
                // the client may resume from `TaskExecStdinStatus`.
                return Err(Status::cancelled(
                    "the stdin stream ended without an end message",
                ));
            };
            match msg.payload {
                Some(Payload::Data(d)) => {
                    self.0.touch(&rec.id).await;
                    s.write_stdin(offset, &d, false)
                        .await
                        .map_err(stdin_status)?;
                    offset += d.len() as u64;
                }
                Some(Payload::End(_)) => {
                    s.write_stdin(offset, &[], true)
                        .await
                        .map_err(stdin_status)?;
                    return Ok(Response::new(pb::TaskExecStdinWriteStreamResponse {}));
                }
                Some(Payload::Start(_)) => return Err(errors::invalid("duplicate start message")),
                None => {}
            }
        }
    }

    async fn sandbox_wait_until_ready(
        &self,
        r: Request<pb::SandboxWaitUntilReadyTcrRequest>,
    ) -> Resp<pb::SandboxWaitUntilReadyTcrResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let def = rec.def();
        let Some(probe) = def.readiness_probe.clone() else {
            return Err(errors::invalid(
                "The Sandbox was not created with a readiness_probe",
            ));
        };
        let deadline = tokio::time::Instant::now() + crate::modal::api::secs(q.timeout);
        let interval = Duration::from_millis(u64::from(probe.interval_ms.unwrap_or(100).max(10)));
        loop {
            if self.0.probe_ready(&rec, &probe).await {
                return Ok(Response::new(pb::SandboxWaitUntilReadyTcrResponse {
                    ready_at: crate::modal::db::now_s(),
                }));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Status::deadline_exceeded(
                    "the Sandbox did not become ready in time",
                ));
            }
            if self
                .0
                .sandbox(&rec.id)
                .is_none_or(|s| s.state == SbState::Finished)
            {
                return Err(errors::conflict(
                    "the Sandbox finished before it became ready",
                ));
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn sandbox_stdio_read_v2(
        &self,
        r: Request<pb::SandboxStdioReadV2Request>,
    ) -> Resp<tonic::codegen::BoxStream<pb::SandboxStdioReadV2Response>> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task_any(&token_task, &q.task_id)?;
        let fd = if q.file_descriptor == pb::SandboxStdioFileDescriptor::Stderr as i32 {
            STDERR
        } else {
            STDOUT
        };
        let session = self.0.live.entrypoints.lock().await.get(&rec.id).cloned();
        let m = self.0.clone();
        let Some(s) = session else {
            // No entrypoint: no output; the stream ends with the sandbox.
            let id = rec.id.clone();
            let stream = futures_util::stream::once(async move {
                m.wait_finished(&id, Duration::from_secs(55)).await;
                None::<Result<pb::SandboxStdioReadV2Response, Status>>
            });
            let stream = futures_util::StreamExt::filter_map(stream, futures_util::future::ready);
            return Ok(Response::new(Box::pin(stream)));
        };
        let stream = futures_util::stream::unfold(
            (s, q.offset, m, rec.id, true),
            move |(s, off, m, sb, first)| async move {
                match s.read(fd, off).await {
                    Ok(Some(chunk)) => {
                        m.touch(&sb).await;
                        let n = chunk.len() as u64;
                        Some((
                            Ok(pb::SandboxStdioReadV2Response {
                                data: chunk,
                                starting_offset: off,
                            }),
                            (s, off + n, m, sb, false),
                        ))
                    }
                    Ok(None) => None,
                    Err(exec::ReadErr::Evicted { start }) => {
                        // Output the sandbox no longer retains: continue at the
                        // oldest byte; `starting_offset` tells the client.
                        let _ = first;
                        match s.read(fd, start).await {
                            Ok(Some(chunk)) => {
                                let n = chunk.len() as u64;
                                Some((
                                    Ok(pb::SandboxStdioReadV2Response {
                                        data: chunk,
                                        starting_offset: start,
                                    }),
                                    (s, start + n, m, sb, false),
                                ))
                            }
                            _ => None,
                        }
                    }
                }
            },
        );
        Ok(Response::new(Box::pin(stream)))
    }

    async fn sandbox_stdin_write_v2(
        &self,
        r: Request<pb::SandboxStdinWriteV2Request>,
    ) -> Resp<pb::SandboxStdinWriteV2Response> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let entry = self.0.live.entrypoints.lock().await.get(&rec.id).cloned();
        let Some(s) = entry else {
            return Err(errors::conflict(
                "the Sandbox has no running entrypoint to write to",
            ));
        };
        self.0.touch(&rec.id).await;
        s.write_stdin(q.offset, &q.data, q.eof)
            .await
            .map_err(stdin_status)?;
        Ok(Response::new(pb::SandboxStdinWriteV2Response {}))
    }

    async fn task_snapshot_memory(
        &self,
        r: Request<pb::TaskSnapshotMemoryRequest>,
    ) -> Resp<pb::TaskSnapshotMemoryResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let snapshot_id = self.0.snapshot_memory(&rec, &q.idempotency_key).await?;
        Ok(Response::new(pb::TaskSnapshotMemoryResponse {
            snapshot_id,
        }))
    }

    async fn task_reload_volumes(
        &self,
        r: Request<pb::TaskReloadVolumesRequest>,
    ) -> Resp<pb::TaskReloadVolumesResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        self.0.reload_volumes(&rec).await?;
        Ok(Response::new(pb::TaskReloadVolumesResponse {}))
    }

    async fn task_set_network_access(
        &self,
        r: Request<pb::TaskSetNetworkAccessRequest>,
    ) -> Resp<pb::TaskSetNetworkAccessResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let want = q.network_access.map(|n| n.network_access_type).unwrap_or(0);
        let blocked_now = rec.def().network_access.is_some_and(|n| {
            n.network_access_type == ap::network_access::NetworkAccessType::Blocked as i32
        });
        if want == ap::network_access::NetworkAccessType::Open as i32 && !blocked_now {
            return Ok(Response::new(pb::TaskSetNetworkAccessResponse {}));
        }
        Err(errors::unsupported(
            "changing a running Sandbox's outbound network policy",
            "Cua Spaces fix egress when they start (block_network) and have no host-enforced allowlists",
        ))
    }

    async fn task_set_outbound_policy(
        &self,
        _r: Request<pb::TaskSetExperimentalOutboundPolicyRequest>,
    ) -> Resp<pb::TaskSetExperimentalOutboundPolicyResponse> {
        Err(errors::unsupported(
            "outbound policies",
            "header replacement for outbound HTTPS needs a host-side proxy Cua Spaces do not have",
        ))
    }

    async fn task_snapshot_filesystem(
        &self,
        r: Request<pb::TaskSnapshotFilesystemRequest>,
    ) -> Resp<pb::TaskSnapshotFilesystemResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        if !q.container_id.is_empty() {
            return Err(errors::unsupported(
                "snapshotting another container",
                "Cua Spaces have one container per sandbox",
            ));
        }
        let image_id = self.0.snapshot_filesystem(&rec).await?;
        Ok(Response::new(pb::TaskSnapshotFilesystemResponse {
            image_id,
        }))
    }

    async fn task_snapshot_directory(
        &self,
        r: Request<pb::TaskSnapshotDirectoryRequest>,
    ) -> Resp<pb::TaskSnapshotDirectoryResponse> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let path = String::from_utf8_lossy(&q.path).to_string();
        let image_id = self.0.snapshot_directory(&rec, &path).await?;
        Ok(Response::new(pb::TaskSnapshotDirectoryResponse {
            image_id,
        }))
    }

    async fn task_mount_directory(&self, r: Request<pb::TaskMountDirectoryRequest>) -> Resp<()> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let path = String::from_utf8_lossy(&q.path).to_string();
        self.0.mount_image(&rec, &path, &q.image_id).await?;
        Ok(Response::new(()))
    }

    async fn task_unmount_directory(
        &self,
        r: Request<pb::TaskUnmountDirectoryRequest>,
    ) -> Resp<()> {
        let token_task = claim(&r);
        let q = r.into_inner();
        let rec = self.task(&token_task, &q.task_id)?;
        let path = String::from_utf8_lossy(&q.path).to_string();
        self.0.unmount_image(&rec, &path).await?;
        Ok(Response::new(()))
    }
}

impl Modal {
    /// `SecretSource`s as environment variables.
    pub fn resolve_secret_sources(
        &self,
        rec: &SandboxRec,
        sources: &[ap::SecretSource],
    ) -> Result<BTreeMap<String, String>, Status> {
        let mut env = BTreeMap::new();
        for s in sources {
            match &s.source {
                Some(ap::secret_source::Source::SecretId(id)) => {
                    let sec = self
                        .db
                        .read(|d| {
                            d.secrets
                                .get(id)
                                .filter(|x| x.key_id == rec.key_id)
                                .cloned()
                        })
                        .ok_or_else(|| errors::not_found(format!("Secret {id} not found")))?;
                    env.extend(sec.env);
                }
                Some(ap::secret_source::Source::Env(m)) => {
                    env.extend(m.contents.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                None => {}
            }
        }
        Ok(env)
    }

    /// Whether a readiness probe passes now.
    pub async fn probe_ready(&self, rec: &SandboxRec, probe: &ap::Probe) -> bool {
        match &probe.probe_oneof {
            Some(ap::probe::ProbeOneof::TcpPort(port)) => {
                let Some(name) = rec.cua_name.as_deref() else {
                    return false;
                };
                match self
                    .backend
                    .forward(&rec.identity(), name, *port as u16)
                    .await
                {
                    // The forward accepts at once whether or not the guest
                    // port listens: the guest dials on connect, and a closed
                    // port closes the connection. So the port is open when
                    // the connection stays up for a moment (or sends first).
                    Ok(addr) => {
                        use tokio::io::AsyncReadExt;
                        let Ok(Ok(mut s)) = tokio::time::timeout(
                            Duration::from_secs(2),
                            tokio::net::TcpStream::connect(addr),
                        )
                        .await
                        else {
                            return false;
                        };
                        let mut b = [0u8; 1];
                        match tokio::time::timeout(Duration::from_millis(400), s.read(&mut b)).await
                        {
                            Ok(Ok(0)) | Ok(Err(_)) => false,
                            Ok(Ok(_)) | Err(_) => true,
                        }
                    }
                    Err(_) => false,
                }
            }
            Some(ap::probe::ProbeOneof::ExecCommand(e)) if !e.argv.is_empty() => {
                let Ok(sess) = self
                    .launch(
                        rec,
                        ExecReq {
                            argv: e.argv.clone(),
                            env: Default::default(),
                            cwd: None,
                            user: None,
                            timeout: Some(Duration::from_secs(30)),
                            pty: None,
                            stdin: false,
                            tag: None,
                        },
                        exec::Config {
                            discard_stdout: true,
                            discard_stderr: true,
                            merge_stderr: false,
                        },
                    )
                    .await
                else {
                    return false;
                };
                sess.wait().await.code == Some(0)
            }
            _ => false,
        }
    }
}

#[allow(dead_code)]
fn _types(_: env_pb::Signal) {}
