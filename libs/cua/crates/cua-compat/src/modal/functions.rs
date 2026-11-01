//! Functions: definitions, calls, inputs and outputs.
//!
//! A Function is code the client registers with an App (`FunctionCreate`)
//! and calls (`FunctionMap`, `FunctionPutInputs`, `FunctionGetOutputs`). The
//! gateway keeps each call's inputs in a queue per function; containers (see
//! `containers.rs`) pull them with `FunctionGetInputs`, run the user's code
//! with Modal's own container runtime, and push results with
//! `FunctionPutOutputs`. Payloads (pickled arguments and results) pass
//! through untouched.

use crate::modal::db::{ClassRec, FunctionRec, now_s};
use crate::modal::pb::api as pb;
use crate::modal::{Caller, Modal, errors, ids};
use base64::Engine;
use prost::Message;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tonic::Status;

pub const GENERATOR_DONE: i32 = pb::DataFormat::GeneratorDone as i32;

pub fn enc<M: Message>(m: &M) -> String {
    base64::engine::general_purpose::STANDARD.encode(m.encode_to_vec())
}

pub fn dec<M: Message + Default>(s: &str) -> M {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()
        .and_then(|b| M::decode(b.as_slice()).ok())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputState {
    Pending,
    Running,
    Done,
}

#[derive(Debug, Clone)]
pub struct InputSlot {
    pub idx: i32,
    pub input_id: String,
    pub input: pb::FunctionInput,
    pub retry_count: u32,
    pub state: InputState,
    pub output: Option<pb::FunctionGetOutputsItem>,
    /// Delivery order of the output (`last_entry_id`).
    pub seq: u64,
    pub started: Option<tokio::time::Instant>,
    /// The container (task id) running it.
    pub task: Option<String>,
}

#[derive(Debug)]
pub struct Call {
    pub id: String,
    pub function_id: String,
    pub key_id: String,
    pub app_id: String,
    pub created_at: f64,
    pub invocation: i32,
    pub inputs: Vec<InputSlot>,
    pub next_seq: u64,
    pub cancelled: bool,
    /// Generator chunks: index -> chunk.
    pub data_out: BTreeMap<u64, pb::DataChunk>,
}

impl Call {
    fn slot_mut(&mut self, input_id: &str) -> Option<&mut InputSlot> {
        self.inputs.iter_mut().find(|s| s.input_id == input_id)
    }
}

/// A container the gateway runs for a function.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub task_id: String,
    pub started_at: tokio::time::Instant,
    /// Ready: it asked for inputs at least once.
    pub ready: bool,
    pub running_inputs: usize,
    pub last_busy: tokio::time::Instant,
    /// Ask it to exit (the next `FunctionGetInputs` carries a kill switch).
    pub stop: bool,
    /// Input ids to cancel (delivered with the heartbeat).
    pub cancel: Vec<String>,
    pub terminate_on_cancel: bool,
    /// The Space the container runs in (not for a host-process test runtime).
    pub space: Option<(crate::backend::Identity, String)>,
}

#[derive(Default)]
pub struct FnQueue {
    /// `(call id, input id)` waiting for a container.
    pub pending: VecDeque<(String, String)>,
    pub containers: HashMap<String, ContainerInfo>,
    /// Containers being started.
    pub starting: u32,
    /// Consecutive container start failures.
    pub failures: u32,
    pub last_error: Option<String>,
}

#[derive(Default)]
pub struct Fns {
    pub calls: HashMap<String, Call>,
    pub queues: HashMap<String, FnQueue>,
}

impl Modal {
    pub fn function(&self, id: &str) -> Option<FunctionRec> {
        self.db.read(|d| d.functions.get(id).cloned())
    }

    pub fn function_of(&self, c: &Caller, id: &str) -> Result<FunctionRec, Status> {
        match self.function(id) {
            Some(f) if f.key_id == c.key_id() => Ok(f),
            _ => Err(errors::not_found(format!("Function {id} not found"))),
        }
    }

    pub fn function_def(&self, f: &FunctionRec) -> pb::Function {
        if !f.definition.is_empty() {
            return dec(&f.definition);
        }
        // A create with ranked functions (GPU lists): the first rank.
        let data: pb::FunctionData = dec(&f.data);
        data.ranked_functions
            .into_iter()
            .min_by_key(|r| r.rank)
            .and_then(|r| r.function)
            .unwrap_or_default()
    }

    /// The bell: anything about calls or containers changed.
    pub fn fn_changed(&self) {
        self.fn_events.send_modify(|v| *v += 1);
    }

    /// The handle metadata a client gets for a function.
    pub fn function_handle(&self, f: &FunctionRec) -> pb::FunctionHandleMetadata {
        let def = self.function_def(f);
        let mut methods = HashMap::new();
        for (name, md) in &def.method_definitions {
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
        pb::FunctionHandleMetadata {
            function_name: if def.function_name.is_empty() {
                f.name.clone()
            } else {
                def.function_name.clone()
            },
            function_type: def.function_type,
            web_url: def.web_url.clone(),
            is_method: def.is_method,
            use_method_name: String::new(),
            definition_id: f.id.clone(),
            class_parameter_info: def.class_parameter_info.clone(),
            method_handle_metadata: methods,
            function_schema: def.function_schema.clone(),
            input_plane_url: None,
            input_plane_region: None,
            max_object_size_bytes: None,
            max_async_object_size_bytes: None,
            supported_input_formats: def.supported_input_formats.clone(),
            supported_output_formats: def.supported_output_formats.clone(),
            app_id: f.app_id.clone(),
            base_function_id: f.base_function_id.clone().unwrap_or_default(),
            ..Default::default()
        }
    }

    pub fn class_handle(&self, c: &ClassRec) -> pb::ClassHandleMetadata {
        let mut methods = Vec::new();
        let mut service = None;
        for (name, fid) in &c.methods {
            let Some(f) = self.function(fid) else {
                continue;
            };
            let meta = self.function_handle(&f);
            if name.is_empty() {
                service = Some((fid.clone(), meta));
                continue;
            }
            methods.push(pb::ClassMethod {
                function_name: name.clone(),
                function_id: fid.clone(),
                function_handle_metadata: Some(meta),
            });
        }
        let (class_function_id, class_function_metadata) = match service {
            Some((i, m)) => (i, Some(m)),
            None => (String::new(), None),
        };
        pb::ClassHandleMetadata {
            methods,
            class_function_id,
            class_function_metadata,
        }
    }

    // ---- registration -------------------------------------------------------

    pub fn precreate_function(
        &self,
        c: &Caller,
        q: &pb::FunctionPrecreateRequest,
    ) -> Result<FunctionRec, Status> {
        let app = self
            .db
            .read(|d| {
                d.apps
                    .get(&q.app_id)
                    .filter(|a| a.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("App {} not found", q.app_id)))?;
        if app.stopped {
            return Err(errors::conflict(format!("App {} is stopped", q.app_id)));
        }
        let rec = self.db.write(|d| {
            if !q.existing_function_id.is_empty()
                && let Some(f) = d.functions.get(&q.existing_function_id)
                && f.key_id == c.key_id()
            {
                return f.clone();
            }
            let f = FunctionRec {
                id: ids::new_id("fu"),
                key_id: c.key_id(),
                app_id: q.app_id.clone(),
                name: q.function_name.clone(),
                created: false,
                definition: String::new(),
                data: String::new(),
                created_at: now_s(),
                base_function_id: None,
                bound_params: None,
                min_containers: None,
                max_containers: None,
                buffer_containers: None,
                scaledown_window: None,
            };
            d.functions.insert(f.id.clone(), f.clone());
            f
        });
        Ok(rec)
    }

    pub fn create_function(
        &self,
        c: &Caller,
        q: &pb::FunctionCreateRequest,
    ) -> Result<FunctionRec, Status> {
        let def = q
            .function
            .clone()
            .or_else(|| {
                q.function_data
                    .as_ref()
                    .and_then(|d| d.ranked_functions.iter().min_by_key(|r| r.rank))
                    .and_then(|r| r.function.clone())
            })
            .ok_or_else(|| errors::invalid("a Function needs a definition"))?;
        self.check_function_definition(c, &def)?;
        let app = self
            .db
            .read(|d| {
                d.apps
                    .get(&q.app_id)
                    .filter(|a| a.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("App {} not found", q.app_id)))?;
        let rec = self.db.write(|d| {
            let existing = (!q.existing_function_id.is_empty())
                .then(|| d.functions.get(&q.existing_function_id).cloned())
                .flatten()
                .filter(|f| f.key_id == c.key_id());
            let mut f = existing.unwrap_or_else(|| FunctionRec {
                id: ids::new_id("fu"),
                key_id: c.key_id(),
                app_id: app.id.clone(),
                name: def.function_name.clone(),
                created: false,
                definition: String::new(),
                data: String::new(),
                created_at: now_s(),
                base_function_id: None,
                bound_params: None,
                min_containers: None,
                max_containers: None,
                buffer_containers: None,
                scaledown_window: None,
            });
            f.created = true;
            if f.name.is_empty() {
                f.name = def.function_name.clone();
            }
            f.definition = enc(&def);
            f.data = q.function_data.as_ref().map(enc).unwrap_or_default();
            d.functions.insert(f.id.clone(), f.clone());
            f
        });
        self.fn_changed();
        Ok(rec)
    }

    /// Refuses, with Modal's own error, what a Function asks for that Cua
    /// Spaces cannot give.
    fn check_function_definition(&self, c: &Caller, def: &pb::Function) -> Result<(), Status> {
        for sid in &def.secret_ids {
            let ok = self
                .db
                .read(|d| d.secrets.get(sid).is_some_and(|s| s.key_id == c.key_id()));
            if !ok {
                return Err(errors::not_found(format!("Secret {sid} not found")));
            }
        }
        if !self.db.read(|d| {
            d.images
                .get(&def.image_id)
                .is_some_and(|i| i.key_id == c.key_id())
        }) {
            return Err(errors::not_found(format!(
                "Image {} not found",
                def.image_id
            )));
        }
        if let Some(g) = def.resources.as_ref().and_then(|r| r.gpu_config.as_ref())
            && (g.count > 0 || !g.gpu_type.is_empty() || g.r#type != 0)
            && !def.experimental_options.contains_key("cua.gpu")
        {
            return Err(errors::unsupported("gpu", crate::modal::sandbox::GPU_HINT));
        }
        for k in def.experimental_options.keys() {
            if !k.starts_with("cua.") {
                return Err(errors::unsupported(
                    &format!("experimental option {k:?}"),
                    "it configures a Modal runtime feature Cua Spaces do not have (Cua options are experimental_options={\"cua.on\": ..., \"cua.kind\": ..., \"cua.gpu\": ..., \"cua.disk_gb\": ...})",
                ));
            }
        }
        if def.webhook_config.as_ref().is_some_and(|w| w.r#type != 0) {
            return Err(errors::unsupported(
                "web endpoints",
                "Modal serves web endpoints from its public edge; this gateway does not route HTTP to Function containers. Expose a port of a Sandbox with encrypted_ports instead",
            ));
        }
        if def.schedule.is_some() {
            return Err(errors::unsupported(
                "schedules",
                "scheduled Functions are run by Modal's scheduler for deployed Apps; use a Cua routine (`cua routine`) or call the Function from your own scheduler",
            ));
        }
        if def.experimental_group_size > 0 || def.experimental_fabric_size > 0 {
            return Err(errors::unsupported(
                "clustered Functions",
                "multi-node clusters are a Modal feature",
            ));
        }
        if def.is_server {
            return Err(errors::unsupported(
                "Server Functions",
                "modal.Server routes inbound HTTP to containers through Modal's edge",
            ));
        }
        if !def.nfs_mounts_empty() {
            return Err(errors::unsupported(
                "network_file_systems",
                "NetworkFileSystem is deprecated by Modal; use a Volume",
            ));
        }
        if def.proxy_id.is_some() {
            return Err(errors::unsupported(
                "proxy",
                "Modal Proxies are a Modal networking product",
            ));
        }
        Ok(())
    }

    // ---- calls ----------------------------------------------------------------

    pub fn new_call(&self, c: &Caller, f: &FunctionRec, invocation: i32) -> String {
        let id = ids::new_id("fc");
        let call = Call {
            id: id.clone(),
            function_id: f.id.clone(),
            key_id: c.key_id(),
            app_id: f.app_id.clone(),
            created_at: now_s(),
            invocation,
            inputs: Vec::new(),
            next_seq: 1,
            cancelled: false,
            data_out: BTreeMap::new(),
        };
        self.fns.lock().expect("fns").calls.insert(id.clone(), call);
        id
    }

    /// Adds inputs to a call and queues them.
    pub fn add_inputs(
        &self,
        c: &Caller,
        function_id: &str,
        call_id: &str,
        items: &[pb::FunctionPutInputsItem],
    ) -> Result<Vec<pb::FunctionPutInputsResponseItem>, Status> {
        let mut out = Vec::new();
        {
            let mut g = self.fns.lock().expect("fns");
            let fns = &mut *g;
            let call = fns
                .calls
                .get_mut(call_id)
                .filter(|x| x.key_id == c.key_id() && x.function_id == function_id)
                .ok_or_else(|| errors::not_found(format!("Function call {call_id} not found")))?;
            if call.cancelled {
                return Err(errors::conflict(format!(
                    "Function call {call_id} was cancelled"
                )));
            }
            for it in items {
                let input_id = ids::new_id("in");
                call.inputs.push(InputSlot {
                    idx: it.idx,
                    input_id: input_id.clone(),
                    input: it.input.clone().unwrap_or_default(),
                    retry_count: 0,
                    state: InputState::Pending,
                    output: None,
                    seq: 0,
                    started: None,
                    task: None,
                });
                fns.queues
                    .entry(function_id.to_string())
                    .or_default()
                    .pending
                    .push_back((call_id.to_string(), input_id.clone()));
                out.push(pb::FunctionPutInputsResponseItem {
                    idx: it.idx,
                    input_id: input_id.clone(),
                    input_jwt: self.signer.mint_with(
                        "input",
                        &input_id,
                        86_400,
                        serde_json::json!({"call": call_id, "fn": function_id}),
                    ),
                });
            }
        }
        self.fn_changed();
        Ok(out)
    }

    fn retry_policy(&self, f: &FunctionRec) -> Option<pb::FunctionRetryPolicy> {
        self.function_def(f).retry_policy.filter(|p| p.retries > 0)
    }

    /// The call a JWT (from `FunctionMap`) names.
    pub fn call_of_jwt(&self, jwt: &str) -> Result<String, Status> {
        let claims = self
            .signer
            .verify(jwt, "call")
            .map_err(|why| errors::denied(format!("invalid function call token: {why}")))?;
        Ok(claims["sub"].as_str().unwrap_or_default().to_string())
    }

    pub fn call_jwt(&self, call_id: &str) -> String {
        self.signer.mint("call", call_id, 86_400)
    }

    pub fn map_response(
        &self,
        f: &FunctionRec,
        call_id: String,
        pipelined: Vec<pb::FunctionPutInputsResponseItem>,
    ) -> pb::FunctionMapResponse {
        let policy = self.retry_policy(f);
        pb::FunctionMapResponse {
            function_call_jwt: self.call_jwt(&call_id),
            function_call_id: call_id,
            pipelined_inputs: pipelined,
            retry_policy: policy,
            sync_client_retries_enabled: false,
            max_inputs_outstanding: 1000,
        }
    }

    /// Waits for outputs: those in `[start, end]` by idx when given, else
    /// those newer than `last_entry_id`.
    #[allow(clippy::too_many_arguments)]
    pub async fn wait_outputs(
        &self,
        c: &Caller,
        call_id: &str,
        range: Option<(i32, i32)>,
        last_entry_id: &str,
        max_values: usize,
        timeout: Duration,
        clear_on_success: bool,
    ) -> Result<pb::FunctionGetOutputsResponse, Status> {
        let after: u64 = last_entry_id
            .split('-')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let deadline = tokio::time::Instant::now() + timeout;
        let mut rx = self.fn_events.subscribe();
        loop {
            {
                let mut g = self.fns.lock().expect("fns");
                let call = g
                    .calls
                    .get_mut(call_id)
                    .filter(|x| x.key_id == c.key_id())
                    .ok_or_else(|| {
                        errors::not_found(format!("Function call {call_id} not found"))
                    })?;
                let mut got: Vec<pb::FunctionGetOutputsItem> = Vec::new();
                let mut last = after;
                let mut ready: Vec<&InputSlot> = call
                    .inputs
                    .iter()
                    .filter(|s| s.output.is_some())
                    .filter(|s| match range {
                        Some((lo, hi)) => s.idx >= lo && s.idx <= hi,
                        None => s.seq > after,
                    })
                    .collect();
                ready.sort_by_key(|s| s.seq);
                for s in ready.into_iter().take(if max_values == 0 {
                    usize::MAX
                } else {
                    max_values
                }) {
                    if let Some(o) = &s.output {
                        got.push(o.clone());
                        last = last.max(s.seq);
                    }
                }
                let unfinished = call.inputs.iter().filter(|s| s.output.is_none()).count() as i32;
                if !got.is_empty() || tokio::time::Instant::now() >= deadline {
                    if clear_on_success {
                        let done: Vec<String> = got
                            .iter()
                            .filter(|o| o.result.as_ref().is_some_and(|r| r.status == 1))
                            .map(|o| o.input_id.clone())
                            .collect();
                        call.inputs.retain(|s| !done.contains(&s.input_id));
                    }
                    return Ok(pb::FunctionGetOutputsResponse {
                        idxs: got.iter().map(|o| o.idx).collect(),
                        outputs: got,
                        last_entry_id: format!("{last}-0"),
                        num_unfinished_inputs: unfinished,
                    });
                }
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                continue;
            }
        }
    }

    /// Records the output of an input; retries it when the policy says so.
    /// Returns the delay of a retry that was scheduled.
    pub fn finish_input(
        self: &Arc<Self>,
        call_id: &str,
        input_id: &str,
        mut item: pb::FunctionGetOutputsItem,
        retriable: bool,
    ) {
        let mut retry_after = None;
        {
            let mut g = self.fns.lock().expect("fns");
            let fns = &mut *g;
            let Some(call) = fns.calls.get_mut(call_id) else {
                return;
            };
            let cancelled = call.cancelled;
            let function_id = call.function_id.clone();
            let seq = call.next_seq;
            let Some(slot) = call.slot_mut(input_id) else {
                return;
            };
            if slot.state == InputState::Done {
                return;
            }
            let mut finished = false;
            let status = item.result.as_ref().map(|r| r.status).unwrap_or(0);
            let failed = !matches!(status, 1 | 3);
            let policy = self
                .function(&function_id)
                .and_then(|f| self.retry_policy(&f))
                .filter(|_| retriable && failed && !cancelled);
            if let Some(p) = policy
                && slot.retry_count < p.retries
            {
                slot.retry_count += 1;
                slot.state = InputState::Pending;
                slot.task = None;
                slot.started = None;
                let coeff = if p.backoff_coefficient > 0.0 {
                    p.backoff_coefficient
                } else {
                    1.0
                };
                let initial = if p.initial_delay_ms > 0 {
                    p.initial_delay_ms
                } else {
                    1000
                };
                let max = if p.max_delay_ms > 0 {
                    p.max_delay_ms
                } else {
                    60_000
                };
                let delay = (f64::from(initial)
                    * f64::from(coeff).powi(slot.retry_count as i32 - 1))
                .min(f64::from(max)) as u64;
                retry_after = Some((function_id, delay));
            } else {
                item.idx = slot.idx;
                item.input_id = slot.input_id.clone();
                item.retry_count = slot.retry_count;
                item.output_created_at = now_s();
                slot.seq = seq;
                slot.output = Some(item);
                slot.state = InputState::Done;
                finished = true;
            }
            if finished {
                call.next_seq += 1;
            }
        }
        if let Some((function_id, delay)) = retry_after {
            let me = self.clone();
            let (call_id, input_id) = (call_id.to_string(), input_id.to_string());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                {
                    let mut g = me.fns.lock().expect("fns");
                    let alive = g.calls.get(&call_id).is_some_and(|c| !c.cancelled);
                    if alive {
                        g.queues
                            .entry(function_id)
                            .or_default()
                            .pending
                            .push_back((call_id, input_id));
                    }
                }
                me.fn_changed();
            });
        }
        self.fn_changed();
    }

    /// A failure result in the shapes Modal's server produces.
    pub fn failure_item(status: i32, message: &str) -> pb::FunctionGetOutputsItem {
        pb::FunctionGetOutputsItem {
            result: Some(pb::GenericResult {
                status,
                exception: message.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub fn cancel_call(&self, c: &Caller, call_id: &str, terminate: bool) -> Result<(), Status> {
        {
            let mut g = self.fns.lock().expect("fns");
            let fns = &mut *g;
            let call = fns
                .calls
                .get_mut(call_id)
                .filter(|x| x.key_id == c.key_id())
                .ok_or_else(|| errors::not_found(format!("Function call {call_id} not found")))?;
            call.cancelled = true;
            let function_id = call.function_id.clone();
            let mut to_cancel: Vec<(Option<String>, String)> = Vec::new();
            for s in call.inputs.iter_mut() {
                if s.state == InputState::Done {
                    continue;
                }
                if s.state == InputState::Running {
                    to_cancel.push((s.task.clone(), s.input_id.clone()));
                }
                s.output = Some(Self::failure_item(3, "Function call cancelled"));
                if let Some(o) = s.output.as_mut() {
                    o.idx = s.idx;
                    o.input_id = s.input_id.clone();
                }
                s.seq = call.next_seq;
                call.next_seq += 1;
                s.state = InputState::Done;
            }
            if let Some(q) = fns.queues.get_mut(&function_id) {
                q.pending.retain(|(cid, _)| cid != call_id);
                for (task, input) in to_cancel {
                    if let Some(t) = task.and_then(|t| q.containers.get_mut(&t)) {
                        t.cancel.push(input);
                        t.terminate_on_cancel |= terminate;
                    }
                }
            }
        }
        self.fn_changed();
        Ok(())
    }

    // ---- classes ------------------------------------------------------------------

    pub fn create_class(&self, c: &Caller, q: &pb::ClassCreateRequest) -> Result<ClassRec, Status> {
        let app = self
            .db
            .read(|d| {
                d.apps
                    .get(&q.app_id)
                    .filter(|a| a.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("App {} not found", q.app_id)))?;
        let mut methods = Vec::new();
        for m in &q.methods {
            let f = self.function_of(c, &m.function_id)?;
            if f.app_id != app.id {
                return Err(errors::invalid(format!(
                    "Function {} belongs to another App",
                    m.function_id
                )));
            }
            methods.push((m.function_name.clone(), m.function_id.clone()));
        }
        let rec = self.db.write(|d| {
            let existing = (!q.existing_class_id.is_empty())
                .then(|| d.classes.get(&q.existing_class_id).cloned())
                .flatten()
                .filter(|x| x.key_id == c.key_id());
            let mut cl = existing.unwrap_or_else(|| ClassRec {
                id: ids::new_id("cs"),
                key_id: c.key_id(),
                app_id: app.id.clone(),
                name: String::new(),
                methods: Vec::new(),
                created_at: now_s(),
            });
            cl.methods = methods.clone();
            // The class is named after its service function.
            if let Some((_, fid)) = cl.methods.iter().find(|(n, _)| n.is_empty())
                && let Some(f) = d.functions.get(fid)
            {
                cl.name = f.name.trim_end_matches(".*").to_string();
            }
            d.classes.insert(cl.id.clone(), cl.clone());
            cl
        });
        Ok(rec)
    }

    /// A copy of a parametrised class's function with its parameters bound.
    pub fn bind_params(
        &self,
        c: &Caller,
        base_id: &str,
        params: &[u8],
        options: Option<&pb::FunctionOptions>,
    ) -> Result<FunctionRec, Status> {
        let base = self.function_of(c, base_id)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(params);
        let rec = self.db.write(|d| {
            if let Some(x) = d.functions.values().find(|x| {
                x.base_function_id.as_deref() == Some(base_id)
                    && x.bound_params.as_deref() == Some(&encoded)
                    && (options.is_none()
                        || x.definition
                            == options
                                .map(|o| {
                                    let mut def = self.function_def(&base);
                                    apply_options(&mut def, o);
                                    enc(&def)
                                })
                                .unwrap_or_default())
            }) {
                return x.clone();
            }
            let mut f = base.clone();
            if let Some(o) = options {
                let mut def = self.function_def(&base);
                apply_options(&mut def, o);
                f.definition = enc(&def);
            }
            f.id = ids::new_id("fu");
            f.base_function_id = Some(base_id.to_string());
            f.bound_params = Some(encoded.clone());
            f.created_at = now_s();
            d.functions.insert(f.id.clone(), f.clone());
            f
        });
        Ok(rec)
    }
}

trait NfsEmpty {
    fn nfs_mounts_empty(&self) -> bool;
}

impl NfsEmpty for pb::Function {
    fn nfs_mounts_empty(&self) -> bool {
        self.shared_volume_mounts.is_empty()
    }
}

/// `Function.with_options`: the per-instance overrides of a definition.
pub fn apply_options(def: &mut pb::Function, o: &pb::FunctionOptions) {
    if !o.secret_ids.is_empty() || o.replace_secret_ids {
        if o.replace_secret_ids {
            def.secret_ids.clear();
        }
        def.secret_ids.extend(o.secret_ids.iter().cloned());
    }
    if !o.mount_ids.is_empty() {
        def.mount_ids.extend(o.mount_ids.iter().cloned());
    }
    if o.resources.is_some() {
        def.resources = o.resources.clone();
    }
    if o.retry_policy.is_some() {
        def.retry_policy = o.retry_policy;
    }
    if let Some(v) = o.concurrency_limit {
        def.concurrency_limit = v;
    }
    if let Some(v) = o.timeout_secs {
        def.timeout_secs = v;
    }
    if let Some(v) = o.task_idle_timeout_secs {
        def.task_idle_timeout_secs = v;
    }
    if let Some(v) = o.warm_pool_size {
        def.warm_pool_size = v;
    }
    if o.replace_volume_mounts {
        def.volume_mounts.clear();
    }
    def.volume_mounts.extend(o.volume_mounts.iter().cloned());
    if o.replace_cloud_bucket_mounts {
        def.cloud_bucket_mounts.clear();
    }
    def.cloud_bucket_mounts
        .extend(o.cloud_bucket_mounts.iter().cloned());
    if let Some(v) = o.target_concurrent_inputs {
        def.target_concurrent_inputs = v;
    }
    if let Some(v) = o.max_concurrent_inputs {
        def.max_concurrent_inputs = v;
    }
    if let Some(v) = o.batch_max_size {
        def.batch_max_size = v;
    }
    if let Some(v) = o.batch_linger_ms {
        def.batch_linger_ms = v;
    }
}
