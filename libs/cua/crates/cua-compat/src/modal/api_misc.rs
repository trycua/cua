//! Environments, app listings and logs.

use crate::modal::api::Api;
use crate::modal::db::{AppRec, now_s};
use crate::modal::pb::api as pb;
use crate::modal::{Caller, errors};
use tonic::{Request, Response, Status};

type Resp<T> = Result<Response<T>, Status>;

fn caller<T>(r: &Request<T>) -> Result<Caller, Status> {
    r.extensions()
        .get::<Caller>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated("Token missing. Could not authenticate client."))
}

fn ts_ns(t: &Option<prost_types::Timestamp>, default: u64) -> u64 {
    t.as_ref()
        .map(|t| (t.seconds.max(0) as u64) * 1_000_000_000 + t.nanos.max(0) as u64)
        .unwrap_or(default)
}

fn ts(ns: u64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: (ns / 1_000_000_000) as i64,
        nanos: (ns % 1_000_000_000) as i32,
    }
}

impl Api {
    pub(super) async fn h_app_list(
        &self,
        r: Request<pb::AppListRequest>,
    ) -> Resp<pb::AppListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let env = if q.environment_name.is_empty() {
            "main".to_string()
        } else {
            q.environment_name
        };
        let mut apps: Vec<AppRec> = self.0.db.read(|d| {
            d.apps
                .values()
                .filter(|a| a.key_id == c.key_id() && a.environment == env && !a.stopped)
                .cloned()
                .collect()
        });
        apps.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        let running = |id: &str| {
            self.0.db.read(|d| {
                d.sandboxes
                    .values()
                    .filter(|s| s.app_id == id && s.running())
                    .count()
            }) as i32
        };
        Ok(Response::new(pb::AppListResponse {
            apps: apps
                .iter()
                .map(|a| pb::app_list_response::AppListItem {
                    app_id: a.id.clone(),
                    description: a.description.clone(),
                    state: if a.ephemeral {
                        pb::AppState::Ephemeral as i32
                    } else {
                        pb::AppState::Deployed as i32
                    },
                    created_at: a.created_at,
                    stopped_at: 0.0,
                    n_running_tasks: running(&a.id),
                    name: a.name.clone().unwrap_or_default(),
                    metadata: None,
                })
                .collect(),
        }))
    }

    pub(super) async fn h_app_get_by_deployment_name(
        &self,
        r: Request<pb::AppGetByDeploymentNameRequest>,
    ) -> Resp<pb::AppGetByDeploymentNameResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let env = if q.environment_name.is_empty() {
            "main".to_string()
        } else {
            q.environment_name
        };
        let app = self.0.db.read(|d| {
            d.apps
                .values()
                .find(|a| {
                    a.key_id == c.key_id()
                        && a.name.as_deref() == Some(&q.name)
                        && a.environment == env
                        && !a.stopped
                })
                .cloned()
        });
        match app {
            Some(a) => Ok(Response::new(pb::AppGetByDeploymentNameResponse {
                app_id: a.id,
                previous_app_id: String::new(),
                environment_name: a.environment,
                lifecycle: None,
            })),
            // An unknown name is an empty answer, not an error: `deploy`
            // reads it as "no deployment yet".
            None => Ok(Response::new(pb::AppGetByDeploymentNameResponse {
                environment_name: env,
                ..Default::default()
            })),
        }
    }

    // ---- environments -------------------------------------------------------

    pub(super) async fn h_environment_create(
        &self,
        r: Request<pb::EnvironmentCreateRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        if q.name.is_empty() || q.name == "main" {
            return Err(errors::exists(format!(
                "Environment '{}' already exists",
                q.name
            )));
        }
        let ok = self.0.db.write(|d| {
            let key = format!("{}/{}", c.key_id(), q.name);
            if d.environments.contains_key(&key) {
                return false;
            }
            d.environments.insert(
                key,
                crate::modal::db::EnvRec {
                    name: q.name.clone(),
                    key_id: c.key_id(),
                    created_at: now_s(),
                },
            );
            true
        });
        if ok {
            Ok(Response::new(()))
        } else {
            Err(errors::exists(format!(
                "Environment '{}' already exists",
                q.name
            )))
        }
    }

    pub(super) async fn h_environment_list(
        &self,
        r: Request<()>,
    ) -> Resp<pb::EnvironmentListResponse> {
        let c = caller(&r)?;
        let mut items = vec![pb::EnvironmentListItem {
            name: "main".into(),
            webhook_suffix: "cua".into(),
            created_at: 0.0,
            default: true,
            environment_id: "en-main".into(),
            ..Default::default()
        }];
        self.0.db.read(|d| {
            for e in d.environments.values().filter(|e| e.key_id == c.key_id()) {
                items.push(pb::EnvironmentListItem {
                    name: e.name.clone(),
                    webhook_suffix: "cua".into(),
                    created_at: e.created_at,
                    default: false,
                    environment_id: format!("en-{}", e.name),
                    ..Default::default()
                });
            }
        });
        Ok(Response::new(pb::EnvironmentListResponse { items }))
    }

    pub(super) async fn h_environment_delete(
        &self,
        r: Request<pb::EnvironmentDeleteRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        if q.name == "main" {
            return Err(errors::conflict(
                "the default Environment cannot be deleted",
            ));
        }
        let busy = self.0.db.read(|d| {
            d.apps
                .values()
                .any(|a| a.key_id == c.key_id() && a.environment == q.name && !a.stopped)
        });
        if busy {
            return Err(errors::conflict(format!(
                "Environment '{}' still has Apps",
                q.name
            )));
        }
        let gone = self.0.db.write(|d| {
            d.environments
                .remove(&format!("{}/{}", c.key_id(), q.name))
                .is_some()
        });
        if gone {
            Ok(Response::new(()))
        } else {
            Err(errors::not_found(format!(
                "Environment '{}' not found",
                q.name
            )))
        }
    }

    pub(super) async fn h_environment_update(
        &self,
        r: Request<pb::EnvironmentUpdateRequest>,
    ) -> Resp<pb::EnvironmentListItem> {
        let c = caller(&r)?;
        let q = r.into_inner();
        if q.current_name == "main" {
            return Err(errors::conflict(
                "the default Environment cannot be renamed",
            ));
        }
        let new_name = q.name.filter(|n| !n.is_empty());
        let out = self.0.db.write(|d| {
            let key = format!("{}/{}", c.key_id(), q.current_name);
            let mut e = d.environments.remove(&key)?;
            if let Some(n) = &new_name {
                e.name = n.clone();
            }
            let created = e.created_at;
            let name = e.name.clone();
            d.environments
                .insert(format!("{}/{}", c.key_id(), e.name), e);
            Some((name, created))
        });
        match out {
            Some((name, created_at)) => Ok(Response::new(pb::EnvironmentListItem {
                environment_id: format!("en-{name}"),
                name,
                webhook_suffix: "cua".into(),
                created_at,
                ..Default::default()
            })),
            None => Err(errors::not_found(format!(
                "Environment '{}' not found",
                q.current_name
            ))),
        }
    }

    // ---- logs ---------------------------------------------------------------

    pub(super) async fn h_app_fetch_logs(
        &self,
        r: Request<pb::AppFetchLogsRequest>,
    ) -> Resp<pb::AppFetchLogsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let scope = self.0.log_scope(&c, &q.app_id, &q.sandbox_id);
        let since = ts_ns(&q.since, 0);
        let until = ts_ns(&q.until, u64::MAX);
        let mut entries = self
            .0
            .log_entries(&scope, since, until, q.source, &q.search_text)
            .await;
        if q.limit > 0 && entries.len() > q.limit as usize {
            entries.truncate(q.limit as usize);
        }
        let mut batches: Vec<pb::TaskLogsBatch> = Vec::new();
        for (at, sb, fd, text) in entries {
            let item = pb::TaskLogs {
                data: text,
                file_descriptor: fd,
                timestamp: at as f64 / 1e9,
                timestamp_ns: at,
                container_id: sb.task_id.clone(),
                ..Default::default()
            };
            match batches.last_mut() {
                Some(b) if b.sandbox_id == sb.id => b.items.push(item),
                _ => batches.push(pb::TaskLogsBatch {
                    task_id: sb.task_id.clone(),
                    sandbox_id: sb.id.clone(),
                    items: vec![item],
                    ..Default::default()
                }),
            }
        }
        for l in self.0.fn_log_entries(
            &c,
            &q.app_id,
            &q.task_id,
            &q.function_id,
            since,
            until,
            q.source,
            &q.search_text,
        ) {
            batches.push(pb::TaskLogsBatch {
                task_id: l.task_id.clone(),
                function_id: l.function_id.clone(),
                items: vec![pb::TaskLogs {
                    data: l.text.clone(),
                    file_descriptor: if l.fd == 1 {
                        pb::FileDescriptor::Stderr as i32
                    } else {
                        pb::FileDescriptor::Stdout as i32
                    },
                    timestamp: l.at_ns as f64 / 1e9,
                    timestamp_ns: l.at_ns,
                    container_id: l.task_id.clone(),
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        Ok(Response::new(pb::AppFetchLogsResponse { batches }))
    }

    pub(super) async fn h_app_get_logs(
        &self,
        r: Request<pb::AppGetLogsRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::TaskLogsBatch>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        if q.app_id.is_empty() && q.sandbox_id.is_empty() {
            return Err(errors::invalid("an app id or a sandbox id is required"));
        }
        let m = self.0.clone();
        let hold = crate::modal::api::secs(if q.timeout > 0.0 { q.timeout } else { 55.0 });
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + hold;
            let mut cursor: u64 = q.last_entry_id.parse().unwrap_or(0);
            loop {
                let mut scope = m.log_scope(&c, &q.app_id, &q.sandbox_id);
                if !q.task_id.is_empty() {
                    scope.retain(|s| s.task_id == q.task_id);
                }
                let entries = m
                    .log_entries(&scope, cursor, u64::MAX, q.file_descriptor, &q.search_text)
                    .await;
                let fn_entries = m.fn_log_entries(
                    &c,
                    &q.app_id,
                    &q.task_id,
                    &q.function_id,
                    cursor,
                    u64::MAX,
                    q.file_descriptor,
                    &q.search_text,
                );
                let sent = !entries.is_empty() || !fn_entries.is_empty();
                for l in fn_entries {
                    cursor = cursor.max(l.at_ns + 1);
                    let batch = pb::TaskLogsBatch {
                        task_id: l.task_id.clone(),
                        function_id: l.function_id.clone(),
                        entry_id: cursor.to_string(),
                        items: vec![pb::TaskLogs {
                            data: l.text.clone(),
                            file_descriptor: if l.fd == 1 {
                                pb::FileDescriptor::Stderr as i32
                            } else {
                                pb::FileDescriptor::Stdout as i32
                            },
                            timestamp: l.at_ns as f64 / 1e9,
                            timestamp_ns: l.at_ns,
                            container_id: l.task_id.clone(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    };
                    if tx.send(Ok(batch)).await.is_err() {
                        return;
                    }
                }
                if !entries.is_empty() {
                    for (at, sb, fd, text) in entries {
                        cursor = cursor.max(at + 1);
                        let batch = pb::TaskLogsBatch {
                            task_id: sb.task_id.clone(),
                            sandbox_id: sb.id.clone(),
                            entry_id: cursor.to_string(),
                            items: vec![pb::TaskLogs {
                                data: text,
                                file_descriptor: fd,
                                timestamp: at as f64 / 1e9,
                                timestamp_ns: at,
                                container_id: sb.task_id.clone(),
                                ..Default::default()
                            }],
                            ..Default::default()
                        };
                        if tx.send(Ok(batch)).await.is_err() {
                            return;
                        }
                    }
                }
                if sent {
                    continue;
                }
                let done =
                    m.db.read(|d| d.apps.get(&q.app_id).is_none_or(|a| a.stopped));
                if done && !q.app_id.is_empty() {
                    let _ = tx
                        .send(Ok(pb::TaskLogsBatch {
                            app_done: true,
                            entry_id: cursor.to_string(),
                            ..Default::default()
                        }))
                        .await;
                    return;
                }
                if tokio::time::Instant::now() >= deadline {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    pub(super) async fn h_token_info_get(
        &self,
        r: Request<pb::TokenInfoGetRequest>,
    ) -> Resp<pb::TokenInfoGetResponse> {
        let c = caller(&r)?;
        Ok(Response::new(pb::TokenInfoGetResponse {
            token_id: crate::modal::token_id(&c.0.key),
            workspace_id: "ac-cua".into(),
            workspace_name: "cua".into(),
            token_name: c.0.label.clone(),
            ..Default::default()
        }))
    }

    pub(super) async fn h_app_count_logs(
        &self,
        r: Request<pb::AppCountLogsRequest>,
    ) -> Resp<pb::AppCountLogsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let scope = self.0.log_scope(&c, &q.app_id, &q.sandbox_id);
        let since = ts_ns(&q.since, 0);
        let until = ts_ns(&q.until, u64::MAX);
        let bucket = u64::from(q.bucket_secs.max(1)) * 1_000_000_000;
        let entries = self
            .0
            .log_entries(&scope, since, until, q.source, &q.search_text)
            .await;
        let mut buckets: std::collections::BTreeMap<u64, (u32, u32)> = Default::default();
        let task = q.task_id.clone();
        let fn_entries = self
            .0
            .fn_log_entries(
                &c,
                &q.app_id,
                &task,
                &q.function_id,
                since,
                until,
                q.source,
                &q.search_text,
            )
            .into_iter()
            .map(|l| {
                (
                    l.at_ns,
                    (),
                    if l.fd == 1 {
                        pb::FileDescriptor::Stderr as i32
                    } else {
                        pb::FileDescriptor::Stdout as i32
                    },
                    (),
                )
            });
        for (at, _, fd, _) in entries
            .into_iter()
            .map(|(a, _, f, _)| (a, (), f, ()))
            .chain(fn_entries)
        {
            let b = at - at % bucket;
            let e = buckets.entry(b).or_default();
            if fd == pb::FileDescriptor::Stderr as i32 {
                e.1 += 1;
            } else {
                e.0 += 1;
            }
        }
        Ok(Response::new(pb::AppCountLogsResponse {
            app_id: q.app_id.clone(),
            buckets: buckets
                .into_iter()
                .map(
                    |(start, (out, err))| pb::app_count_logs_response::LogBucket {
                        bucket_start_at: Some(ts(start)),
                        stdout_logs: u64::from(out),
                        stderr_logs: u64::from(err),
                        system_logs: 0,
                    },
                )
                .collect(),
        }))
    }
}

impl crate::modal::Modal {
    /// The sandboxes a log query covers.
    pub fn log_scope(
        &self,
        c: &Caller,
        app_id: &str,
        sandbox_id: &str,
    ) -> Vec<crate::modal::sandbox::SandboxRec> {
        self.db.read(|d| {
            d.sandboxes
                .values()
                .filter(|s| s.key_id == c.key_id())
                .filter(|s| {
                    (sandbox_id.is_empty() || s.id == sandbox_id)
                        && (app_id.is_empty() || s.app_id == app_id)
                })
                .cloned()
                .collect()
        })
    }

    /// Function container output for an app (`task_id` empty: every task).
    #[allow(clippy::too_many_arguments)]
    pub fn fn_log_entries(
        &self,
        c: &Caller,
        app_id: &str,
        task_id: &str,
        function_id: &str,
        since_ns: u64,
        until_ns: u64,
        source: i32,
        search: &str,
    ) -> Vec<crate::modal::containers::AppLogLine> {
        if app_id.is_empty() || source == pb::FileDescriptor::Info as i32 {
            return vec![];
        }
        let owns = self.db.read(|d| {
            d.apps.get(app_id).is_some_and(|a| a.key_id == c.key_id())
                || app_id
                    .strip_prefix("ap-ib-")
                    .and_then(|i| d.images.get(i))
                    .is_some_and(|i| i.key_id == c.key_id())
        });
        if !owns {
            return vec![];
        }
        let g = self.app_logs.lock().expect("app logs");
        g.get(app_id)
            .map(|v| {
                v.iter()
                    .filter(|l| l.at_ns >= since_ns && l.at_ns < until_ns)
                    .filter(|l| task_id.is_empty() || l.task_id == task_id)
                    .filter(|l| function_id.is_empty() || l.function_id == function_id)
                    .filter(|l| {
                        let fd = if l.fd == 1 {
                            pb::FileDescriptor::Stderr as i32
                        } else {
                            pb::FileDescriptor::Stdout as i32
                        };
                        source == 0 || source == fd
                    })
                    .filter(|l| search.is_empty() || l.text.contains(search))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn log_entries(
        &self,
        scope: &[crate::modal::sandbox::SandboxRec],
        since_ns: u64,
        until_ns: u64,
        source: i32,
        search: &str,
    ) -> Vec<(u64, crate::modal::sandbox::SandboxRec, i32, String)> {
        let mut out = Vec::new();
        if source == pb::FileDescriptor::Info as i32 {
            return out;
        }
        for sb in scope {
            let Some(s) = self.live.entrypoints.lock().await.get(&sb.id).cloned() else {
                continue;
            };
            for (chunk, bytes) in s.log_between(since_ns, until_ns) {
                let fd = if chunk.fd == crate::modal::exec::STDERR {
                    pb::FileDescriptor::Stderr as i32
                } else {
                    pb::FileDescriptor::Stdout as i32
                };
                if source != 0 && source != fd {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if !search.is_empty() && !text.contains(search) {
                    continue;
                }
                out.push((chunk.at_ns, sb.clone(), fd, text));
            }
        }
        out.sort_by_key(|e| e.0);
        out
    }
}
