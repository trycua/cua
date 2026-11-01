// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What clients ask a coordinator, by method name and JSON params: the
//! same requests whether they come from this machine (the daemon's pool
//! socket) or from another of the account's devices through the relay's
//! mailbox ([`crate::pool`]).
//!
//! | Method | Params | Answer |
//! |---|---|---|
//! | `status` | | machines, queue, counters |
//! | `submit` | `{run, command, count, image, kind, runtime, cpus, memory_mb, timeout_secs, env, result_path, max_output_bytes, os, arch, on, accelerator, bundle, retries, concurrency, max_attempts}` or `{run, jobs: [{key, spec}], ...}` | `{run, ids, existing}` |
//! | `submit` (a gang) | the above with `gang: true` and `nodes` (`2`, `"1:2"`, `[1, 2]` or `{min, max}`) | `{run, gang: true, nodes: {min, max}, existing}` |
//! | `run` / `runs` | `{run}` / `{limit}` | summaries with `state`, `gang`, `cost_usd`, `machine_seconds` (`run`: and `members`) |
//! | `job_output` | `{job}` or `{run, rank, attempt}`, `from` | `{job, rank, attempt, host, state, from, next, size, data_b64, text, done}` |
//! | `run_result` | `{run}` (`index` for a run of jobs) | `{run, state, done, job, host, exit_code, error, result_b64}` |
//! | `jobs` | `{run, state, after, limit, output}` | jobs |
//! | `job` | `{id}` | one job with output |
//! | `cancel_run` / `cancel_job` | `{run}` / `{id}` | |
//! | `audit` | `{limit}` | the coordinator's audit, newest first |
//! | `durable_*` | see [`super::durable`] | |
//! | `session_start` | `{name, spec: {argv, env, image, kind, on, space, state_dir, label, ...}, max_attempts}` | a `SessionView` |
//! | `session_get` / `session_stop` | `{name}` | a `SessionView` |
//! | `session_list` | | `{sessions: [SessionView]}` |
//! | `session_delete` | `{name}` | `{name, deleted}` |
//! | `session_output` | `{name, from}` | `{name, epoch, job, host, state, from, next, size, data_b64, text, done}` |
//!
//! Sessions are described in [`super::sessions`].
//!
//! A submit is idempotent per job key (`<run>/<index>` by default), so a
//! client that retries after a timeout or a change of coordinator never
//! doubles a run.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::store::{self, NewJob};
use super::{CoordError, Coordinator};
use cua_cluster::model::JobSpec;

/// Who asks.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Caller {
    /// Account id (`local` on this machine).
    #[serde(default)]
    pub account: String,
    /// User.
    #[serde(default)]
    pub user: String,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Enrolled device.
    #[serde(default)]
    pub device: String,
}

impl Caller {
    /// A caller on this machine.
    pub fn local() -> Self {
        Self {
            account: "local".into(),
            user: whoami(),
            ..Default::default()
        }
    }

    /// For the audit: who and from which device.
    pub fn describe(&self) -> String {
        let who = self.email.clone().unwrap_or_else(|| self.user.clone());
        if self.device.is_empty() {
            format!("{who} (this machine)")
        } else {
            format!("{who} from device {}", self.device)
        }
    }
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "someone".into())
}

/// Most jobs one submit makes.
pub const MAX_SUBMIT: usize = 100_000;

#[derive(Deserialize)]
struct SubmitJob {
    #[serde(default)]
    key: Option<String>,
    spec: JobSpec,
}

/// `nodes` of a submit: `2`, `"1:2"`, `[1, 2]` or `{"min": 1, "max": 2}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Nodes {
    min: u32,
    max: u32,
}

impl<'de> Deserialize<'de> for Nodes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let v = Value::deserialize(d)?;
        let num = |v: &Value| {
            v.as_u64().map(|n| n as u32).ok_or_else(|| {
                D::Error::custom("nodes: a count, \"min:max\", [min, max] or {min, max}")
            })
        };
        let (min, max) = match &v {
            Value::Number(_) => {
                let n = num(&v)?;
                (n, n)
            }
            Value::String(s) => {
                let parse = |t: &str| {
                    t.trim()
                        .parse::<u32>()
                        .map_err(|_| D::Error::custom(format!("nodes {s:?}: use N or MIN:MAX")))
                };
                match s.split_once(':') {
                    Some((a, b)) => (parse(a)?, parse(b)?),
                    None => {
                        let n = parse(s)?;
                        (n, n)
                    }
                }
            }
            Value::Array(a) if a.len() == 2 => (num(&a[0])?, num(&a[1])?),
            Value::Object(o) => {
                let min = o.get("min").map(num).transpose()?.unwrap_or(1);
                let max = o.get("max").map(num).transpose()?.unwrap_or(min);
                (min, max)
            }
            _ => {
                return Err(D::Error::custom(
                    "nodes: a count, \"min:max\", [min, max] or {min, max}",
                ));
            }
        };
        Ok(Self { min, max })
    }
}

/// `retries`: a count, or the cua jobs runtime's `Retries` as an object
/// (`{"max_retries", "backoff_coefficient", "initial_delay", "max_delay"}`,
/// delays in seconds).
#[derive(Clone, Copy, Debug, Default)]
struct RetriesIn {
    max: u32,
    backoff: Option<super::gang::Backoff>,
}

impl<'de> Deserialize<'de> for RetriesIn {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let v = Value::deserialize(d)?;
        let bad = || {
            D::Error::custom(
                "retries: a count or {max_retries, backoff_coefficient, initial_delay, max_delay}",
            )
        };
        match &v {
            Value::Number(n) => Ok(Self {
                max: n.as_u64().ok_or_else(bad)?.min(u64::from(u32::MAX)) as u32,
                backoff: None,
            }),
            Value::Object(o) => {
                let num = |k: &str| o.get(k).map(|x| x.as_f64().ok_or_else(bad)).transpose();
                let max = num("max_retries")?.unwrap_or(3.0);
                if max.is_nan() || max < 0.0 {
                    return Err(bad());
                }
                let d = super::gang::Backoff::default();
                let secs_ms = |s: Option<f64>, dflt: u64| -> Result<u64, D::Error> {
                    match s {
                        None => Ok(dflt),
                        Some(x) if x.is_finite() && x >= 0.0 => Ok((x * 1000.0) as u64),
                        Some(_) => Err(bad()),
                    }
                };
                let coefficient = num("backoff_coefficient")?.unwrap_or(d.coefficient);
                if !(coefficient.is_finite() && coefficient >= 1.0) {
                    return Err(D::Error::custom("retries.backoff_coefficient must be >= 1"));
                }
                Ok(Self {
                    max: max as u32,
                    backoff: Some(super::gang::Backoff {
                        initial_ms: secs_ms(num("initial_delay")?, d.initial_ms)?,
                        coefficient,
                        max_ms: secs_ms(num("max_delay")?, d.max_ms)?,
                    }),
                })
            }
            _ => Err(bad()),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Submit {
    run: Option<String>,
    concurrency: u32,
    max_attempts: u32,
    /// Hosts of a gang run (`gang`): at least min, at most max.
    nodes: Option<Nodes>,
    /// Run the command as a gang: one member per host, ranks 0..n-1, all
    /// together ([`super::gang`]).
    gang: bool,
    /// Times a failed attempt runs again (with a backoff); for a gang, the
    /// code failures of the whole gang it retries after. Default: 0 for
    /// jobs, 3 for gangs. A count, or `{max_retries, backoff_coefficient,
    /// initial_delay, max_delay}` (seconds; a gang's backoff).
    retries: Option<RetriesIn>,
    /// The cua jobs runtime's `Retries` (`{max_retries, backoff_coefficient,
    /// initial_delay, max_delay}`, seconds), as `cua run` sends it next to
    /// `retries`: its backoff applies (and its count when `retries` is not
    /// given).
    retry: Option<RetriesIn>,
    /// A gang's budget for lost machines, members that never started and
    /// start-barrier timeouts (default 10; it refills after a member ran
    /// 10 minutes).
    infra_retries: Option<u32>,
    /// A gang's members wait at a start barrier at most this long (0, the
    /// default: no barrier; the cua jobs runtime reports ready).
    barrier_timeout_secs: u32,
    /// A gang member that reported progress and then none for this long
    /// stalls its attempt (default 600; 0: off).
    stall_timeout_secs: Option<u32>,
    /// `gpu` (any), `apple`, `nvidia`, or empty.
    accelerator: String,
    /// A native job's files: a gzip tar (base64, at most 1 MiB).
    bundle: String,
    /// Explicit jobs (the SDK and the benchmark).
    jobs: Vec<SubmitJob>,
    /// Or `count` copies of one command.
    command: Vec<String>,
    count: u32,
    image: String,
    kind: String,
    runtime: String,
    cpus: f64,
    memory_mb: u64,
    timeout_secs: u32,
    env: BTreeMap<String, String>,
    result_path: String,
    max_output_bytes: u32,
    os: String,
    arch: String,
    /// `auto` (or empty), one machine (or `modal`), or `cloud:<provider>`.
    on: String,
    /// Host services every member reaches (`compat:e2b`, ...): see
    /// `JobSpec::services`.
    services: Vec<String>,
}

fn invalid(m: impl Into<String>) -> CoordError {
    CoordError::Invalid(m.into())
}

fn parse<T: for<'de> Deserialize<'de> + Default>(params: Value) -> Result<T, CoordError> {
    if params.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(params).map_err(|e| invalid(format!("bad params: {e}")))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JobsQuery {
    run: String,
    state: String,
    after: i64,
    limit: Option<usize>,
    output: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Id {
    id: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Run {
    run: String,
    limit: Option<usize>,
}

/// `job_output` / `run_result`: a job by id, or a run's member by rank
/// (a gang's newest attempt unless `attempt`; a run of jobs: its job
/// `index`/`rank`).
#[derive(Deserialize, Default)]
#[serde(default)]
struct Output {
    run: String,
    job: i64,
    rank: Option<u32>,
    index: Option<u32>,
    attempt: Option<u32>,
    from: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Durable {
    name: String,
    spec: super::durable::DurableSpec,
    holder: String,
    token: u64,
    ttl_secs: Option<u64>,
    /// `durable_create`: place it before answering (default), or answer at
    /// once and place it in the background (`false`: a client that polls
    /// `durable_get`, so a slow first start never outlasts a request).
    wait: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SessionReq {
    name: String,
    spec: super::sessions::SessionSpec,
    max_attempts: u32,
    from: u64,
}

/// The jobs a submit makes, with their keys.
fn jobs_of(s: &mut Submit, run: &str, max_attempts: u32) -> Result<Vec<NewJob>, CoordError> {
    let on = cua_cluster::model::normalize_auto(&s.on);
    let retries = s.retries.map_or(0, |r| r.max).min(100);
    // Defaults applied; placement words kept (they never reach a host).
    let place = |spec: JobSpec| -> Result<JobSpec, CoordError> {
        let mut out = JobSpec::from_pb(&spec.to_pb());
        out.os = spec.os.trim().to_ascii_lowercase();
        out.arch = if spec.arch.trim().is_empty() {
            String::new()
        } else {
            cua_cluster::model::normalize_arch(&spec.arch)
        };
        out.on = match cua_cluster::model::normalize_auto(&spec.on) {
            o if o.is_empty() => on.clone(),
            o => o,
        };
        out.accelerator =
            cua_cluster::model::normalize_accelerator(&spec.accelerator).map_err(invalid)?;
        out.retries = if spec.retries > 0 {
            spec.retries.min(100)
        } else {
            retries
        };
        // A native job's run keeps its data on each host under the run.
        if out.run_id.is_empty() {
            out.run_id = run.to_string();
        }
        check_kind_and_bundle(&out)?;
        Ok(out)
    };
    if !s.jobs.is_empty() {
        if s.jobs.len() > MAX_SUBMIT {
            return Err(invalid(format!("at most {MAX_SUBMIT} jobs per submit")));
        }
        return std::mem::take(&mut s.jobs)
            .into_iter()
            .enumerate()
            .map(|(i, j)| {
                Ok(NewJob {
                    key: j
                        .key
                        .filter(|k| !k.is_empty())
                        .unwrap_or_else(|| format!("{run}/{i}")),
                    max_attempts: max_attempts.max(j.spec.retries.max(retries) + 1),
                    spec: place(j.spec)?,
                })
            })
            .collect::<Result<_, CoordError>>();
    }
    if s.command.is_empty() || s.command[0].trim().is_empty() {
        return Err(invalid("a run needs a command"));
    }
    let count = s.count.max(1) as usize;
    if count > MAX_SUBMIT {
        return Err(invalid(format!("at most {MAX_SUBMIT} jobs per run")));
    }
    let spec = place(template_of(s)?)?;
    let max_attempts = max_attempts.max(spec.retries + 1);
    Ok((0..count)
        .map(|i| NewJob {
            key: format!("{run}/{i}"),
            spec: spec.clone(),
            max_attempts,
        })
        .collect())
}

/// The one command a submit runs (`command` and its words).
fn template_of(s: &Submit) -> Result<JobSpec, CoordError> {
    use base64::Engine as _;
    let bundle = if s.bundle.trim().is_empty() {
        vec![]
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(s.bundle.trim())
            .map_err(|e| invalid(format!("bundle: not base64 ({e})")))?
    };
    Ok(JobSpec {
        job_id: String::new(),
        attempt: 0,
        image: s.image.clone(),
        kind: cua_cluster::model::normalize_auto(&s.kind),
        runtime: s.runtime.clone(),
        millicpus: if s.cpus > 0.0 {
            (s.cpus * 1000.0).round() as u32
        } else {
            0
        },
        memory_mb: s.memory_mb,
        argv: s.command.clone(),
        env: s.env.clone(),
        timeout_secs: s.timeout_secs,
        result_path: s.result_path.clone(),
        max_output_bytes: s.max_output_bytes,
        os: s.os.clone(),
        arch: s.arch.clone(),
        on: String::new(),
        bundle,
        run_id: String::new(),
        accelerator: s.accelerator.clone(),
        retries: 0,
        services: s.services.clone(),
    })
}

/// A job's bundle is small, and the removed native kind is refused.
fn check_kind_and_bundle(spec: &JobSpec) -> Result<(), CoordError> {
    if spec.kind == cua_cluster::model::KIND_PROCESS {
        return Err(invalid(cua_cluster::model::process_refused()));
    }
    if spec.bundle.len() > cua_cluster::model::MAX_BUNDLE {
        return Err(invalid(format!(
            "the bundle is {} bytes; at most {} (1 MiB): leave data out of it",
            spec.bundle.len(),
            cua_cluster::model::MAX_BUNDLE
        )));
    }
    Ok(())
}

/// A gang submit: one command on `nodes` hosts at once.
fn gang_of(s: &mut Submit, run: &str) -> Result<super::gang::GangSpec, CoordError> {
    if !s.jobs.is_empty() || s.count > 1 {
        return Err(invalid(
            "a gang runs one command (`command`) on `nodes` computers; not `jobs` or `count`",
        ));
    }
    if s.command.is_empty() || s.command[0].trim().is_empty() {
        return Err(invalid("a run needs a command"));
    }
    let nodes = s.nodes.unwrap_or(Nodes { min: 1, max: 1 });
    let retries = s.retries.or(s.retry).map_or(3, |r| r.max).min(100);
    let defaults = super::gang::GangPolicy::default();
    let policy = super::gang::GangPolicy {
        infra_retries: s.infra_retries.unwrap_or(defaults.infra_retries).min(1000),
        backoff: s
            .retry
            .and_then(|r| r.backoff)
            .or_else(|| s.retries.and_then(|r| r.backoff))
            .unwrap_or(defaults.backoff),
        barrier_timeout_secs: s.barrier_timeout_secs.min(24 * 3600),
        stall_timeout_secs: s
            .stall_timeout_secs
            .unwrap_or(defaults.stall_timeout_secs)
            .min(7 * 24 * 3600),
    };
    let mut template = template_of(s)?;
    template.on = cua_cluster::model::normalize_auto(&s.on);
    template.os = template.os.trim().to_ascii_lowercase();
    if !template.arch.trim().is_empty() {
        template.arch = cua_cluster::model::normalize_arch(&template.arch);
    }
    template.accelerator =
        cua_cluster::model::normalize_accelerator(&template.accelerator).map_err(invalid)?;
    // Defaults applied, placement words kept.
    let mut spec = JobSpec::from_pb(&template.to_pb());
    spec.on = template.on;
    spec.os = template.os;
    spec.arch = template.arch;
    spec.accelerator = template.accelerator;
    spec.run_id = run.to_string();
    check_kind_and_bundle(&spec)?;
    Ok(super::gang::GangSpec {
        min: nodes.min,
        max: nodes.max,
        retries,
        spec,
        policy,
    })
}

/// The machine `on` names, as the coordinator calls it: its name, its
/// relay id, or (any case, punctuation aside) words of its name; `auto` and
/// empty are any machine.
fn resolve_machine(coord: &Coordinator, on: &str) -> Result<String, CoordError> {
    let on = on.trim();
    if on.is_empty() || on.eq_ignore_ascii_case("auto") {
        return Ok(String::new());
    }
    // A cloud, not a machine: any host that cloud supplied.
    if let Some(p) = cua_cluster::placement::cloud_pin(&on.to_ascii_lowercase()) {
        return Ok(format!("cloud:{p}"));
    }
    let rows = coord.node_rows();
    if let Some(r) = rows
        .iter()
        .find(|r| r.name == on || (!r.relay_machine.is_empty() && r.relay_machine == on))
    {
        return Ok(r.name.clone());
    }
    let want = super::sanitize_name(on);
    let hits: Vec<&String> = rows
        .iter()
        .map(|r| &r.name)
        .filter(|n| **n == want || n.contains(&want))
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CoordError::NotFound(format!(
            "no machine called {on:?} among yours that provide Spaces ({})",
            rows.iter()
                .filter(|r| r.via != "provider")
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
        many => Err(CoordError::Invalid(format!(
            "{on:?} matches more than one machine: {}",
            many.iter()
                .map(|n| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Answers one request.
pub async fn handle(
    coord: &Coordinator,
    method: &str,
    params: Value,
    caller: &Caller,
) -> Result<Value, CoordError> {
    match method {
        "status" => {
            let counts = coord
                .store()
                .call(|c| store::counts(c))
                .await
                .unwrap_or_default();
            let (queued, inflight) = coord.depth();
            Ok(json!({
                "pool": coord.cluster_id(),
                "machines": coord.nodes(),
                "jobs": counts,
                "queued_in_memory": queued,
                "in_flight": inflight,
                "metrics": coord.metrics(),
                "database_error": coord.db_error(),
            }))
        }
        "pending" => {
            let min_age = params["min_age_secs"].as_u64().unwrap_or(20);
            Ok(json!({ "pending": coord.pending_demand(std::time::Duration::from_secs(min_age)) }))
        }
        "submit" => {
            let mut s: Submit = parse(params)?;
            let run = s
                .run
                .clone()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| format!("r-{:08x}", rand::random::<u32>()));
            if run.len() > 100 || run.contains(char::is_whitespace) {
                return Err(invalid("a run id is up to 100 characters, no spaces"));
            }
            let max_attempts = if s.max_attempts == 0 {
                coord.config().default_max_attempts
            } else {
                s.max_attempts.min(100)
            };
            s.on = resolve_machine(coord, &s.on)?;
            if s.gang || s.nodes.is_some_and(|n| n.max > 1) {
                let g = gang_of(&mut s, &run)?;
                let (min, max) = (g.min, g.max);
                let argv: String = g.spec.argv.join(" ").chars().take(120).collect();
                let on = g.spec.on.clone();
                let fresh = coord.submit_gang(&run, g).await?;
                if fresh {
                    coord
                        .audit(
                            "submit",
                            &caller.describe(),
                            &run,
                            &format!(
                                "a gang of {min} to {max} on {}: {argv}",
                                if on.is_empty() { "auto" } else { &on }
                            ),
                        )
                        .await;
                }
                return Ok(json!({
                    "run": run,
                    "gang": true,
                    "nodes": {"min": min, "max": max},
                    "jobs": 0,
                    "ids": [],
                    "existing": u32::from(!fresh),
                }));
            }
            let jobs = jobs_of(&mut s, &run, max_attempts)?;
            let n = jobs.len();
            let argv = jobs
                .first()
                .map(|j| j.spec.argv.join(" ").chars().take(120).collect::<String>())
                .unwrap_or_default();
            let on = jobs.first().map(|j| j.spec.on.clone()).unwrap_or_default();
            let ids = coord.submit(&run, s.concurrency, jobs).await?;
            let fresh = ids.iter().filter(|(_, e)| !*e).count();
            if fresh > 0 {
                coord
                    .audit(
                        "submit",
                        &caller.describe(),
                        &run,
                        &format!(
                            "{fresh} of {n} jobs on {}: {argv}",
                            if on.is_empty() { "auto" } else { &on }
                        ),
                    )
                    .await;
            }
            Ok(json!({
                "run": run,
                "jobs": n,
                "ids": ids.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                "existing": ids.iter().filter(|(_, e)| *e).count(),
            }))
        }
        "jobs" => {
            let q: JobsQuery = parse(params)?;
            let limit = q.limit.unwrap_or(1000).min(10_000);
            let jobs = coord
                .store()
                .call(move |c| store::jobs(c, &q.run, &q.state, q.after, limit, q.output))
                .await
                .map_err(CoordError::Store)?;
            Ok(json!({ "jobs": jobs }))
        }
        "job" => {
            let Id { id } = parse(params)?;
            match coord
                .store()
                .call(move |c| store::job(c, id))
                .await
                .map_err(CoordError::Store)?
            {
                Some(j) => Ok(serde_json::to_value(j).unwrap_or_default()),
                None => Err(CoordError::NotFound(format!("no job {id}"))),
            }
        }
        "cancel_job" => {
            let Id { id } = parse(params)?;
            let state = coord.cancel_job(id).await?;
            coord
                .audit("cancel", &caller.describe(), &format!("job {id}"), &state)
                .await;
            Ok(json!({ "id": id, "state": state }))
        }
        "runs" => {
            let r: Run = parse(params)?;
            let limit = r.limit.unwrap_or(20).min(1000);
            let ids = coord
                .store()
                .call(move |c| store::runs(c, limit))
                .await
                .map_err(CoordError::Store)?;
            let mut runs = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(v) = run_view(coord, &id, false).await? {
                    runs.push(v);
                }
            }
            Ok(json!({ "runs": runs }))
        }
        "run" => {
            let r: Run = parse(params)?;
            run_view(coord, &r.run, true)
                .await?
                .ok_or_else(|| CoordError::NotFound(format!("no run {}", r.run)))
        }
        "job_output" => {
            let q: Output = parse(params)?;
            job_output(coord, q).await
        }
        "run_result" => {
            let q: Output = parse(params)?;
            run_result(coord, q).await
        }
        "cancel_run" => {
            let r: Run = parse(params)?;
            let n = coord.cancel_run(&r.run).await?;
            coord
                .audit("cancel", &caller.describe(), &r.run, &format!("{n} jobs"))
                .await;
            Ok(json!({ "run": r.run, "canceled": n }))
        }
        "audit" => {
            let r: Run = parse(params)?;
            let a = coord.audit_log(r.limit.unwrap_or(100).min(10_000)).await;
            Ok(json!({ "events": a.recent, "error": a.error }))
        }
        "durable_create" => {
            let d: Durable = parse(params)?;
            coord
                .audit("durable", &caller.describe(), &d.name, "create")
                .await;
            Ok(serde_json::to_value(if d.wait == Some(false) {
                coord.durable_create_later(&d.name, d.spec).await?
            } else {
                coord.durable_create(&d.name, d.spec).await?
            })
            .unwrap_or_default())
        }
        "durable_list" => Ok(json!({ "spaces": coord.durable_list().await? })),
        "durable_get" => {
            let d: Durable = parse(params)?;
            Ok(serde_json::to_value(coord.durable_get(&d.name).await?).unwrap_or_default())
        }
        "durable_delete" => {
            let d: Durable = parse(params)?;
            coord.durable_delete(&d.name).await?;
            coord
                .audit("durable", &caller.describe(), &d.name, "delete")
                .await;
            Ok(json!({ "name": d.name, "deleted": true }))
        }
        "durable_acquire" | "durable_renew" => {
            let d: Durable = parse(params)?;
            let ttl =
                std::time::Duration::from_secs(d.ttl_secs.unwrap_or(600).clamp(5, 7 * 24 * 3600));
            let c = if method == "durable_acquire" {
                let holder = if d.holder.trim().is_empty() {
                    caller.describe()
                } else {
                    d.holder.clone()
                };
                coord.durable_acquire(&d.name, &holder, ttl).await?
            } else {
                coord.durable_renew(&d.name, d.token, ttl).await?
            };
            Ok(serde_json::to_value(c).unwrap_or_default())
        }
        "durable_release" => {
            let d: Durable = parse(params)?;
            coord.durable_release(&d.name, d.token).await?;
            Ok(json!({ "name": d.name, "released": true }))
        }
        "durable_connection" => {
            let d: Durable = parse(params)?;
            Ok(
                serde_json::to_value(coord.durable_connection(&d.name, d.token).await?)
                    .unwrap_or_default(),
            )
        }
        "session_start" => {
            let mut q: SessionReq = parse(params)?;
            q.spec.on = resolve_machine(coord, &q.spec.on)?;
            let v = coord.session_start(&q.name, q.spec, q.max_attempts).await?;
            Ok(serde_json::to_value(v).unwrap_or_default())
        }
        "session_get" => {
            let q: SessionReq = parse(params)?;
            Ok(serde_json::to_value(coord.session_get(&q.name).await?).unwrap_or_default())
        }
        "session_list" => Ok(json!({ "sessions": coord.session_list().await? })),
        "session_stop" => {
            let q: SessionReq = parse(params)?;
            let v = coord.session_stop(&q.name).await?;
            Ok(serde_json::to_value(v).unwrap_or_default())
        }
        "session_delete" => {
            let q: SessionReq = parse(params)?;
            coord.session_delete(&q.name).await?;
            Ok(json!({ "name": q.name, "deleted": true }))
        }
        "session_output" => {
            let q: SessionReq = parse(params)?;
            coord.session_output(&q.name, q.from).await
        }
        other => Err(CoordError::NotFound(format!("no request {other:?}"))),
    }
}

/// The HTTP-like status of an error, for a mailbox or socket answer.
pub fn status_of(e: &CoordError) -> u16 {
    match e {
        CoordError::Invalid(_) => 400,
        CoordError::NotFound(_) => 404,
        CoordError::Denied(_) => 403,
        CoordError::Unavailable(_) => 503,
        CoordError::Store(_) => 500,
        CoordError::Conflict(_) => 409,
    }
}

/// [`handle`] as a (status, body) answer.
pub async fn answer(
    coord: &Coordinator,
    method: &str,
    params: Value,
    caller: &Caller,
) -> (u16, Value) {
    match handle(coord, method, params, caller).await {
        Ok(v) => (200, v),
        Err(e) => (status_of(&e), json!({ "error": e.to_string() })),
    }
}

/// The job `q` names: (job id, rank, gang attempt).
async fn resolve_job(coord: &Coordinator, q: &Output) -> Result<(i64, u32, u32), CoordError> {
    if q.job > 0 {
        let id = q.job;
        let found = coord
            .store()
            .call(move |c| {
                let m = c
                    .query_row(
                        "SELECT rank, attempt FROM gang_members WHERE job=?1",
                        [id],
                        |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32)),
                    )
                    .ok();
                Ok((store::job(c, id)?.is_some(), m))
            })
            .await
            .map_err(CoordError::Store)?;
        return match found {
            (false, _) => Err(CoordError::NotFound(format!("no job {id}"))),
            (true, Some((rank, attempt))) => Ok((id, rank, attempt)),
            (true, None) => Ok((id, 0, 0)),
        };
    }
    if q.run.is_empty() {
        return Err(invalid("name a job (`job`) or a run (`run`, with `rank`)"));
    }
    let run = q.run.clone();
    let rank = q.rank.or(q.index).unwrap_or(0);
    let attempt = q.attempt;
    let r = coord
        .store()
        .call(move |c| {
            if let Some(g) = super::gang::gang(c, &run)? {
                let a = attempt.unwrap_or(g.attempt);
                let m = super::gang::members(c, &run, Some(a), 0)?;
                return Ok(m.iter().find(|m| m.rank == rank).map(|m| (m.job, rank, a)));
            }
            let key = format!("{run}/{rank}");
            Ok(
                c.query_row("SELECT id FROM jobs WHERE key=?1", [&key], |r| {
                    r.get::<_, i64>(0)
                })
                .ok()
                .map(|id| (id, rank, 0)),
            )
        })
        .await
        .map_err(CoordError::Store)?;
    r.ok_or_else(|| {
        CoordError::NotFound(format!(
            "run {} has no rank {rank}{} (yet)",
            q.run,
            attempt
                .map(|a| format!(" in attempt {a}"))
                .unwrap_or_default()
        ))
    })
}

/// `job_output`: a job's live output from byte `from`, read from its host.
async fn job_output(coord: &Coordinator, q: Output) -> Result<Value, CoordError> {
    use base64::Engine as _;
    let (id, rank, attempt) = resolve_job(coord, &q).await?;
    let (host, r) = coord.job_output(id, q.from).await?;
    let job = r.job.unwrap_or_default();
    let next = q.from + r.output_chunk.len() as u64;
    let ended =
        cua_cluster::model::HostJobState::parse(&job.state).is_some_and(|s| s.is_terminal());
    Ok(json!({
        "run": q.run,
        "job": id,
        "rank": rank,
        "attempt": attempt,
        "host": host,
        "state": job.state,
        "from": q.from,
        "next": next,
        "size": r.output_size,
        "data_b64": base64::engine::general_purpose::STANDARD.encode(&r.output_chunk),
        "text": String::from_utf8_lossy(&r.output_chunk),
        "done": ended && next >= r.output_size,
    }))
}

/// `run_result`: the run's result: rank 0's result file of the attempt
/// that succeeded (a gang), or job `index` of a run of jobs.
async fn run_result(coord: &Coordinator, q: Output) -> Result<Value, CoordError> {
    let run = q.run.clone();
    if run.is_empty() {
        return Err(invalid("name the run (`run`)"));
    }
    let r = run.clone();
    let gang = coord
        .store()
        .call(move |c| super::gang::gang(c, &r))
        .await
        .map_err(CoordError::Store)?;
    let (id, state, error) = match gang {
        Some(g) => {
            if !super::gang::gstate::is_terminal(&g.state) {
                return Ok(json!({
                    "run": run, "state": g.state, "done": false, "attempt": g.attempt,
                }));
            }
            if g.result_job == 0 {
                return Ok(json!({
                    "run": run, "state": g.state, "done": true, "attempt": g.attempt,
                    "error": g.error,
                }));
            }
            (g.result_job, g.state, g.error)
        }
        None => {
            let (id, _, _) = resolve_job(coord, &q).await?;
            (id, String::new(), String::new())
        }
    };
    let job = coord
        .store()
        .call(move |c| store::job(c, id))
        .await
        .map_err(CoordError::Store)?
        .ok_or_else(|| CoordError::NotFound(format!("no job {id}")))?;
    let done = store::state::is_terminal(&job.state);
    Ok(json!({
        "run": run,
        "state": if state.is_empty() { job.state.clone() } else { state },
        "done": done,
        "job": id,
        "host": job.node,
        "exit_code": job.exit_code,
        "error": if error.is_empty() { job.error.clone() } else { error },
        "result_b64": job.result_b64,
    }))
}

/// One job of a run of jobs: id, key, host, attempt, state, started,
/// finished, exit code, error.
type JobLine = (
    i64,
    String,
    String,
    u32,
    String,
    i64,
    i64,
    Option<i32>,
    String,
);

/// A run as `run` (with every member) and `runs` show it: its jobs by
/// state, its gang, and a cost estimate from its hosts' prices.
async fn run_view(
    coord: &Coordinator,
    run: &str,
    with_members: bool,
) -> Result<Option<Value>, CoordError> {
    let r = run.to_string();
    let found = coord
        .store()
        .call(move |c| {
            let Some(sum) = store::run_summary(c, &r)? else {
                return Ok(None);
            };
            let gang = super::gang::gang(c, &r)?;
            let now = cua_cluster::model::now_ms();
            let members = if gang.is_some() {
                super::gang::members(c, &r, None, now)?
            } else {
                vec![]
            };
            // A run of jobs: each job's host and time.
            let mut st = c
                .prepare_cached(
                    "SELECT id, key, node, attempt, state, started_ms, finished_ms, exit_code, error
                     FROM jobs WHERE run=?1 ORDER BY id LIMIT 10000",
                )
                .map_err(|e| e.to_string())?;
            let jobs: Vec<JobLine> = if gang.is_some() {
                vec![]
            } else {
                st.query_map([&r], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, i64>(3)? as u32,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
            };
            Ok(Some((sum, gang, members, jobs, now)))
        })
        .await
        .map_err(CoordError::Store)?;
    let Some((sum, gang, members, jobs, now)) = found else {
        return Ok(None);
    };
    let mut v = serde_json::to_value(&sum).unwrap_or_default();
    let (mut cost, mut seconds) = (0.0f64, 0.0f64);
    let mut member_views: Vec<Value> = Vec::new();
    match &gang {
        Some(g) => {
            for m in &members {
                cost += m.cost_usd;
                seconds += m.seconds;
            }
            if with_members {
                member_views = members
                    .iter()
                    .map(|m| serde_json::to_value(m).unwrap_or_default())
                    .collect();
            }
            v["state"] = json!(g.state);
            v["gang"] = json!({
                "min": g.min,
                "max": g.max,
                "attempt": g.attempt,
                "size": g.size,
                "failures": g.failures,
                "retries": g.retries,
                "infra_failures": g.infra_failures,
                "infra_used": g.infra_used,
                "infra_retries": g.policy.infra_retries,
                "barrier_timeout_secs": g.policy.barrier_timeout_secs,
                "stall_timeout_secs": g.policy.stall_timeout_secs,
                "summary": super::gang::summary(g),
                "error": g.error,
                "hosts": g.hosts,
                "waiting": coord.gang_waiting(run),
                "result_job": g.result_job,
                "submitted_ms": g.submitted_ms,
                "finished_ms": g.finished_ms,
            });
        }
        None => {
            for (id, key, node, attempt, state, started, finished, exit_code, error) in &jobs {
                let usd = if node.is_empty() {
                    0.0
                } else {
                    coord.node_usd_per_hour(node)
                };
                let secs = if *started > 0 {
                    ((if *finished > 0 { *finished } else { now }) - started).max(0) as f64 / 1000.0
                } else {
                    0.0
                };
                cost += secs / 3600.0 * usd;
                seconds += secs;
                if with_members {
                    let rank = key
                        .rsplit_once('/')
                        .and_then(|(_, i)| i.parse::<u32>().ok())
                        .unwrap_or(0);
                    member_views.push(json!({
                        "job": id, "rank": rank, "attempt": attempt, "host": node,
                        "state": state, "started_ms": started, "finished_ms": finished,
                        "exit_code": exit_code, "error": error, "usd_per_hour": usd,
                        "seconds": secs, "cost_usd": secs / 3600.0 * usd,
                    }));
                }
            }
            v["state"] = json!(jobs_state(&sum));
        }
    }
    v["machine_seconds"] = json!(seconds);
    v["cost_usd"] = json!(cost);
    if with_members {
        v["members"] = json!(member_views);
    }
    Ok(Some(v))
}

/// A run of jobs' state from its counts.
fn jobs_state(sum: &store::RunSummary) -> &'static str {
    let n = |k: &str| sum.counts.get(k).copied().unwrap_or(0);
    if !sum.done {
        if n(store::state::QUEUED) == sum.total {
            "queued"
        } else {
            "running"
        }
    } else if n(store::state::SUCCEEDED) == sum.total {
        "succeeded"
    } else if n(store::state::FAILED) + n(store::state::ERROR) == 0 {
        "canceled"
    } else {
        "failed"
    }
}
