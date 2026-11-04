//! Control-plane and container handlers for Functions and Classes.

use crate::modal::api::{Api, secs};
use crate::modal::db::{FunctionRec, now_s};
use crate::modal::functions::{InputState, dec};
use crate::modal::pb::api as pb;
use crate::modal::router::TaskClaim;
use crate::modal::{Caller, errors};
use std::time::Duration;
use tonic::{Request, Response, Status};

type Resp<T> = Result<Response<T>, Status>;

fn caller<T>(r: &Request<T>) -> Result<Caller, Status> {
    r.extensions()
        .get::<Caller>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated("Token missing. Could not authenticate client."))
}

/// The task of a Function container's token; the container-side RPCs are
/// for containers only.
fn task<T>(r: &Request<T>) -> Result<String, Status> {
    r.extensions()
        .get::<TaskClaim>()
        .map(|t| t.0.clone())
        .ok_or_else(|| {
            errors::denied("this call is for Function containers (it needs a task token)")
        })
}

impl Api {
    // ---- registration -----------------------------------------------------------

    pub(super) async fn h_function_precreate(
        &self,
        r: Request<pb::FunctionPrecreateRequest>,
    ) -> Resp<pb::FunctionPrecreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let rec = self.0.precreate_function(&c, &q)?;
        let mut methods = std::collections::HashMap::new();
        for (name, md) in &q.method_definitions {
            methods.insert(
                name.clone(),
                pb::FunctionHandleMetadata {
                    function_name: name.clone(),
                    function_type: md.function_type,
                    web_url: md.web_url.clone(),
                    is_method: true,
                    use_method_name: name.clone(),
                    function_schema: md.function_schema.clone(),
                    supported_input_formats: md.supported_input_formats.clone(),
                    supported_output_formats: md.supported_output_formats.clone(),
                    ..Default::default()
                },
            );
        }
        let meta = pb::FunctionHandleMetadata {
            function_name: q.function_name.clone(),
            function_type: q.function_type,
            definition_id: rec.id.clone(),
            method_handle_metadata: methods,
            function_schema: q.function_schema.clone(),
            supported_input_formats: q.supported_input_formats.clone(),
            supported_output_formats: q.supported_output_formats.clone(),
            app_id: rec.app_id.clone(),
            ..Default::default()
        };
        Ok(Response::new(pb::FunctionPrecreateResponse {
            function_id: rec.id,
            handle_metadata: Some(meta),
        }))
    }

    pub(super) async fn h_function_create(
        &self,
        r: Request<pb::FunctionCreateRequest>,
    ) -> Resp<pb::FunctionCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let rec = self.0.create_function(&c, &q)?;
        let def = self.0.function_def(&rec);
        Ok(Response::new(pb::FunctionCreateResponse {
            function_id: rec.id.clone(),
            function: Some(def.clone()),
            handle_metadata: Some(self.0.function_handle(&rec)),
            function_data: Some(function_data_of(&rec, &def, q.function_data.as_ref())),
            ..Default::default()
        }))
    }

    pub(super) async fn h_function_get(
        &self,
        r: Request<pb::FunctionGetRequest>,
    ) -> Resp<pb::FunctionGetResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let env = if q.environment_name.is_empty() {
            "main".to_string()
        } else {
            q.environment_name
        };
        let app =
            m.db.read(|d| {
                d.apps
                    .values()
                    .find(|a| {
                        a.key_id == c.key_id()
                            && a.name.as_deref() == Some(&q.app_name)
                            && a.environment == env
                            && !a.stopped
                            && !a.ephemeral
                    })
                    .cloned()
            })
            .ok_or_else(|| {
                errors::not_found(format!(
                    "App '{}' not found in environment '{env}'",
                    q.app_name
                ))
            })?;
        let id = app
            .function_ids
            .get(&q.object_tag)
            .cloned()
            .ok_or_else(|| {
                errors::not_found(format!(
                    "Function '{}' not found in App '{}'",
                    q.object_tag, q.app_name
                ))
            })?;
        let f = m.function_of(&c, &id)?;
        let def = m.function_def(&f);
        Ok(Response::new(pb::FunctionGetResponse {
            function_id: f.id.clone(),
            handle_metadata: Some(m.function_handle(&f)),
            function: Some(function_data_of(&f, &def, None)),
            ..Default::default()
        }))
    }

    pub(super) async fn h_function_get_by_id(
        &self,
        r: Request<pb::FunctionGetByIdRequest>,
    ) -> Resp<pb::FunctionGetByIdResponse> {
        let c = caller(&r)?;
        let f = self.0.function_of(&c, &r.into_inner().function_id)?;
        let def = self.0.function_def(&f);
        Ok(Response::new(pb::FunctionGetByIdResponse {
            function: Some(function_data_of(&f, &def, None)),
            handle_metadata: Some(self.0.function_handle(&f)),
        }))
    }

    pub(super) async fn h_class_create(
        &self,
        r: Request<pb::ClassCreateRequest>,
    ) -> Resp<pb::ClassCreateResponse> {
        let c = caller(&r)?;
        let rec = self.0.create_class(&c, &r.into_inner())?;
        Ok(Response::new(pb::ClassCreateResponse {
            class_id: rec.id.clone(),
            handle_metadata: Some(self.0.class_handle(&rec)),
        }))
    }

    pub(super) async fn h_class_get(
        &self,
        r: Request<pb::ClassGetRequest>,
    ) -> Resp<pb::ClassGetResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let env = if q.environment_name.is_empty() {
            "main".to_string()
        } else {
            q.environment_name
        };
        let app = self
            .0
            .db
            .read(|d| {
                d.apps
                    .values()
                    .find(|a| {
                        a.key_id == c.key_id()
                            && a.name.as_deref() == Some(&q.app_name)
                            && a.environment == env
                            && !a.stopped
                            && !a.ephemeral
                    })
                    .cloned()
            })
            .ok_or_else(|| {
                errors::not_found(format!(
                    "App '{}' not found in environment '{env}'",
                    q.app_name
                ))
            })?;
        let id = app.class_ids.get(&q.object_tag).cloned().ok_or_else(|| {
            errors::not_found(format!(
                "Class '{}' not found in App '{}'",
                q.object_tag, q.app_name
            ))
        })?;
        let cl = self
            .0
            .db
            .read(|d| d.classes.get(&id).cloned())
            .ok_or_else(|| errors::not_found("class vanished"))?;
        Ok(Response::new(pb::ClassGetResponse {
            class_id: cl.id.clone(),
            handle_metadata: Some(self.0.class_handle(&cl)),
            ..Default::default()
        }))
    }

    pub(super) async fn h_function_bind_params(
        &self,
        r: Request<pb::FunctionBindParamsRequest>,
    ) -> Resp<pb::FunctionBindParamsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let bound = self.0.bind_params(
            &c,
            &q.function_id,
            &q.serialized_params,
            q.function_options.as_ref(),
        )?;
        Ok(Response::new(pb::FunctionBindParamsResponse {
            bound_function_id: bound.id.clone(),
            handle_metadata: Some(self.0.function_handle(&bound)),
        }))
    }

    pub(super) async fn h_app_publish(
        &self,
        r: Request<pb::AppPublishRequest>,
    ) -> Resp<pb::AppPublishResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let deploy = q.app_state == pb::AppState::Deployed as i32 && !q.name.is_empty();
        let now = now_s();
        let out = m.db.write(|d| {
            let a = d
                .apps
                .get_mut(&q.app_id)
                .filter(|a| a.key_id == c.key_id())?;
            a.function_ids = q.function_ids.clone().into_iter().collect();
            a.class_ids = q.class_ids.clone().into_iter().collect();
            if deploy {
                a.name = Some(q.name.clone());
                a.ephemeral = false;
                a.description = q.name.clone();
                a.deployed_at = now;
                a.deployment_version += 1;
                for (k, v) in &q.tags {
                    a.tags.insert(k.clone(), v.clone());
                }
            }
            Some((a.id.clone(), a.deployment_version))
        });
        let Some((id, _v)) = out else {
            return Err(errors::not_found(format!("App {} not found", q.app_id)));
        };
        Ok(Response::new(pb::AppPublishResponse {
            url: format!("{}/apps/{id}", m.cfg.api_url),
            server_warnings: vec![],
            deployed_at: if deploy { now } else { 0.0 },
        }))
    }

    pub(super) async fn h_app_get_layout(
        &self,
        r: Request<pb::AppGetLayoutRequest>,
    ) -> Resp<pb::AppGetLayoutResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        let a = self
            .0
            .db
            .read(|d| d.apps.get(&id).filter(|a| a.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("App {id} not found")))?;
        let mut objects = Vec::new();
        for fid in a.function_ids.values().chain(a.class_ids.values()) {
            objects.push(self.0.object_for(fid));
        }
        Ok(Response::new(pb::AppGetLayoutResponse {
            app_layout: Some(pb::AppLayout {
                objects,
                function_ids: a.function_ids.into_iter().collect(),
                class_ids: a.class_ids.into_iter().collect(),
            }),
        }))
    }

    pub(super) async fn h_app_get_objects(
        &self,
        r: Request<pb::AppGetObjectsRequest>,
    ) -> Resp<pb::AppGetObjectsResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        let a = self
            .0
            .db
            .read(|d| d.apps.get(&id).filter(|a| a.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("App {id} not found")))?;
        let items = a
            .function_ids
            .iter()
            .chain(a.class_ids.iter())
            .map(|(tag, oid)| pb::AppGetObjectsItem {
                tag: tag.clone(),
                object: Some(self.0.object_for(oid)),
            })
            .collect();
        Ok(Response::new(pb::AppGetObjectsResponse { items }))
    }

    // ---- calls ----------------------------------------------------------------------

    pub(super) async fn h_function_map(
        &self,
        r: Request<pb::FunctionMapRequest>,
    ) -> Resp<pb::FunctionMapResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let f = m.function_of(&c, &q.function_id)?;
        if !f.created {
            return Err(errors::conflict(format!(
                "Function {} has not been created",
                f.id
            )));
        }
        let call = m.new_call(&c, &f, q.function_call_invocation_type);
        let pipelined = if q.pipelined_inputs.is_empty() {
            vec![]
        } else {
            m.add_inputs(&c, &f.id, &call, &q.pipelined_inputs)?
        };
        Ok(Response::new(m.map_response(&f, call, pipelined)))
    }

    pub(super) async fn h_function_put_inputs(
        &self,
        r: Request<pb::FunctionPutInputsRequest>,
    ) -> Resp<pb::FunctionPutInputsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.0.function_of(&c, &q.function_id)?;
        let inputs = self
            .0
            .add_inputs(&c, &q.function_id, &q.function_call_id, &q.inputs)?;
        Ok(Response::new(pb::FunctionPutInputsResponse { inputs }))
    }

    pub(super) async fn h_function_get_outputs(
        &self,
        r: Request<pb::FunctionGetOutputsRequest>,
    ) -> Resp<pb::FunctionGetOutputsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let range = match (q.start_idx, q.end_idx) {
            (Some(a), Some(b)) => Some((a, b)),
            (Some(a), None) => Some((a, i32::MAX)),
            (None, Some(b)) => Some((0, b)),
            (None, None) => None,
        };
        let out = self
            .0
            .wait_outputs(
                &c,
                &q.function_call_id,
                range,
                &q.last_entry_id,
                q.max_values.max(0) as usize,
                secs(q.timeout.max(0.0)),
                q.clear_on_success,
            )
            .await?;
        Ok(Response::new(out))
    }

    pub(super) async fn h_function_call_from_id(
        &self,
        r: Request<pb::FunctionCallFromIdRequest>,
    ) -> Resp<pb::FunctionCallFromIdResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().function_call_id;
        let g = self.0.fns.lock().expect("fns");
        let call = g
            .calls
            .get(&id)
            .filter(|x| x.key_id == c.key_id())
            .ok_or_else(|| errors::not_found(format!("Function call {id} not found")))?;
        Ok(Response::new(pb::FunctionCallFromIdResponse {
            function_call_id: id,
            num_inputs: call.inputs.len() as i32,
            metadata: Some(pb::FunctionCallHandleMetadata {
                app_id: call.app_id.clone(),
                function_id: call.function_id.clone(),
            }),
        }))
    }

    pub(super) async fn h_function_call_cancel(
        &self,
        r: Request<pb::FunctionCallCancelRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.0
            .cancel_call(&c, &q.function_call_id, q.terminate_containers)?;
        Ok(Response::new(()))
    }

    pub(super) async fn h_function_retry_inputs(
        &self,
        r: Request<pb::FunctionRetryInputsRequest>,
    ) -> Resp<pb::FunctionRetryInputsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let call_id = self.0.call_of_jwt(&q.function_call_jwt)?;
        let mut jwts = Vec::new();
        {
            let mut g = self.0.fns.lock().expect("fns");
            let fns = &mut *g;
            let call = fns
                .calls
                .get_mut(&call_id)
                .filter(|x| x.key_id == c.key_id())
                .ok_or_else(|| errors::not_found(format!("Function call {call_id} not found")))?;
            let function_id = call.function_id.clone();
            for it in &q.inputs {
                let claims = self
                    .0
                    .signer
                    .verify(&it.input_jwt, "input")
                    .map_err(|why| errors::denied(format!("invalid input token: {why}")))?;
                let input_id = claims["sub"].as_str().unwrap_or_default().to_string();
                let Some(slot) = call.inputs.iter_mut().find(|s| s.input_id == input_id) else {
                    continue;
                };
                slot.retry_count = it.retry_count;
                if let Some(i) = &it.input {
                    slot.input = i.clone();
                }
                slot.state = InputState::Pending;
                slot.output = None;
                slot.task = None;
                fns.queues
                    .entry(function_id.clone())
                    .or_default()
                    .pending
                    .push_back((call_id.clone(), input_id.clone()));
                jwts.push(it.input_jwt.clone());
            }
        }
        self.0.fn_changed();
        Ok(Response::new(pb::FunctionRetryInputsResponse {
            input_jwts: jwts,
        }))
    }

    pub(super) async fn h_function_call_get_info(
        &self,
        r: Request<pb::FunctionCallGetInfoRequest>,
    ) -> Resp<pb::FunctionCallGetInfoResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let g = self.0.fns.lock().expect("fns");
        let call = g
            .calls
            .get(&q.function_call_id)
            .filter(|x| x.key_id == c.key_id())
            .ok_or_else(|| {
                errors::not_found(format!("Function call {} not found", q.function_call_id))
            })?;
        let count =
            |f: &dyn Fn(&crate::modal::functions::InputSlot) -> bool| pb::InputCategoryInfo {
                total: call.inputs.iter().filter(|s| f(s)).count() as i32,
                latest: vec![],
            };
        let status = |s: &crate::modal::functions::InputSlot| {
            s.output
                .as_ref()
                .and_then(|o| o.result.as_ref())
                .map(|r| r.status)
        };
        Ok(Response::new(pb::FunctionCallGetInfoResponse {
            info: Some(pb::FunctionCallInfo {
                function_call_id: call.id.clone(),
                created_at: call.created_at,
                scheduled_at: call.created_at,
                pending_inputs: Some(count(&|s| s.output.is_none())),
                failed_inputs: Some(count(&|s| matches!(status(s), Some(2 | 5 | 6)))),
                succeeded_inputs: Some(count(&|s| status(s) == Some(1))),
                timeout_inputs: Some(count(&|s| status(s) == Some(4))),
                cancelled_inputs: Some(count(&|s| status(s) == Some(3))),
                total_inputs: call.inputs.len() as i32,
                ..Default::default()
            }),
        }))
    }

    pub(super) async fn h_function_get_current_stats(
        &self,
        r: Request<pb::FunctionGetCurrentStatsRequest>,
    ) -> Resp<pb::FunctionStats> {
        let c = caller(&r)?;
        let id = r.into_inner().function_id;
        self.0.function_of(&c, &id)?;
        let g = self.0.fns.lock().expect("fns");
        let stats = match g.queues.get(&id) {
            Some(q) => pb::FunctionStats {
                backlog: q.pending.len() as u32,
                num_total_tasks: q.containers.len() as u32,
                num_running_inputs: q.containers.values().map(|c| c.running_inputs as u32).sum(),
                input_headroom: 0,
            },
            None => pb::FunctionStats::default(),
        };
        Ok(Response::new(stats))
    }

    pub(super) async fn h_function_get_call_graph(
        &self,
        r: Request<pb::FunctionGetCallGraphRequest>,
    ) -> Resp<pb::FunctionGetCallGraphResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().function_call_id;
        let g = self.0.fns.lock().expect("fns");
        let call = g
            .calls
            .get(&id)
            .filter(|x| x.key_id == c.key_id())
            .ok_or_else(|| errors::not_found(format!("Function call {id} not found")))?;
        let f = self.0.function(&call.function_id);
        let def = f
            .as_ref()
            .map(|f| self.0.function_def(f))
            .unwrap_or_default();
        let inputs = call
            .inputs
            .iter()
            .map(|s| pb::InputCallGraphInfo {
                input_id: s.input_id.clone(),
                status: s
                    .output
                    .as_ref()
                    .and_then(|o| o.result.as_ref())
                    .map(|r| r.status)
                    .unwrap_or(0),
                function_call_id: call.id.clone(),
                task_id: s.task.clone().unwrap_or_default(),
            })
            .collect();
        Ok(Response::new(pb::FunctionGetCallGraphResponse {
            inputs,
            function_calls: vec![pb::FunctionCallCallGraphInfo {
                function_call_id: call.id.clone(),
                parent_input_id: String::new(),
                function_name: def.function_name.clone(),
                module_name: def.module_name.clone(),
            }],
            truncated: false,
        }))
    }

    /// `spawn_map` and `FunctionCall` streaming inputs say when they are done.
    pub(super) async fn h_function_finish_inputs(
        &self,
        r: Request<pb::FunctionFinishInputsRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let g = self.0.fns.lock().expect("fns");
        g.calls
            .get(&q.function_call_id)
            .filter(|x| x.key_id == c.key_id())
            .ok_or_else(|| {
                errors::not_found(format!("Function call {} not found", q.function_call_id))
            })?;
        Ok(Response::new(()))
    }

    pub(super) async fn h_function_get_scheduling_params(
        &self,
        r: Request<pb::FunctionGetSchedulingParamsRequest>,
    ) -> Resp<pb::FunctionGetSchedulingParamsResponse> {
        let c = caller(&r)?;
        let f = self.0.function_of(&c, &r.into_inner().function_id)?;
        let def = self.0.function_def(&f);
        Ok(Response::new(pb::FunctionGetSchedulingParamsResponse {
            autoscaler_configuration: Some(pb::AutoscalerConfiguration {
                settings: Some(pb::AutoscalerSettings {
                    min_containers: Some(f.min_containers.unwrap_or(def.warm_pool_size)),
                    max_containers: f
                        .max_containers
                        .or(Some(def.concurrency_limit).filter(|v| *v > 0)),
                    buffer_containers: f.buffer_containers,
                    scaledown_window: f
                        .scaledown_window
                        .or(Some(def.task_idle_timeout_secs).filter(|v| *v > 0)),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }))
    }

    pub(super) async fn h_function_update_scheduling_params(
        &self,
        r: Request<pb::FunctionUpdateSchedulingParamsRequest>,
    ) -> Resp<pb::FunctionUpdateSchedulingParamsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.0.function_of(&c, &q.function_id)?;
        let s = q.settings.unwrap_or_default();
        self.0.db.write(|d| {
            if let Some(f) = d.functions.get_mut(&q.function_id) {
                if s.min_containers.is_some() {
                    f.min_containers = s.min_containers;
                }
                if s.max_containers.is_some() {
                    f.max_containers = s.max_containers;
                }
                if s.buffer_containers.is_some() {
                    f.buffer_containers = s.buffer_containers;
                }
                if s.scaledown_window.is_some() {
                    f.scaledown_window = s.scaledown_window;
                }
            }
        });
        self.0.fn_changed();
        let f = self
            .0
            .function(&q.function_id)
            .unwrap_or_else(|| unreachable!());
        Ok(Response::new(pb::FunctionUpdateSchedulingParamsResponse {
            current_settings: Some(pb::AutoscalerSettings {
                min_containers: f.min_containers,
                max_containers: f.max_containers,
                buffer_containers: f.buffer_containers,
                scaledown_window: f.scaledown_window,
                ..Default::default()
            }),
        }))
    }

    pub(super) async fn h_task_list(
        &self,
        r: Request<pb::TaskListRequest>,
    ) -> Resp<pb::TaskListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let g = self.0.fns.lock().expect("fns");
        let mut tasks = Vec::new();
        for (fid, queue) in &g.queues {
            let Some(f) = self.0.function(fid).filter(|f| f.key_id == c.key_id()) else {
                continue;
            };
            if !q.app_id.is_empty() && f.app_id != q.app_id {
                continue;
            }
            for t in queue.containers.values() {
                tasks.push(pb::TaskStats {
                    task_id: t.task_id.clone(),
                    app_id: f.app_id.clone(),
                    app_description: f.name.clone(),
                    started_at: now_s(),
                    enqueued_at: now_s(),
                });
            }
        }
        Ok(Response::new(pb::TaskListResponse { tasks }))
    }

    // ---- the container side --------------------------------------------------------

    pub(super) async fn h_container_heartbeat(
        &self,
        r: Request<pb::ContainerHeartbeatRequest>,
    ) -> Resp<pb::ContainerHeartbeatResponse> {
        let t = task(&r)?;
        let mut cancel = None;
        {
            let mut g = self.0.fns.lock().expect("fns");
            for q in g.queues.values_mut() {
                if let Some(c) = q.containers.get_mut(&t)
                    && !c.cancel.is_empty()
                {
                    cancel = Some(pb::CancelInputEvent {
                        input_ids: std::mem::take(&mut c.cancel),
                        terminate_containers: c.terminate_on_cancel,
                        cancellation_reason: "cancelled".into(),
                    });
                }
            }
        }
        Ok(Response::new(pb::ContainerHeartbeatResponse {
            cancel_input_event: cancel,
        }))
    }

    pub(super) async fn h_function_get_inputs(
        &self,
        r: Request<pb::FunctionGetInputsRequest>,
    ) -> Resp<pb::FunctionGetInputsResponse> {
        let t = task(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let batch = q.batch_max_size.max(1) as usize;
        let linger = Duration::from_millis(q.batch_linger_ms);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut rx = m.fn_events.subscribe();
        let mut first_at: Option<tokio::time::Instant> = None;
        loop {
            let mut items = Vec::new();
            {
                let mut g = m.fns.lock().expect("fns");
                let fns = &mut *g;
                let queue = fns.queues.entry(q.function_id.clone()).or_default();
                if let Some(c) = queue.containers.get_mut(&t) {
                    c.ready = true;
                    if c.stop {
                        return Ok(Response::new(pb::FunctionGetInputsResponse {
                            inputs: vec![pb::FunctionGetInputsItem {
                                kill_switch: true,
                                ..Default::default()
                            }],
                            rate_limit_sleep_duration: 0.0,
                        }));
                    }
                }
                let ready_enough = queue.pending.len() >= batch
                    || (!queue.pending.is_empty()
                        && (q.batch_max_size == 0
                            || first_at.is_some_and(|f| {
                                tokio::time::Instant::now().duration_since(f) >= linger
                            })));
                if !queue.pending.is_empty() && first_at.is_none() {
                    first_at = Some(tokio::time::Instant::now());
                }
                if ready_enough {
                    let mut taken = Vec::new();
                    while taken.len() < batch {
                        let Some((call_id, input_id)) = queue.pending.pop_front() else {
                            break;
                        };
                        taken.push((call_id, input_id));
                    }
                    let now = tokio::time::Instant::now();
                    if let Some(c) = queue.containers.get_mut(&t) {
                        c.running_inputs += taken.len();
                        c.last_busy = now;
                    }
                    for (call_id, input_id) in taken {
                        let Some(call) = fns.calls.get_mut(&call_id) else {
                            continue;
                        };
                        if call.cancelled {
                            continue;
                        }
                        let invocation = call.invocation;
                        let Some(slot) = call.inputs.iter_mut().find(|s| s.input_id == input_id)
                        else {
                            continue;
                        };
                        if slot.state == InputState::Done {
                            continue;
                        }
                        slot.state = InputState::Running;
                        slot.started = Some(now);
                        slot.task = Some(t.clone());
                        items.push(pb::FunctionGetInputsItem {
                            input_id: slot.input_id.clone(),
                            input: Some(slot.input.clone()),
                            kill_switch: false,
                            function_call_id: call_id.clone(),
                            function_call_invocation_type: invocation,
                            retry_count: slot.retry_count,
                            function_map_idx: Some(slot.idx),
                            attempt_token: String::new(),
                            from_input_plane: false,
                        });
                    }
                }
            }
            if !items.is_empty() {
                m.fn_changed();
                return Ok(Response::new(pb::FunctionGetInputsResponse {
                    inputs: items,
                    rate_limit_sleep_duration: 0.0,
                }));
            }
            let wait_until = match first_at {
                Some(f) if q.batch_max_size > 0 => (f + linger).min(deadline),
                _ => deadline,
            };
            if tokio::time::Instant::now() >= deadline {
                return Ok(Response::new(pb::FunctionGetInputsResponse::default()));
            }
            let _ = tokio::time::timeout_at(wait_until, rx.changed()).await;
        }
    }

    pub(super) async fn h_function_put_outputs(
        &self,
        r: Request<pb::FunctionPutOutputsRequest>,
    ) -> Resp<()> {
        let t = task(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        for it in q.outputs {
            let status = it.result.as_ref().map(|r| r.status).unwrap_or(0);
            let call_id = if it.function_call_id.is_empty() {
                let g = m.fns.lock().expect("fns");
                g.calls
                    .values()
                    .find(|c| c.inputs.iter().any(|s| s.input_id == it.input_id))
                    .map(|c| c.id.clone())
                    .unwrap_or_default()
            } else {
                it.function_call_id.clone()
            };
            {
                let mut g = m.fns.lock().expect("fns");
                for queue in g.queues.values_mut() {
                    if let Some(c) = queue.containers.get_mut(&t) {
                        c.running_inputs = c.running_inputs.saturating_sub(1);
                        c.last_busy = tokio::time::Instant::now();
                    }
                }
            }
            let item = pb::FunctionGetOutputsItem {
                result: it.result.clone(),
                idx: it.function_map_idx.unwrap_or(0),
                input_id: it.input_id.clone(),
                data_format: it.data_format,
                task_id: t.clone(),
                input_started_at: it.input_started_at,
                output_created_at: it.output_created_at,
                retry_count: it.retry_count,
                fc_trace_tag: String::new(),
            };
            // Failures and timeouts are retried by the function's policy;
            // a cancelled input (TERMINATED) is final.
            let retriable = !matches!(status, 1 | 3 | 5);
            m.finish_input(&call_id, &it.input_id, item, retriable);
        }
        Ok(Response::new(()))
    }

    pub(super) async fn h_function_call_put_data_out(
        &self,
        r: Request<pb::FunctionCallPutDataRequest>,
    ) -> Resp<()> {
        let q = r.into_inner();
        let id = match q.call_info {
            Some(pb::function_call_put_data_request::CallInfo::FunctionCallId(i)) => i,
            _ => return Err(errors::invalid("a function call id is required")),
        };
        {
            let mut g = self.0.fns.lock().expect("fns");
            let call = g
                .calls
                .get_mut(&id)
                .ok_or_else(|| errors::not_found(format!("Function call {id} not found")))?;
            for ch in q.data_chunks {
                call.data_out.insert(ch.index, ch);
            }
        }
        self.0.fn_changed();
        Ok(Response::new(()))
    }

    pub(super) async fn h_function_call_get_data_out(
        &self,
        r: Request<pb::FunctionCallGetDataRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::DataChunk>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let id = match q.call_info {
            Some(pb::function_call_get_data_request::CallInfo::FunctionCallId(i)) => i,
            _ => return Err(errors::invalid("a function call id is required")),
        };
        let m = self.0.clone();
        {
            let g = m.fns.lock().expect("fns");
            g.calls
                .get(&id)
                .filter(|x| x.key_id == c.key_id())
                .ok_or_else(|| errors::not_found(format!("Function call {id} not found")))?;
        }
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut last = q.last_index;
            let mut events = m.fn_events.subscribe();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(55);
            loop {
                let chunks: Vec<pb::DataChunk> = {
                    let g = m.fns.lock().expect("fns");
                    match g.calls.get(&id) {
                        Some(call) => call
                            .data_out
                            .range(last + 1..)
                            .map(|(_, c)| c.clone())
                            .collect(),
                        None => return,
                    }
                };
                for ch in chunks {
                    last = ch.index;
                    let done = ch.data_format == crate::modal::functions::GENERATOR_DONE;
                    if tx.send(Ok(ch)).await.is_err() {
                        return;
                    }
                    if done {
                        return;
                    }
                }
                if tokio::time::timeout_at(deadline, events.changed())
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    pub(super) async fn h_container_stop(
        &self,
        r: Request<pb::ContainerStopRequest>,
    ) -> Resp<pb::ContainerStopResponse> {
        let q = r.into_inner();
        {
            let mut g = self.0.fns.lock().expect("fns");
            for queue in g.queues.values_mut() {
                if let Some(c) = queue.containers.get_mut(&q.task_id) {
                    c.stop = true;
                }
            }
        }
        self.0.fn_changed();
        Ok(Response::new(pb::ContainerStopResponse {}))
    }

    pub(super) async fn h_task_result(&self, r: Request<pb::TaskResultRequest>) -> Resp<()> {
        let t = task(&r)?;
        let q = r.into_inner();
        let result = q.result.unwrap_or_default();
        // The container failed while starting (importing the user's code):
        // what waits for it fails with that result.
        let mut waiting: Vec<(String, String)> = Vec::new();
        {
            let mut g = self.0.fns.lock().expect("fns");
            let fns = &mut *g;
            let mut fid = None;
            for (f, queue) in fns.queues.iter_mut() {
                if queue.containers.contains_key(&t) {
                    fid = Some(f.clone());
                    queue.last_error = Some(result.exception.clone());
                    waiting.extend(queue.pending.drain(..));
                }
            }
            if let Some(f) = fid {
                for call in fns.calls.values() {
                    if call.function_id != f {
                        continue;
                    }
                    for s in &call.inputs {
                        if s.state == InputState::Running && s.task.as_deref() == Some(&t) {
                            waiting.push((call.id.clone(), s.input_id.clone()));
                        }
                    }
                }
            }
        }
        for (call, input) in waiting {
            let item = pb::FunctionGetOutputsItem {
                result: Some(result.clone()),
                ..Default::default()
            };
            self.0.finish_input(&call, &input, item, false);
        }
        Ok(Response::new(()))
    }

    pub(super) async fn h_function_get_dynamic_concurrency(
        &self,
        r: Request<pb::FunctionGetDynamicConcurrencyRequest>,
    ) -> Resp<pb::FunctionGetDynamicConcurrencyResponse> {
        let q = r.into_inner();
        Ok(Response::new(pb::FunctionGetDynamicConcurrencyResponse {
            concurrency: if q.target_concurrency > 0 {
                q.target_concurrency
            } else {
                q.max_concurrency
            },
        }))
    }
}

/// `FunctionData` for a function (what `FunctionGet` and `FunctionCreate`
/// answer with).
pub fn function_data_of(
    f: &FunctionRec,
    def: &pb::Function,
    given: Option<&pb::FunctionData>,
) -> pb::FunctionData {
    let mut data = if let Some(d) = given {
        d.clone()
    } else if !f.data.is_empty() {
        dec(&f.data)
    } else {
        pb::FunctionData {
            module_name: def.module_name.clone(),
            function_name: def.function_name.clone(),
            function_type: def.function_type,
            warm_pool_size: def.warm_pool_size,
            concurrency_limit: def.concurrency_limit,
            task_idle_timeout_secs: def.task_idle_timeout_secs,
            timeout_secs: def.timeout_secs,
            web_url: def.web_url.clone(),
            webhook_config: def.webhook_config.clone(),
            is_class: def.is_class,
            class_parameter_info: def.class_parameter_info.clone(),
            is_method: def.is_method,
            method_definitions: def.method_definitions.clone(),
            method_definitions_set: def.method_definitions_set,
            function_schema: def.function_schema.clone(),
            supported_input_formats: def.supported_input_formats.clone(),
            supported_output_formats: def.supported_output_formats.clone(),
            implementation_name: def.implementation_name.clone(),
            ..Default::default()
        }
    };
    // Clients read the Function's settings from the first ranked function.
    if data.ranked_functions.is_empty() {
        data.ranked_functions
            .push(pb::function_data::RankedFunction {
                rank: 0,
                function: Some(def.clone()),
            });
    }
    data
}
