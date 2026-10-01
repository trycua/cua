//! The client: decides, samples, validates, logs locally, batches and
//! sends in the background. Nothing here can block or fail a product call:
//! [`Telemetry::capture`] returns immediately and never errors, sends run
//! on one background thread with a 3 s budget, and a failed batch goes to a
//! small on-disk spool that is retried later.

use rand::Rng;
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{self, Decision};
use crate::events::Event;
use crate::identity::{self, Identity};
use crate::schema;
use crate::sink::{self, HttpSink, SendError, Sink};
use crate::{fsutil, notice};

/// An environment lookup the client owns.
type EnvFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Events per batch.
pub const BATCH_SIZE: usize = 50;
/// A batch goes out at least this often while events are queued.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
/// Events queued in memory at most (older ones are dropped).
pub const MAX_QUEUE: usize = 500;
/// Events kept in the offline spool at most.
pub const MAX_SPOOL: usize = 500;
/// Spooled events older than this are dropped, not sent.
pub const SPOOL_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Events per process per hour at most.
pub const MAX_PER_HOUR: usize = 600;
/// Entries kept in the local `show-last` log.
pub const LAST_LOG_LEN: usize = 100;

const SPOOL_FILE: &str = "spool.jsonl";
const LAST_FILE: &str = "last_events.jsonl";
const FIRST_RUN_FILE: &str = "first_run_recorded";
const FUNNEL_DIR: &str = "funnel";
/// The UTC day (days since the epoch) `cua_app_active` was last sent.
const ACTIVE_DAY_FILE: &str = "active_day";
/// The Spaces app's experiments that are on (`cua_volume+sharing`, `none`).
const EXPERIMENTS_FILE: &str = "experiments_on";

/// What happened to one captured event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// Queued for sending.
    Queued,
    /// Telemetry is off.
    Disabled,
    /// The first-run notice has not been shown on this machine yet;
    /// nothing is sent until it has been.
    NoticePending,
    /// Dropped by sampling.
    Sampled,
    /// Over the per-process rate limit.
    RateLimited,
    /// Refused by the schema (a bug; tests fail on it).
    Invalid(String),
    /// A once-per-install event that was already recorded.
    AlreadyRecorded,
}

/// How the first-run notice is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeMode {
    /// Print it to stderr once (CLI, SDKs).
    Stderr,
    /// The host shows it in its own UI and calls
    /// [`Telemetry::acknowledge_notice`] (the Spaces app).
    External,
}

struct Queue {
    events: VecDeque<Value>,
    in_flight: bool,
    flush_requested: bool,
    spool_loaded: bool,
}

struct Rate {
    window: Instant,
    count: usize,
}

struct Inner {
    env: EnvFn,
    home: Option<PathBuf>,
    product: Mutex<&'static str>,
    product_version: Mutex<String>,
    notice_mode: Mutex<NoticeMode>,
    sink: Arc<dyn Sink>,
    queue: Mutex<Queue>,
    cond: Condvar,
    identity: Mutex<Option<Identity>>,
    session_id: String,
    rate: Mutex<Rate>,
    decision: Mutex<Option<(Instant, Decision)>>,
    sender_started: AtomicBool,
    background: bool,
    first_run_checked: AtomicBool,
    /// This process showed the first-run notice: it sends nothing.
    notice_shown_here: AtomicBool,
}

/// A telemetry client. Cheap to clone.
#[derive(Clone)]
pub struct Telemetry {
    inner: Arc<Inner>,
}

/// Builder for a [`Telemetry`].
pub struct Builder {
    env: EnvFn,
    home: Option<Option<PathBuf>>,
    product: &'static str,
    product_version: String,
    sink: Option<Arc<dyn Sink>>,
    background: bool,
    notice_mode: NoticeMode,
}

impl Builder {
    /// Reads `env` instead of the process environment.
    pub fn env(mut self, env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Box::new(env);
        self
    }

    /// Uses `home` as `$CUA_HOME`.
    pub fn home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(Some(home.into()));
        self
    }

    /// Sends to `sink` instead of PostHog.
    pub fn sink(mut self, sink: Arc<dyn Sink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Product and version.
    pub fn product(mut self, product: &str, version: &str) -> Self {
        self.product = schema::PRODUCTS
            .iter()
            .copied()
            .find(|p| *p == product)
            .unwrap_or("sdk_rust");
        self.product_version = version.to_string();
        self
    }

    /// Send only on [`Telemetry::flush`] (tests), not from a background
    /// thread.
    pub fn foreground(mut self) -> Self {
        self.background = false;
        self
    }

    /// How the first-run notice is shown.
    pub fn notice_mode(mut self, mode: NoticeMode) -> Self {
        self.notice_mode = mode;
        self
    }

    /// Builds the client.
    pub fn build(self) -> Telemetry {
        // Without an explicit home, `$CUA_HOME` is re-read on every use, so
        // a process that changes it (tests, embedders) never writes to the
        // old one.
        let home = self.home.flatten();
        let sink = self.sink.unwrap_or_else(|| {
            let endpoint = (self.env)(config::ENV_ENDPOINT)
                .filter(|e| !e.trim().is_empty())
                .unwrap_or_else(|| sink::DEFAULT_ENDPOINT.to_string());
            Arc::new(HttpSink::new(
                endpoint,
                config::network_forbidden(&*self.env),
            ))
        });
        let mut id = [0u8; 16];
        rand::rng().fill(&mut id);
        Telemetry {
            inner: Arc::new(Inner {
                env: self.env,
                home,
                product: Mutex::new(self.product),
                product_version: Mutex::new(self.product_version),
                notice_mode: Mutex::new(self.notice_mode),
                sink,
                queue: Mutex::new(Queue {
                    events: VecDeque::new(),
                    in_flight: false,
                    flush_requested: false,
                    spool_loaded: false,
                }),
                cond: Condvar::new(),
                identity: Mutex::new(None),
                session_id: hex::encode(id),
                rate: Mutex::new(Rate {
                    window: Instant::now(),
                    count: 0,
                }),
                decision: Mutex::new(None),
                sender_started: AtomicBool::new(false),
                background: self.background,
                first_run_checked: AtomicBool::new(false),
                notice_shown_here: AtomicBool::new(false),
            }),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

static GLOBAL: OnceLock<Telemetry> = OnceLock::new();

/// The process-wide client (process environment, `$CUA_HOME`, PostHog).
/// Its product is `sdk_rust` until [`Telemetry::set_product`] or [`init`].
pub fn global() -> &'static Telemetry {
    GLOBAL.get_or_init(|| Telemetry::builder().build())
}

/// Sets the process-wide client's product (call early; the CLI, daemon,
/// app and bindings do).
pub fn init(product: &str, version: &str) -> &'static Telemetry {
    let t = global();
    t.set_product(product, version);
    t
}

impl Telemetry {
    /// A builder over the process environment.
    pub fn builder() -> Builder {
        Builder {
            env: Box::new(config::process_env),
            home: None,
            product: "sdk_rust",
            product_version: env!("CARGO_PKG_VERSION").to_string(),
            sink: None,
            background: true,
            notice_mode: NoticeMode::Stderr,
        }
    }

    fn env(&self, name: &str) -> Option<String> {
        (self.inner.env)(name)
    }

    /// The product and version events are attributed to.
    pub fn set_product(&self, product: &str, version: &str) {
        if let Some(p) = schema::PRODUCTS.iter().copied().find(|p| *p == product) {
            *lock(&self.inner.product) = p;
        }
        if schema::is_strict_version(version) {
            *lock(&self.inner.product_version) = version.to_string();
        }
    }

    /// The product name.
    pub fn product(&self) -> &'static str {
        *lock(&self.inner.product)
    }

    /// How the first-run notice is shown.
    pub fn set_notice_mode(&self, mode: NoticeMode) {
        *lock(&self.inner.notice_mode) = mode;
    }

    /// `$CUA_HOME` (or `~/.cua`), if known.
    pub fn home(&self) -> Option<PathBuf> {
        self.inner
            .home
            .clone()
            .or_else(|| config::cua_home(&*self.inner.env))
    }

    /// `$CUA_HOME/telemetry`, if known.
    pub fn state_dir(&self) -> Option<PathBuf> {
        self.home().map(|h| config::state_dir(&h))
    }

    /// The effective on/off decision (re-read at most every 5 s, so a
    /// switch flipped by another process applies quickly).
    pub fn decision(&self) -> Decision {
        let mut d = lock(&self.inner.decision);
        if let Some((at, dec)) = d.as_ref()
            && at.elapsed() < Duration::from_secs(5)
        {
            return dec.clone();
        }
        let dec = config::decide(&*self.inner.env, self.home().as_deref());
        *d = Some((Instant::now(), dec.clone()));
        dec
    }

    /// Forgets the cached decision (after a command that may have changed a
    /// switch).
    pub fn refresh(&self) {
        *lock(&self.inner.decision) = None;
    }

    /// Whether events may be sent.
    pub fn is_enabled(&self) -> bool {
        self.decision().enabled
    }

    /// Turns telemetry on or off in `$CUA_HOME/config.toml` (the environment
    /// still wins). Turning it off also deletes the offline spool.
    pub fn set_enabled(&self, enabled: bool) -> std::io::Result<PathBuf> {
        let home = self
            .home()
            .ok_or_else(|| std::io::Error::other("no home directory (set CUA_HOME)"))?;
        let path = config::write_config_value(&home, enabled)?;
        *lock(&self.inner.decision) = None;
        if !enabled {
            lock(&self.inner.queue).events.clear();
            if let Some(dir) = self.state_dir() {
                fsutil::remove(&dir.join(SPOOL_FILE))?;
            }
        }
        Ok(path)
    }

    /// Deletes the install id and salt; the next event gets new ones.
    pub fn reset_id(&self) -> std::io::Result<Vec<PathBuf>> {
        *lock(&self.inner.identity) = None;
        match self.state_dir() {
            Some(dir) => identity::reset(&dir),
            None => Ok(vec![]),
        }
    }

    /// Whether the first-run notice has been shown on this machine.
    pub fn notice_shown(&self) -> bool {
        self.state_dir()
            .is_some_and(|d| d.join(notice::MARKER).exists())
    }

    /// Records that the notice was shown (the app calls this after showing
    /// it in its UI).
    pub fn acknowledge_notice(&self) {
        if let Some(d) = self.state_dir() {
            let _ = fsutil::create_once(&d.join(notice::MARKER), b"1\n");
        }
    }

    /// Shows the first-run notice on stderr if it has not been shown and
    /// telemetry is on. Returns whether it printed. The CLI calls this at
    /// startup; SDK processes get it on their first event.
    pub fn show_notice_if_needed(&self) -> bool {
        if !self.is_enabled() || self.notice_shown() {
            return false;
        }
        if *lock(&self.inner.notice_mode) == NoticeMode::External {
            return false;
        }
        eprintln!("{}", notice::TEXT);
        self.acknowledge_notice();
        self.inner.notice_shown_here.store(true, Ordering::SeqCst);
        true
    }

    fn identity(&self) -> Identity {
        let mut g = lock(&self.inner.identity);
        if let Some(i) = g.as_ref() {
            return i.clone();
        }
        let i = match self.state_dir() {
            Some(d) => Identity::load_or_create(&d),
            None => Identity::ephemeral(),
        };
        *g = Some(i.clone());
        i
    }

    /// A per-install salted hash (see [`Identity::hash`]). None when
    /// telemetry is off (then no identity is created).
    pub fn salted_hash(&self, kind: &str, value: &str) -> Option<String> {
        self.is_enabled().then(|| self.identity().hash(kind, value))
    }

    fn common(&self, spec: &schema::EventSpec, rate: f64) -> Map<String, Value> {
        let dec = self.decision();
        let mut m = Map::new();
        let mut put = |k: &str, v: Value| {
            m.insert(k.into(), v);
        };
        put(
            "telemetry_schema_version",
            Value::from(schema::SCHEMA_VERSION),
        );
        put("event_version", Value::from(spec.version));
        put("product", Value::from(self.product()));
        let version = lock(&self.inner.product_version).clone();
        put(
            "product_version",
            Value::from(if schema::is_strict_version(&version) {
                version
            } else {
                "0.0.0".into()
            }),
        );
        put("os_family", Value::from(os_family()));
        put("os_major", Value::from(os_major()));
        put("arch", Value::from(arch()));
        put("is_ci", Value::Bool(dec.is_ci));
        put(
            "is_synthetic",
            Value::Bool(
                self.env(config::ENV_SYNTHETIC)
                    .as_deref()
                    .and_then(config::parse_bool)
                    .unwrap_or(false),
            ),
        );
        put(
            "process_session_id",
            Value::from(self.inner.session_id.clone()),
        );
        put("sample_rate", Value::from(rate));
        put("$process_person_profile", Value::Bool(false));
        put("$geoip_disable", Value::Bool(true));
        put("$lib", Value::from("cua-telemetry"));
        put("$lib_version", Value::from(env!("CARGO_PKG_VERSION")));
        m
    }

    /// The exact properties every event from this process carries (what
    /// `cua telemetry status` prints).
    pub fn envelope_preview(&self) -> Map<String, Value> {
        let spec = schema::spec(schema::event::CLI_COMMAND).expect("declared");
        self.common(spec, 1.0)
    }

    /// Builds the full payload for `event` without sending it (and without
    /// creating an install id: the id is shown as `<install id>`).
    pub fn preview(&self, event: &Event) -> Result<Value, schema::SchemaError> {
        let spec = schema::spec(event.name)
            .ok_or_else(|| schema::SchemaError::UnknownEvent(event.name.into()))?;
        let mut props = self.common(spec, spec.sample_rate);
        for (k, v) in &event.props {
            props.insert(k.clone(), v.clone());
        }
        schema::validate(event.name, &props)?;
        Ok(serde_json::json!({
            "event": event.name,
            "distinct_id": "<install id>",
            "properties": props,
        }))
    }

    /// Captures `event`. Never blocks on the network and never fails the
    /// caller.
    pub fn capture(&self, event: Event) -> Captured {
        let dec = self.decision();
        if !dec.enabled {
            // Off means nothing is kept for later either.
            if let Some(dir) = self.state_dir() {
                let _ = fsutil::remove(&dir.join(SPOOL_FILE));
            }
            return Captured::Disabled;
        }
        if self.inner.notice_shown_here.load(Ordering::SeqCst) {
            return Captured::NoticePending;
        }
        if !self.notice_shown() {
            // Nothing is sent before the notice has been shown once. The
            // process that shows it sends nothing.
            self.show_notice_if_needed();
            return Captured::NoticePending;
        }
        self.record_first_run();
        self.capture_inner(event)
    }

    fn capture_inner(&self, event: Event) -> Captured {
        let Some(spec) = schema::spec(event.name) else {
            return Captured::Invalid(format!("unknown event {}", event.name));
        };
        if spec.sample_rate < 1.0 && rand::rng().random::<f64>() >= spec.sample_rate {
            return Captured::Sampled;
        }
        {
            let mut r = lock(&self.inner.rate);
            if r.window.elapsed() >= Duration::from_secs(3600) {
                r.window = Instant::now();
                r.count = 0;
            }
            if r.count >= MAX_PER_HOUR {
                return Captured::RateLimited;
            }
            r.count += 1;
        }
        let mut props = self.common(spec, spec.sample_rate);
        for (k, v) in event.props {
            props.insert(k, v);
        }
        if let Err(e) = schema::validate(event.name, &props) {
            return Captured::Invalid(e.to_string());
        }
        let identity = self.identity();
        let payload = serde_json::json!({
            "event": event.name,
            "distinct_id": identity.id,
            "uuid": random_uuid(),
            "timestamp": rfc3339(SystemTime::now()),
            "properties": props,
        });
        if self
            .env(config::ENV_DEBUG)
            .as_deref()
            .and_then(config::parse_bool)
            .unwrap_or(false)
        {
            eprintln!("[cua telemetry] {payload}");
        }
        self.log_last(&payload, "queued");
        {
            let mut q = lock(&self.inner.queue);
            if q.events.len() >= MAX_QUEUE {
                q.events.pop_front();
            }
            q.events.push_back(payload);
        }
        self.start_sender();
        self.inner.cond.notify_all();
        Captured::Queued
    }

    /// Records `cua_first_run` once per install (after the notice).
    fn record_first_run(&self) {
        if self.inner.first_run_checked.swap(true, Ordering::SeqCst) {
            return;
        }
        // A sandbox's cua-spacesd is not an install.
        if self.product() == "spacesd" {
            return;
        }
        let Some(dir) = self.state_dir() else { return };
        let marker = dir.join(FIRST_RUN_FILE);
        if marker.exists() || fsutil::create_once(&marker, b"1\n").is_err() {
            return;
        }
        // The installers record how they installed Cua in the state dir;
        // `CUA_INSTALL_CHANNEL` wins. The event keeps only known channels.
        let channel = self
            .env(config::ENV_INSTALL_CHANNEL)
            .or_else(|| {
                std::fs::read_to_string(dir.join(config::INSTALL_CHANNEL_FILE))
                    .ok()
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
            })
            .unwrap_or_else(|| match self.product() {
                "spaces_app" => "spaces_app".into(),
                "sdk_python" => "pip".into(),
                "sdk_typescript" => "npm".into(),
                _ => "unknown".into(),
            });
        self.capture_inner(crate::events::first_run(&channel));
    }

    /// Captures an onboarding `first_*` step once per install; other steps
    /// every time.
    pub fn capture_step(&self, step: &str, outcome: crate::events::Outcome) -> Captured {
        let Some(ev) = crate::events::onboarding_step(step, outcome) else {
            return Captured::Invalid(format!("unknown step {step}"));
        };
        if step.starts_with("first_") {
            if !self.is_enabled() {
                return Captured::Disabled;
            }
            let Some(dir) = self.state_dir() else {
                return Captured::Disabled;
            };
            let marker = dir.join(FUNNEL_DIR).join(step);
            if marker.exists() {
                return Captured::AlreadyRecorded;
            }
            let r = self.capture(ev);
            if r == Captured::Queued {
                let _ = fsutil::create_once(&marker, b"1\n");
            }
            return r;
        }
        self.capture(ev)
    }

    /// Captures `cua_app_active` at most once per install per UTC day. Call
    /// it whenever the product is used; repeat calls the same day are
    /// dropped on device.
    pub fn capture_active_day(&self) -> Captured {
        let day = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 86_400)
            .unwrap_or(0);
        self.capture_active_on(day)
    }

    /// [`Self::capture_active_day`] for a given UTC day (days since the
    /// epoch).
    pub fn capture_active_on(&self, day: u64) -> Captured {
        if !self.is_enabled() {
            return Captured::Disabled;
        }
        // A sandbox's cua-spacesd is not an install.
        if self.product() == "spacesd" {
            return Captured::Disabled;
        }
        let Some(dir) = self.state_dir() else {
            return Captured::Disabled;
        };
        let marker = dir.join(ACTIVE_DAY_FILE);
        let today = format!("{day}\n");
        if std::fs::read_to_string(&marker).is_ok_and(|s| s == today) {
            return Captured::AlreadyRecorded;
        }
        // The experiments the Spaces app last said are on (any product of
        // this install sends the day's event; the set is the install's).
        let experiments = std::fs::read_to_string(dir.join(EXPERIMENTS_FILE)).unwrap_or_default();
        let ids: Vec<&str> = experiments.trim().split('+').collect();
        let r = self.capture(crate::events::app_active_with(&ids));
        if r == Captured::Queued {
            let _ = fsutil::write_replace(&marker, today.as_bytes());
        }
        r
    }

    /// The Spaces app's experiments that are on now (fixed ids; anything
    /// else is dropped): kept in the state directory, so the day's
    /// `cua_app_active` carries them whichever product sends it. Nothing is
    /// written while telemetry is off.
    pub fn set_experiments_on(&self, ids: &[&str]) {
        if !self.is_enabled() {
            return;
        }
        let Some(dir) = self.state_dir() else { return };
        let path = dir.join(EXPERIMENTS_FILE);
        let word = crate::events::experiments_set(ids);
        if std::fs::read_to_string(&path).is_ok_and(|s| s.trim() == word) {
            return;
        }
        let _ = fsutil::write_replace(&path, format!("{word}\n").as_bytes());
    }

    fn log_last(&self, payload: &Value, status: &str) {
        let Some(dir) = self.state_dir() else { return };
        let path = dir.join(LAST_FILE);
        let line = serde_json::json!({
            "uuid": payload["uuid"],
            "status": status,
            "payload": payload,
        });
        let mut lines: Vec<String> = std::fs::read_to_string(&path)
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default();
        lines.push(line.to_string());
        if lines.len() > LAST_LOG_LEN * 3 {
            let cut = lines.len() - LAST_LOG_LEN * 3;
            lines.drain(..cut);
        }
        let _ = fsutil::write_replace(&path, (lines.join("\n") + "\n").as_bytes());
    }

    /// The last `limit` events this machine queued or sent, newest last,
    /// each `{status, payload}` exactly as sent (`status`: `queued`, `sent`,
    /// `spooled` or `refused`).
    pub fn show_last(&self, limit: usize) -> Vec<Value> {
        let Some(dir) = self.state_dir() else {
            return vec![];
        };
        let text = std::fs::read_to_string(dir.join(LAST_FILE)).unwrap_or_default();
        let mut order: Vec<String> = Vec::new();
        let mut by_id: std::collections::HashMap<String, Value> = Default::default();
        for l in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(l) else {
                continue;
            };
            let id = v["uuid"].as_str().unwrap_or_default().to_string();
            if !by_id.contains_key(&id) {
                order.push(id.clone());
            }
            by_id.insert(id, v);
        }
        let start = order.len().saturating_sub(limit);
        order[start..]
            .iter()
            .filter_map(|id| by_id.get(id))
            .map(|v| serde_json::json!({"status": v["status"], "payload": v["payload"]}))
            .collect()
    }

    fn start_sender(&self) {
        if !self.inner.background || self.inner.sender_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("cua-telemetry".into())
            .spawn(move || me.sender_loop());
        if spawned.is_err() {
            self.inner.sender_started.store(false, Ordering::SeqCst);
        }
    }

    fn sender_loop(&self) {
        loop {
            {
                let mut q = lock(&self.inner.queue);
                let deadline = Instant::now() + FLUSH_INTERVAL;
                while q.events.len() < BATCH_SIZE && !q.flush_requested {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    q = self
                        .inner
                        .cond
                        .wait_timeout(q, deadline - now)
                        .unwrap_or_else(|p| p.into_inner())
                        .0;
                }
            }
            self.send_once();
        }
    }

    /// Sends what is queued (and the spool) now. Returns how many events
    /// were delivered.
    pub fn send_once(&self) -> usize {
        let (batch, load_spool) = {
            let mut q = lock(&self.inner.queue);
            q.flush_requested = false;
            if q.in_flight {
                return 0;
            }
            let load_spool = !q.spool_loaded;
            q.spool_loaded = true;
            let n = q.events.len().min(BATCH_SIZE);
            let batch: Vec<Value> = q.events.drain(..n).collect();
            q.in_flight = true;
            (batch, load_spool)
        };
        let mut batch = batch;
        if load_spool && self.is_enabled() {
            batch.extend(self.take_spool());
        }
        let mut delivered = 0;
        if !batch.is_empty() {
            // Re-check the switch: a user who turned telemetry off since
            // these were queued gets nothing sent.
            if !self.is_enabled() {
                batch.clear();
            }
        }
        for chunk in batch.chunks(BATCH_SIZE) {
            let body = serde_json::json!({
                "api_key": sink::POSTHOG_API_KEY,
                "batch": chunk,
            });
            match self.inner.sink.send(&body) {
                Ok(()) => {
                    delivered += chunk.len();
                    for e in chunk {
                        self.log_last(e, "sent");
                    }
                }
                Err(SendError::Forbidden) => {
                    for e in chunk {
                        self.log_last(e, "refused");
                    }
                }
                Err(SendError::Failed(_)) => {
                    self.spool(chunk);
                    for e in chunk {
                        self.log_last(e, "spooled");
                    }
                }
            }
        }
        lock(&self.inner.queue).in_flight = false;
        self.inner.cond.notify_all();
        delivered
    }

    /// Sends everything queued, waiting at most `timeout` (process exit).
    pub fn flush(&self, timeout: Duration) {
        if !self.inner.background {
            let deadline = Instant::now() + timeout;
            while !lock(&self.inner.queue).events.is_empty() && Instant::now() < deadline {
                if self.send_once() == 0 && lock(&self.inner.queue).events.is_empty() {
                    break;
                }
            }
            return;
        }
        {
            let mut q = lock(&self.inner.queue);
            if q.events.is_empty() && !q.in_flight {
                return;
            }
            q.flush_requested = true;
        }
        self.inner.cond.notify_all();
        let deadline = Instant::now() + timeout;
        let mut q = lock(&self.inner.queue);
        while (!q.events.is_empty() || q.in_flight) && Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            q = self
                .inner
                .cond
                .wait_timeout(q, left.min(Duration::from_millis(50)))
                .unwrap_or_else(|p| p.into_inner())
                .0;
            if !q.events.is_empty() {
                q.flush_requested = true;
                self.inner.cond.notify_all();
            }
        }
    }

    /// Process exit: flushes for at most `budget`, then moves anything
    /// still queued to the offline spool so it is sent by a later process.
    /// Never waits longer than `budget`.
    pub fn shutdown(&self, budget: Duration) {
        self.flush(budget);
        let rest: Vec<Value> = lock(&self.inner.queue).events.drain(..).collect();
        if !rest.is_empty() && self.is_enabled() {
            self.spool(&rest);
            for e in &rest {
                self.log_last(e, "spooled");
            }
        }
    }

    /// Events waiting in memory.
    pub fn queued(&self) -> usize {
        lock(&self.inner.queue).events.len()
    }

    fn spool(&self, events: &[Value]) {
        let Some(dir) = self.state_dir() else { return };
        let path = dir.join(SPOOL_FILE);
        let mut lines: Vec<String> = std::fs::read_to_string(&path)
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default();
        lines.extend(events.iter().map(Value::to_string));
        if lines.len() > MAX_SPOOL {
            let cut = lines.len() - MAX_SPOOL;
            lines.drain(..cut);
        }
        let _ = fsutil::write_atomic(&path, (lines.join("\n") + "\n").as_bytes());
    }

    fn take_spool(&self) -> Vec<Value> {
        let Some(dir) = self.state_dir() else {
            return vec![];
        };
        let path = dir.join(SPOOL_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return vec![];
        };
        let _ = fsutil::remove(&path);
        let oldest = SystemTime::now()
            .checked_sub(SPOOL_MAX_AGE)
            .map(rfc3339)
            .unwrap_or_default();
        text.lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| {
                v["timestamp"]
                    .as_str()
                    .is_some_and(|t| t >= oldest.as_str())
                    && v["event"]
                        .as_str()
                        .is_some_and(|e| schema::spec(e).is_some())
            })
            .collect()
    }

    /// The sink's endpoint.
    pub fn endpoint(&self) -> String {
        self.inner.sink.endpoint()
    }

    /// Whether the offline spool holds events.
    pub fn spooled(&self) -> usize {
        self.state_dir()
            .and_then(|d| std::fs::read_to_string(d.join(SPOOL_FILE)).ok())
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    /// Status for `cua telemetry status` and the SDK.
    pub fn status(&self) -> Status {
        let dec = self.decision();
        Status {
            enabled: dec.enabled,
            source: dec.source.to_string(),
            source_kind: dec.source.kind().to_string(),
            is_ci: dec.is_ci,
            install_id: self
                .state_dir()
                .and_then(|d| Identity::peek(&d))
                .map(|i| identity::redact(&i)),
            notice_shown: self.notice_shown(),
            endpoint: self.endpoint(),
            product: self.product().to_string(),
            state_dir: self
                .state_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            config_path: self
                .home()
                .map(|h| config::config_path(&h).display().to_string())
                .unwrap_or_default(),
            spooled: self.spooled(),
        }
    }
}

/// `cua telemetry status`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Status {
    pub enabled: bool,
    /// Where the decision comes from (`env CUA_TELEMETRY`, `config <path>`,
    /// ...).
    pub source: String,
    /// `do_not_track`, `env`, `legacy_env`, `config`, `ci` or `default`.
    pub source_kind: String,
    pub is_ci: bool,
    /// First 8 characters of the install id, if one exists.
    pub install_id: Option<String>,
    pub notice_shown: bool,
    pub endpoint: String,
    pub product: String,
    pub state_dir: String,
    pub config_path: String,
    /// Events waiting in the offline spool.
    pub spooled: usize,
}

fn random_uuid() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// RFC 3339 UTC with second precision.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Host OS family.
pub fn os_family() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        _ => "other",
    }
}

/// Host CPU architecture.
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => "other",
    }
}

/// Host OS major version (0 when unknown). Only the major number.
pub fn os_major() -> u64 {
    static MAJOR: OnceLock<u64> = OnceLock::new();
    *MAJOR.get_or_init(|| {
        let raw = os_version_string();
        raw.split(|c: char| !c.is_ascii_digit())
            .find(|p| !p.is_empty())
            .and_then(|p| p.parse::<u64>().ok())
            .filter(|n| *n <= 999)
            .unwrap_or(0)
    })
}

fn os_version_string() -> String {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
    }
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|c| {
                c.lines()
                    .find_map(|l| l.strip_prefix("VERSION_ID=").map(str::to_string))
            })
            .map(|v| v.trim_matches('"').to_string())
            .unwrap_or_default();
    }
    #[allow(unreachable_code)]
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_790_000_000)),
            "2026-09-21T14:13:20Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
    }

    #[test]
    fn uuid_is_v4_shaped() {
        let u = random_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
