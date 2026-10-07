//! Find windows and elements by description, wait for them, and check them.
//!
//! Used by `run_actions` steps that name their target (`role`, `name`, `app`,
//! `window`) instead of carrying a token from an earlier read, and by the
//! per-step `wait_for` / `expect` checks. Every read goes through the same
//! [`ToolRegistry`] entry point a direct call uses (`list_windows`,
//! `list_apps`, `get_window_state`), so a lookup is held to the same session,
//! policy and approval checks as the reads a model would make itself.
//!
//! Lookup reads ask for `full_output`, which the `since` snapshot memory does
//! not remember, so they never become the baseline of the caller's next
//! `since:"latest"` diff.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::{
    protocol::{Content, ToolResult},
    tool::ToolRegistry,
};

/// Keys a step may add to an action's arguments to name its target. They are
/// removed before the action tool runs.
pub(crate) const SELECTOR_KEYS: &[&str] = &["role", "name", "label", "nth", "app", "window"];
/// Keys a `wait_for` / `expect` check accepts.
pub(crate) const CHECK_KEYS: &[&str] = &[
    "role",
    "name",
    "label",
    "text",
    "nth",
    "app",
    "window",
    "pid",
    "window_id",
    "gone",
    "value",
    "value_contains",
    "enabled",
    "selected",
    "timeout_ms",
];

/// Default wait for a named target to appear before its action runs.
pub(crate) const DEFAULT_FIND_TIMEOUT_MS: u64 = 3_000;
/// Default wait for a `wait_for` check.
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: u64 = 5_000;
/// Default wait for an `expect` check.
pub(crate) const DEFAULT_EXPECT_TIMEOUT_MS: u64 = 2_000;
/// Longest wait any single check may ask for.
pub(crate) const MAX_CHECK_TIMEOUT_MS: u64 = 10_000;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const CANDIDATES_SHOWN: usize = 5;
/// Most windows of one app a lookup reads per poll.
const MAX_WINDOWS_SEARCHED: usize = 4;

/// Which window a step or check addresses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowSpec {
    pub pid: Option<i64>,
    pub window_id: Option<u64>,
    /// App name (case-insensitive, exact or substring) or bundle id.
    pub app: Option<String>,
    /// Case-insensitive substring of the window title.
    pub title: Option<String>,
}

impl WindowSpec {
    pub fn is_empty(&self) -> bool {
        self.pid.is_none() && self.window_id.is_none() && self.app.is_none() && self.title.is_none()
    }

    /// A spec that names one exact window needs no lookup.
    fn exact(&self) -> Option<(i64, u64)> {
        match (self.pid, self.window_id, &self.app, &self.title) {
            (Some(pid), Some(window_id), None, None) => Some((pid, window_id)),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(app) = &self.app {
            parts.push(format!("app \"{app}\""));
        }
        if let Some(title) = &self.title {
            parts.push(format!("window \"{title}\""));
        }
        if let Some(pid) = self.pid {
            parts.push(format!("pid {pid}"));
        }
        if let Some(window_id) = self.window_id {
            parts.push(format!("window_id {window_id}"));
        }
        if parts.is_empty() {
            "no window".to_owned()
        } else {
            parts.join(", ")
        }
    }
}

/// Which element inside the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ElementSpec {
    pub role: Option<String>,
    pub name: Option<String>,
    /// Any row of the tree containing this text, display-only rows included.
    pub text: Option<String>,
    pub nth: Option<usize>,
}

impl ElementSpec {
    pub fn is_empty(&self) -> bool {
        self.role.is_none() && self.name.is_none() && self.text.is_none()
    }

    pub fn describe(&self) -> String {
        let mut out = String::new();
        if let Some(role) = &self.role {
            out.push_str(role);
        } else {
            out.push_str("element");
        }
        if let Some(name) = &self.name {
            out.push_str(&format!(" \"{name}\""));
        }
        if let Some(text) = &self.text {
            out.push_str(&format!(" with text \"{text}\""));
        }
        if let Some(nth) = self.nth {
            out.push_str(&format!(" (nth {nth})"));
        }
        out
    }
}

/// A `wait_for` or `expect` predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Check {
    pub window: WindowSpec,
    pub element: ElementSpec,
    pub gone: bool,
    pub value: Option<String>,
    pub value_contains: Option<String>,
    pub enabled: Option<bool>,
    pub selected: Option<bool>,
    pub timeout: Duration,
}

impl Check {
    pub fn parse(raw: &Value, default_timeout_ms: u64) -> Result<Self, String> {
        let object = raw
            .as_object()
            .ok_or("must be an object such as {\"role\":\"button\",\"name\":\"Save\"}")?;
        if let Some(key) = object
            .keys()
            .find(|key| !CHECK_KEYS.contains(&key.as_str()))
        {
            return Err(format!(
                "unknown field `{key}`; a check takes {}",
                CHECK_KEYS.join(", ")
            ));
        }
        let element = element_spec(object)?;
        if element.is_empty() {
            return Err("needs `role`, `name` or `text` to say which element".to_owned());
        }
        let window = window_spec(object)?;
        let gone = opt_bool(object, "gone")?.unwrap_or(false);
        let value = opt_string(object, "value")?;
        let value_contains = opt_string(object, "value_contains")?;
        let enabled = opt_bool(object, "enabled")?;
        let selected = opt_bool(object, "selected")?;
        if gone
            && (value.is_some()
                || value_contains.is_some()
                || enabled.is_some()
                || selected.is_some())
        {
            return Err("`gone:true` cannot be combined with value/enabled/selected".to_owned());
        }
        if element.text.is_some()
            && element.role.is_none()
            && element.name.is_none()
            && (value.is_some()
                || value_contains.is_some()
                || enabled.is_some()
                || selected.is_some())
        {
            return Err(
                "value/enabled/selected need `role` or `name`; `text` alone only checks that the text is shown"
                    .to_owned(),
            );
        }
        let timeout_ms = match object.get("timeout_ms") {
            None | Some(Value::Null) => default_timeout_ms,
            Some(value) => value
                .as_u64()
                .filter(|ms| *ms <= MAX_CHECK_TIMEOUT_MS)
                .ok_or_else(|| {
                    format!("`timeout_ms` must be an integer 0 to {MAX_CHECK_TIMEOUT_MS}")
                })?,
        };
        Ok(Self {
            window,
            element,
            gone,
            value,
            value_contains,
            enabled,
            selected,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    pub fn describe(&self) -> String {
        let mut out = self.element.describe();
        if self.gone {
            out.push_str(" is gone");
        } else {
            out.push_str(" is shown");
        }
        if let Some(value) = &self.value {
            out.push_str(&format!(", value \"{value}\""));
        }
        if let Some(value) = &self.value_contains {
            out.push_str(&format!(", value contains \"{value}\""));
        }
        if let Some(enabled) = self.enabled {
            out.push_str(if enabled { ", enabled" } else { ", disabled" });
        }
        if let Some(selected) = self.selected {
            out.push_str(if selected {
                ", selected"
            } else {
                ", not selected"
            });
        }
        out
    }
}

/// Split the selector keys out of an action's arguments.
pub(crate) fn take_selector(
    args: &mut Map<String, Value>,
) -> Result<(WindowSpec, ElementSpec), String> {
    let mut picked = Map::new();
    for key in SELECTOR_KEYS {
        if let Some(value) = args.remove(*key) {
            picked.insert((*key).to_owned(), value);
        }
    }
    let element = element_spec(&picked)?;
    if element.text.is_some() {
        return Err(
            "`text` only works in wait_for/expect; target an element by role and name".to_owned(),
        );
    }
    let mut window = window_spec(&picked)?;
    // A malformed pid/window_id is left to the tool's schema check, which
    // names the field the same way a direct call would.
    window.pid = args.get("pid").and_then(Value::as_i64);
    window.window_id = args.get("window_id").and_then(Value::as_u64);
    Ok((window, element))
}

fn element_spec(object: &Map<String, Value>) -> Result<ElementSpec, String> {
    let name = match (opt_string(object, "name")?, opt_string(object, "label")?) {
        (Some(_), Some(_)) => return Err("give `name` or `label`, not both".to_owned()),
        (name, label) => name.or(label),
    };
    let nth = match object.get("nth") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or("`nth` must be a non-negative integer (0 is the first match)")?
                as usize,
        ),
    };
    Ok(ElementSpec {
        role: opt_string(object, "role")?,
        name,
        text: opt_string(object, "text")?,
        nth,
    })
}

fn window_spec(object: &Map<String, Value>) -> Result<WindowSpec, String> {
    Ok(WindowSpec {
        pid: opt_i64(object, "pid")?,
        window_id: opt_u64(object, "window_id")?,
        app: opt_string(object, "app")?,
        title: opt_string(object, "window")?,
    })
}

fn opt_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim().to_owned())),
        Some(_) => Err(format!("`{key}` must be a non-empty string")),
    }
}

fn opt_bool(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

fn opt_i64(object: &Map<String, Value>, key: &str) -> Result<Option<i64>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be an integer")),
    }
}

fn opt_u64(object: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

/// One concrete window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    pub pid: i64,
    pub window_id: u64,
    pub app: Option<String>,
    pub title: Option<String>,
    pub on_screen: bool,
}

impl Window {
    pub fn describe(&self) -> String {
        let mut out = String::new();
        if let Some(app) = &self.app {
            out.push_str(app);
            out.push(' ');
        }
        out.push_str(&format!("window {}", self.window_id));
        if let Some(title) = self.title.as_deref().filter(|title| !title.is_empty()) {
            out.push_str(&format!(" \"{}\"", clip(title, 60)));
        }
        out
    }

    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "window_id": self.window_id,
            "app": self.app,
            "title": self.title,
        })
    }
}

/// One accessibility read of a window.
pub(crate) struct Read {
    pub window: Window,
    pub elements: Vec<Value>,
    pub markdown: String,
}

/// An element found by description.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub window: Window,
    pub element: Value,
}

impl Found {
    pub fn token(&self) -> Option<&str> {
        self.element.get("element_token").and_then(Value::as_str)
    }

    pub fn describe(&self) -> String {
        describe_element(&self.element)
    }
}

/// Why a lookup or check did not succeed.
#[derive(Clone, Debug)]
pub(crate) struct Miss {
    /// Stable machine-readable reason.
    pub code: &'static str,
    pub message: String,
    pub window: Option<Window>,
}

impl Miss {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            window: None,
        }
    }

    /// Whether polling again can change the answer. A refused or failed
    /// read, or an ambiguous match, will not fix itself, and retrying a
    /// refused read would only repeat the refusal.
    fn worth_waiting(&self) -> bool {
        matches!(
            self.code,
            "not_found"
                | "app_not_found"
                | "window_not_found"
                | "still_present"
                | "unexpected_state"
        )
    }

    fn in_window(mut self, window: &Window) -> Self {
        self.window = Some(window.clone());
        self
    }
}

/// Outcome of a satisfied check.
#[derive(Clone, Debug)]
pub(crate) struct Checked {
    pub window: Option<Window>,
    pub observed: Option<String>,
    pub waited: Duration,
}

/// Window and element lookup through the registry.
pub(crate) struct Locator<'a> {
    registry: &'a ToolRegistry,
    session: Option<String>,
    /// Hard stop for every poll loop (the batch's own time budget).
    deadline: Instant,
    /// When the batch ends with a default `observe`, the `max_elements` of
    /// that read: the first lookup of each window also makes one plain
    /// read with that budget, which `since:"latest"` remembers, so the end
    /// observation is a diff of what the batch changed rather than a full
    /// read of a window the caller never read.
    baseline_max_elements: Option<u64>,
    baselined: std::sync::Mutex<std::collections::HashSet<(i64, u64)>>,
}

impl<'a> Locator<'a> {
    pub fn new(registry: &'a ToolRegistry, session: Option<String>, deadline: Instant) -> Self {
        Self {
            registry,
            session,
            deadline,
            baseline_max_elements: None,
            baselined: Default::default(),
        }
    }

    pub fn with_observe_baseline(mut self, max_elements: u64) -> Self {
        self.baseline_max_elements = Some(max_elements);
        self
    }

    fn with_session(&self, mut args: Value) -> Value {
        if let (Some(session), Some(object)) = (&self.session, args.as_object_mut()) {
            object.insert("session".to_owned(), Value::String(session.clone()));
        }
        args
    }

    async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        let result = self.registry.invoke(tool, self.with_session(args)).await;
        if result.is_error == Some(true) {
            return Err(format!("{tool}: {}", first_text(&result)));
        }
        Ok(result.structured_content.unwrap_or(Value::Null))
    }

    /// Resolve a spec to one window. With only `app` or `pid`, the app's
    /// frontmost window wins, so a dialog that opens on top is found.
    pub async fn window(&self, spec: &WindowSpec) -> Result<Window, Miss> {
        if let Some((pid, window_id)) = spec.exact() {
            return Ok(Window {
                pid,
                window_id,
                app: None,
                title: None,
                on_screen: true,
            });
        }
        self.listed_window(spec).await
    }

    /// Like [`Self::window`], but always confirms the window through
    /// `list_windows`, so the result carries its app name and title.
    pub async fn listed_window(&self, spec: &WindowSpec) -> Result<Window, Miss> {
        let mut windows = self.listed_windows(spec).await?;
        Ok(windows.remove(0))
    }

    /// Every window matching `spec`, best first: on-screen before
    /// off-screen, then topmost (largest z_index) first. Never empty.
    pub async fn listed_windows(&self, spec: &WindowSpec) -> Result<Vec<Window>, Miss> {
        if spec.is_empty() {
            return Err(Miss::new(
                "no_window",
                "no window to look in: give `app` (or pid and window_id) on this step or an earlier one",
            ));
        }
        let mut list_args = json!({"on_screen_only": false});
        if let Some(pid) = spec.pid {
            list_args["pid"] = json!(pid);
        }
        let listed = self
            .call("list_windows", list_args)
            .await
            .map_err(|error| Miss::new("lookup_failed", error))?;
        let windows: Vec<Value> = listed
            .get("windows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut candidates: Vec<&Value> = windows
            .iter()
            .filter(|window| window.get("pid").and_then(Value::as_i64).is_some())
            .collect();
        if let Some(app) = &spec.app {
            let by_name: Vec<&Value> = candidates
                .iter()
                .copied()
                .filter(|window| text_matches(window.get("app_name"), app))
                .collect();
            candidates = if by_name.is_empty() {
                let pids = self.app_pids(app).await;
                candidates
                    .into_iter()
                    .filter(|window| {
                        window
                            .get("pid")
                            .and_then(Value::as_i64)
                            .is_some_and(|pid| pids.contains(&pid))
                    })
                    .collect()
            } else {
                by_name
            };
            if candidates.is_empty() {
                let mut apps: Vec<&str> = windows
                    .iter()
                    .filter_map(|window| window.get("app_name").and_then(Value::as_str))
                    .collect();
                apps.sort_unstable();
                apps.dedup();
                return Err(Miss::new(
                    "app_not_found",
                    format!(
                        "no window of app \"{app}\" (apps with windows: {})",
                        if apps.is_empty() {
                            "none".to_owned()
                        } else {
                            apps.join(", ")
                        }
                    ),
                ));
            }
        }
        if let Some(window_id) = spec.window_id {
            candidates.retain(|window| {
                window.get("window_id").and_then(Value::as_u64) == Some(window_id)
            });
        }
        if let Some(title) = &spec.title {
            let before: Vec<String> = candidates
                .iter()
                .filter_map(|window| window.get("title").and_then(Value::as_str))
                .map(|title| format!("\"{}\"", clip(title, 40)))
                .collect();
            candidates.retain(|window| text_matches(window.get("title"), title));
            if candidates.is_empty() {
                return Err(Miss::new(
                    "window_not_found",
                    format!(
                        "no window titled like \"{title}\" for {} (titles: {})",
                        spec.describe(),
                        if before.is_empty() {
                            "none".to_owned()
                        } else {
                            before.join(", ")
                        }
                    ),
                ));
            }
        }
        if candidates.is_empty() {
            return Err(Miss::new(
                "window_not_found",
                format!("no window for {}", spec.describe()),
            ));
        }
        // On-screen windows first, then the topmost (largest z_index).
        let mut ranked: Vec<(usize, &Value)> = candidates.into_iter().enumerate().collect();
        ranked.sort_by_key(|(order, window)| {
            let on_screen = window.get("is_on_screen").and_then(Value::as_bool) != Some(false);
            let z = window
                .get("z_index")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MIN);
            std::cmp::Reverse((on_screen, z, std::cmp::Reverse(*order)))
        });
        Ok(ranked
            .into_iter()
            .map(|(_, window)| Window {
                pid: window
                    .get("pid")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                window_id: window
                    .get("window_id")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                app: window
                    .get("app_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                title: window
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                on_screen: window.get("is_on_screen").and_then(Value::as_bool) != Some(false),
            })
            .collect())
    }

    /// The windows a lookup searches, best first. An exact pid/window_id is
    /// the only one; otherwise the app's on-screen windows in stacking
    /// order, so an element in a dialog on top is found first and one in
    /// the main window is still found when a transient popup is on top.
    pub async fn search_windows(&self, spec: &WindowSpec) -> Result<Vec<Window>, Miss> {
        if spec.exact().is_some() {
            return Ok(vec![self.window(spec).await?]);
        }
        let windows = self.listed_windows(spec).await?;
        let on_screen: Vec<Window> = windows.iter().filter(|w| w.on_screen).cloned().collect();
        let mut pool = if on_screen.is_empty() {
            windows
        } else {
            on_screen
        };
        pool.truncate(MAX_WINDOWS_SEARCHED);
        Ok(pool)
    }

    /// Pids of running apps whose name or bundle id matches.
    async fn app_pids(&self, app: &str) -> Vec<i64> {
        let Ok(listed) = self.call("list_apps", json!({})).await else {
            return Vec::new();
        };
        listed
            .get("apps")
            .and_then(Value::as_array)
            .map(|apps| {
                apps.iter()
                    .filter(|entry| {
                        entry
                            .get("bundle_id")
                            .and_then(Value::as_str)
                            .is_some_and(|bundle| bundle.eq_ignore_ascii_case(app))
                            || text_matches(entry.get("name"), app)
                    })
                    .filter_map(|entry| entry.get("pid").and_then(Value::as_i64))
                    .filter(|pid| *pid > 0)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One fresh accessibility read. It refreshes the window's element
    /// tokens, so the tokens it returns are the ones to act with.
    pub async fn read(&self, window: &Window) -> Result<Read, Miss> {
        if let Some(max_elements) = self.baseline_max_elements {
            let first = self
                .baselined
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((window.pid, window.window_id));
            if first {
                // Best effort: a failed baseline only means a full end read.
                let _ = self
                    .call(
                        "get_window_state",
                        json!({
                            "pid": window.pid,
                            "window_id": window.window_id,
                            "max_elements": max_elements,
                            "include_screenshot": false,
                        }),
                    )
                    .await;
            }
        }
        let state = self
            .call(
                "get_window_state",
                json!({
                    "pid": window.pid,
                    "window_id": window.window_id,
                    "full_output": true,
                    "include_screenshot": false,
                }),
            )
            .await
            .map_err(|error| Miss::new("read_failed", error).in_window(window))?;
        let elements = state
            .get("elements")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let markdown = state
            .get("tree_markdown")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(Read {
            window: window.clone(),
            elements,
            markdown,
        })
    }

    /// Find exactly one element, waiting up to `timeout` for it to appear.
    pub async fn find(
        &self,
        window: &WindowSpec,
        element: &ElementSpec,
        timeout: Duration,
    ) -> Result<Found, Miss> {
        let started = Instant::now();
        let deadline = (started + timeout).min(self.deadline);
        loop {
            let attempt = self.find_once(window, element).await;
            match attempt {
                Ok(found) => return Ok(found),
                Err(miss) if !miss.worth_waiting() => return Err(miss),
                Err(miss) => {
                    if Instant::now() + POLL_INTERVAL > deadline {
                        let waited = started.elapsed().as_millis();
                        let mut miss = miss;
                        if waited > 0 {
                            miss.message = format!("{} (waited {waited} ms)", miss.message);
                        }
                        return Err(miss);
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }

    /// One pass over the windows `spec` names: the first window holding a
    /// unique match wins. When none does, the miss comes from the window
    /// with the most elements, whose nearest matches are the useful ones.
    async fn find_once(&self, spec: &WindowSpec, element: &ElementSpec) -> Result<Found, Miss> {
        let windows = self.search_windows(spec).await?;
        let mut best_miss: Option<(usize, Miss)> = None;
        for window in &windows {
            let read = self.read(window).await?;
            match pick(&read, element) {
                Ok(found) => return Ok(found),
                Err(miss) if miss.code == "ambiguous" || miss.code == "not_addressable" => {
                    return Err(miss.in_window(window))
                }
                Err(miss) => {
                    let size = read.elements.len();
                    if best_miss.as_ref().is_none_or(|(best, _)| size > *best) {
                        best_miss = Some((size, miss.in_window(window)));
                    }
                }
            }
        }
        let (_, mut miss) = best_miss.expect("search_windows is never empty");
        if windows.len() > 1 {
            miss.message = format!(
                "{} (searched {} windows of the app)",
                miss.message,
                windows.len()
            );
        }
        Err(miss)
    }

    /// One pass of a check over the windows it names. A presence check
    /// holds when any window satisfies it; `gone` holds when none shows it.
    async fn check_once(
        &self,
        check: &Check,
        spec: &WindowSpec,
        started: Instant,
    ) -> Result<Checked, Miss> {
        let windows = match self.search_windows(spec).await {
            Ok(windows) => windows,
            Err(miss)
                if check.gone && matches!(miss.code, "window_not_found" | "app_not_found") =>
            {
                return Ok(Checked {
                    window: None,
                    observed: Some(format!("{} is closed", spec.describe())),
                    waited: started.elapsed(),
                })
            }
            Err(miss) => return Err(miss),
        };
        let mut best_miss: Option<(usize, Miss)> = None;
        for window in &windows {
            let read = match self.read(window).await {
                Ok(read) => read,
                // A window that closed between the lookup and the read has
                // no elements left.
                Err(_) if check.gone => continue,
                Err(miss) => return Err(miss),
            };
            match evaluate(check, &read) {
                Ok(observed) if !check.gone => {
                    return Ok(Checked {
                        window: Some(window.clone()),
                        observed,
                        waited: started.elapsed(),
                    })
                }
                Ok(_) => {}
                Err(miss) if check.gone => return Err(miss),
                Err(miss) => {
                    let size = read.elements.len();
                    if best_miss.as_ref().is_none_or(|(best, _)| size > *best) {
                        best_miss = Some((size, miss));
                    }
                }
            }
        }
        match best_miss {
            None => Ok(Checked {
                window: windows.first().cloned(),
                observed: None,
                waited: started.elapsed(),
            }),
            Some((_, mut miss)) => {
                if windows.len() > 1 {
                    miss.message = format!(
                        "{} (searched {} windows of the app)",
                        miss.message,
                        windows.len()
                    );
                }
                Err(miss)
            }
        }
    }

    /// Poll until the check holds or its timeout passes.
    pub async fn check(&self, check: &Check, fallback: &WindowSpec) -> Result<Checked, Miss> {
        let window_spec = if check.window.is_empty() {
            fallback.clone()
        } else {
            check.window.clone()
        };
        let started = Instant::now();
        let deadline = (started + check.timeout).min(self.deadline);
        loop {
            let outcome = self.check_once(check, &window_spec, started).await;
            match outcome {
                Ok(checked) => return Ok(checked),
                Err(miss) if !miss.worth_waiting() => return Err(miss),
                Err(miss) => {
                    if Instant::now() + POLL_INTERVAL > deadline {
                        let mut miss = miss;
                        miss.message = format!(
                            "{} (checked for {} ms)",
                            miss.message,
                            started.elapsed().as_millis()
                        );
                        return Err(miss);
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }
}

/// Elements of a read that match `spec`. Exact name matches win over
/// substring matches; zero-size elements only count when nothing visible
/// matches.
pub(crate) fn matches<'r>(read: &'r Read, spec: &ElementSpec) -> Vec<&'r Value> {
    let by_role: Vec<&Value> = read
        .elements
        .iter()
        .filter(|element| element.get("display_only").and_then(Value::as_bool) != Some(true))
        .filter(|element| {
            spec.role.as_deref().is_none_or(|role| {
                element
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|actual| role_matches(role, actual))
            })
        })
        .collect();
    let named: Vec<&Value> = match &spec.name {
        None => by_role,
        Some(name) => {
            let needle = fold(name);
            let exact: Vec<&Value> = by_role
                .iter()
                .copied()
                .filter(|element| label_of(element).is_some_and(|label| fold(label) == needle))
                .collect();
            if exact.is_empty() {
                by_role
                    .into_iter()
                    .filter(|element| {
                        label_of(element).is_some_and(|label| has_word_start(&fold(label), &needle))
                    })
                    .collect()
            } else {
                exact
            }
        }
    };
    let visible: Vec<&Value> = named
        .iter()
        .copied()
        .filter(|element| has_area(element))
        .collect();
    if visible.is_empty() {
        named
    } else {
        visible
    }
}

fn pick(read: &Read, spec: &ElementSpec) -> Result<Found, Miss> {
    let found = matches(read, spec);
    let chosen = match (found.len(), spec.nth) {
        (0, _) => {
            return Err(Miss::new(
                "not_found",
                format!(
                    "no {} in {}{}",
                    spec.describe(),
                    read.window.describe(),
                    near_misses(read, spec)
                ),
            ))
        }
        (count, Some(nth)) if nth >= count => {
            return Err(Miss::new(
                "not_found",
                format!(
                    "nth {nth} asked but only {count} {} in {}",
                    spec.describe(),
                    read.window.describe()
                ),
            ))
        }
        (_, Some(nth)) => found[nth],
        (1, None) => found[0],
        (count, None) => {
            let listed: Vec<String> = found
                .iter()
                .take(CANDIDATES_SHOWN)
                .enumerate()
                .map(|(nth, element)| format!("nth {nth}: {}", describe_element(element)))
                .collect();
            return Err(Miss::new(
                "ambiguous",
                format!(
                    "{count} elements match {} in {}; add `nth` or a more exact name: {}",
                    spec.describe(),
                    read.window.describe(),
                    listed.join("; ")
                ),
            ));
        }
    };
    if chosen
        .get("element_token")
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(Miss::new(
            "not_addressable",
            format!(
                "{} in {} has no element_token to act on",
                describe_element(chosen),
                read.window.describe()
            ),
        ));
    }
    Ok(Found {
        window: read.window.clone(),
        element: chosen.clone(),
    })
}

/// `Ok(observed)` when the check holds against this read.
fn evaluate(check: &Check, read: &Read) -> Result<Option<String>, Miss> {
    let element_spec = ElementSpec {
        nth: None,
        ..check.element.clone()
    };
    let structured = if element_spec.role.is_some() || element_spec.name.is_some() {
        matches(read, &element_spec)
    } else {
        Vec::new()
    };
    // Static text and other display-only rows are only in the markdown.
    let rows: Vec<&str> = read
        .markdown
        .lines()
        .map(str::trim)
        .filter(|line| row_matches(line, &check.element))
        .collect();
    let by_text_only = element_spec.role.is_none() && element_spec.name.is_none();
    let present = if by_text_only {
        !rows.is_empty()
    } else {
        !structured.is_empty() || (check.element.text.is_none() && !rows.is_empty())
    };
    let window = &read.window;
    if check.gone {
        return if present {
            let shown = structured
                .first()
                .map(|element| describe_element(element))
                .or_else(|| rows.first().map(|row| clip(row, 120)))
                .unwrap_or_default();
            Err(Miss::new(
                "still_present",
                format!(
                    "{} is still shown in {}: {shown}",
                    check.element.describe(),
                    window.describe()
                ),
            )
            .in_window(window))
        } else {
            Ok(None)
        };
    }
    if !present {
        return Err(Miss::new(
            "not_found",
            format!(
                "no {} in {}{}",
                check.element.describe(),
                window.describe(),
                near_misses(read, &check.element)
            ),
        )
        .in_window(window));
    }
    let needs_fields = check.value.is_some()
        || check.value_contains.is_some()
        || check.enabled.is_some()
        || check.selected.is_some();
    if !needs_fields {
        let observed = structured
            .first()
            .map(|element| describe_element(element))
            .or_else(|| rows.first().map(|row| clip(row, 120)));
        return Ok(observed);
    }
    let pool: Vec<&Value> = match check.element.nth {
        Some(nth) => structured.get(nth).copied().into_iter().collect(),
        None => structured.clone(),
    };
    if let Some(element) = pool.iter().find(|element| fields_hold(check, element)) {
        return Ok(Some(describe_element(element)));
    }
    let shown = pool
        .iter()
        .take(CANDIDATES_SHOWN)
        .map(|element| describe_element(element))
        .collect::<Vec<_>>()
        .join("; ");
    Err(Miss::new(
        "unexpected_state",
        format!(
            "{} expected in {}, found {}",
            check.describe(),
            window.describe(),
            if shown.is_empty() {
                "no addressable match".to_owned()
            } else {
                shown
            }
        ),
    )
    .in_window(window))
}

fn fields_hold(check: &Check, element: &Value) -> bool {
    let value = element
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(expected) = &check.value {
        if value.trim() != expected.trim() {
            return false;
        }
    }
    if let Some(expected) = &check.value_contains {
        if !fold(value).contains(&fold(expected)) {
            return false;
        }
    }
    if let Some(expected) = check.enabled {
        // Elements report `enabled` only when the platform knows it.
        if element
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            != expected
        {
            return false;
        }
    }
    if let Some(expected) = check.selected {
        if selected_state(element).unwrap_or(false) != expected {
            return false;
        }
    }
    true
}

/// An element's on/off state. For a toggle control (checkbox, radio,
/// switch) its checked value ("1"/"0", "true"/"false", "on"/"off") wins:
/// Chromium reports web checkboxes as value "1" with AXSelected false,
/// because AXSelected means list selection, not checked. Otherwise the
/// platform's `selected` flag, else that value.
fn selected_state(element: &Value) -> Option<bool> {
    let checked = element
        .get("value")
        .and_then(Value::as_str)
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "checked" => Some(true),
            "0" | "false" | "off" | "unchecked" => Some(false),
            _ => None,
        });
    let toggle = element
        .get("role")
        .and_then(Value::as_str)
        .map(role_family)
        .is_some_and(|family| matches!(family.as_str(), "checkbox" | "radiobutton" | "switch"));
    if toggle && checked.is_some() {
        return checked;
    }
    element.get("selected").and_then(Value::as_bool).or(checked)
}

/// Display-only rows (static text and other rows without an element index)
/// that match `spec`, parsed into {role, label, value, text}. Scripts read
/// values such as "Ticket: 4F2K" from these; they cannot be acted on.
pub(crate) fn display_rows(read: &Read, spec: &ElementSpec) -> Vec<Value> {
    if spec.role.is_none() && spec.name.is_none() && spec.text.is_none() {
        return Vec::new();
    }
    read.markdown
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("- "))
        .filter(|rest| !rest.starts_with('['))
        .filter_map(|rest| {
            let role = rest.split_whitespace().next()?.to_owned();
            let after = rest[role.len()..].trim_start();
            let quoted = |text: &str| -> Option<String> {
                let inner = text.strip_prefix('"')?;
                inner.find('"').map(|end| inner[..end].to_owned())
            };
            let label = quoted(after).or_else(|| {
                let open = after.rfind('(')?;
                after[open + 1..].strip_suffix(')').map(str::to_owned)
            });
            let value = after.find("= ").and_then(|at| quoted(&after[at + 2..]));
            let folded = fold(rest);
            if let Some(role_wanted) = &spec.role {
                if !role_matches(role_wanted, &role) {
                    return None;
                }
            }
            if let Some(name) = &spec.name {
                if !has_word_start(&folded, &fold(name)) {
                    return None;
                }
            }
            if let Some(text) = &spec.text {
                if !folded.contains(&fold(text)) {
                    return None;
                }
            }
            Some(json!({
                "role": role,
                "label": label,
                "value": value,
                "text": rest,
                "display_only": true,
            }))
        })
        .collect()
}

fn row_matches(row: &str, spec: &ElementSpec) -> bool {
    if !row.starts_with("- ") {
        return false;
    }
    let folded = fold(row);
    if let Some(text) = &spec.text {
        if !folded.contains(&fold(text)) {
            return false;
        }
    }
    if let Some(name) = &spec.name {
        if !has_word_start(&folded, &fold(name)) {
            return false;
        }
    }
    if let Some(role) = &spec.role {
        let row_role = row
            .trim_start_matches("- ")
            .trim_start_matches(|c: char| c == '[' || c.is_ascii_digit() || c == ']')
            .split_whitespace()
            .next()
            .unwrap_or("");
        if !role_matches(role, row_role) {
            return false;
        }
    }
    spec.text.is_some() || spec.name.is_some()
}

fn near_misses(read: &Read, spec: &ElementSpec) -> String {
    // Same role, any name: the usual slip is a near-miss label.
    let relaxed = ElementSpec {
        name: None,
        text: None,
        nth: None,
        role: spec.role.clone(),
    };
    let pool: Vec<&Value> = if spec.role.is_some() {
        matches(read, &relaxed)
    } else if spec.name.is_some() {
        // Any role: only elements sharing a word with the name (below).
        read.elements
            .iter()
            .filter(|element| element.get("display_only").and_then(Value::as_bool) != Some(true))
            .collect()
    } else {
        Vec::new()
    };
    // Rank by the words a label shares with the wanted name, so "Update
    // event" is offered for "Create event" ahead of unrelated buttons, and
    // look across roles too: the element may be a link, not a button.
    let shared = |element: &Value| -> usize {
        let Some(name) = &spec.name else {
            return 0;
        };
        let Some(label) = label_of(element) else {
            return 0;
        };
        let label_words = words(label);
        words(name)
            .iter()
            .filter(|word| label_words.contains(word))
            .count()
    };
    let mut ranked: Vec<(usize, &Value)> = pool
        .iter()
        .copied()
        .filter(|element| label_of(element).is_some())
        .map(|element| (shared(element), element))
        .collect();
    if spec.role.is_some() && spec.name.is_some() {
        for element in &read.elements {
            let score = shared(element);
            if score > 0
                && element.get("display_only").and_then(Value::as_bool) != Some(true)
                && !ranked.iter().any(|(_, seen)| std::ptr::eq(*seen, element))
            {
                ranked.push((score, element));
            }
        }
    }
    if spec.role.is_none() {
        ranked.retain(|(score, _)| *score > 0);
    }
    ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let named: Vec<String> = ranked
        .iter()
        .take(CANDIDATES_SHOWN)
        .map(|(_, element)| describe_element(element))
        .collect();
    if named.is_empty() {
        if read.elements.is_empty() {
            "; the window has no readable elements".to_owned()
        } else {
            String::new()
        }
    } else {
        format!("; nearest: {}", named.join("; "))
    }
}

pub(crate) fn describe_element(element: &Value) -> String {
    let role = element
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("element");
    let mut out = match element.get("element_index").and_then(Value::as_u64) {
        Some(index) => format!("[{index}] {role}"),
        None => role.to_owned(),
    };
    if let Some(label) = label_of(element) {
        out.push_str(&format!(" \"{}\"", clip(label, 60)));
    }
    if let Some(value) = element.get("value").and_then(Value::as_str) {
        if Some(value) != label_of(element) {
            out.push_str(&format!(" value=\"{}\"", clip(value, 60)));
        }
    }
    if element.get("enabled").and_then(Value::as_bool) == Some(false) {
        out.push_str(" disabled");
    }
    if element.get("selected").and_then(Value::as_bool) == Some(true) {
        out.push_str(" selected");
    }
    out
}

/// Lowercase words of two or more characters.
fn words(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 2)
        .map(str::to_owned)
        .collect()
}

fn label_of(element: &Value) -> Option<&str> {
    element
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.trim().is_empty())
}

fn has_area(element: &Value) -> bool {
    let Some(frame) = element.get("frame") else {
        return true;
    };
    let dimension = |a: &str, b: &str| {
        frame
            .get(a)
            .or_else(|| frame.get(b))
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
    };
    dimension("w", "width") > 0.0 && dimension("h", "height") > 0.0
}

fn text_matches(value: Option<&Value>, needle: &str) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| fold(text).contains(&fold(needle)))
}

/// Whether `needle` occurs in `haystack` starting at a word boundary, so
/// "ok" matches "OK" and "Ok, continue" but not "Bookmark".
fn has_word_start(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.match_indices(needle).any(|(at, _)| {
        haystack[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric())
    })
}

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        let mut out: String = flat.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn first_text(result: &ToolResult) -> String {
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Text { text, .. } => Some(text.trim()),
            _ => None,
        })
        .unwrap_or("failed");
    clip(text.lines().next().unwrap_or(text), 300)
}

/// Role names models use, folded to one family per control kind, across the
/// macOS (AX), Windows (UIA) and Linux (AT-SPI) vocabularies.
pub(crate) fn role_family(role: &str) -> String {
    let normalized: String = role
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(|c| c.to_lowercase())
        .collect();
    let normalized = normalized
        .strip_prefix("ax")
        .unwrap_or(&normalized)
        .to_owned();
    let family = match normalized.as_str() {
        "button" | "pushbutton" | "togglebutton" => "button",
        "textfield" | "textarea" | "textbox" | "edit" | "entry" | "input" | "searchfield"
        | "securetextfield" | "passwordtext" | "text" => "textfield",
        "statictext" | "label" | "static" => "statictext",
        "link" | "hyperlink" => "link",
        "checkbox" | "checkbutton" | "check" => "checkbox",
        "radiobutton" | "radio" => "radiobutton",
        "popupbutton" | "combobox" | "menubutton" | "dropdown" | "select" => "popupbutton",
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => "menuitem",
        "pagetab" | "tabitem" | "tab" => "tab",
        "image" | "img" | "icon" => "image",
        "row" | "listitem" | "tablerow" | "outlinerow" => "row",
        "cell" | "tablecell" | "gridcell" => "cell",
        other => return other.to_owned(),
    };
    family.to_owned()
}

fn role_matches(wanted: &str, actual: &str) -> bool {
    role_family(wanted) == role_family(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(elements: Vec<Value>, markdown: &str) -> Read {
        Read {
            window: Window {
                pid: 1,
                window_id: 2,
                app: Some("Demo".into()),
                title: Some("Doc".into()),
                on_screen: true,
            },
            elements,
            markdown: markdown.to_owned(),
        }
    }

    fn element(index: u64, role: &str, label: &str) -> Value {
        json!({
            "element_index": index,
            "element_token": format!("s00000001:{index}"),
            "role": role,
            "label": label,
            "frame": {"x": 0, "y": 0, "w": 10, "h": 10},
        })
    }

    #[test]
    fn exact_name_beats_substring_and_roles_fold_across_platforms() {
        let read = read(
            vec![
                element(1, "AXButton", "Save As…"),
                element(2, "AXButton", "Save"),
                element(3, "AXTextField", "Save"),
            ],
            "",
        );
        let spec = ElementSpec {
            role: Some("button".into()),
            name: Some("save".into()),
            ..Default::default()
        };
        let found = pick(&read, &spec).unwrap();
        assert_eq!(found.element["element_index"], 2);
        let field = ElementSpec {
            role: Some("textbox".into()),
            name: Some("Save".into()),
            ..Default::default()
        };
        assert_eq!(pick(&read, &field).unwrap().element["element_index"], 3);
        assert_eq!(role_family("Push Button"), "button");
        assert_eq!(role_family("Hyperlink"), "link");
    }

    #[test]
    fn a_short_name_matches_whole_words_only() {
        let read = read(
            vec![
                element(1, "AXButton", "Bookmark this tab"),
                element(2, "AXButton", "Ok, continue"),
            ],
            "",
        );
        let spec = ElementSpec {
            role: Some("button".into()),
            name: Some("ok".into()),
            ..Default::default()
        };
        assert_eq!(pick(&read, &spec).unwrap().element["element_index"], 2);
        assert!(has_word_start("save as…", "as"));
        assert!(!has_word_start("bookmark", "ok"));
    }

    #[test]
    fn ambiguity_lists_candidates_and_nth_picks_one() {
        let read = read(
            vec![element(4, "AXButton", "OK"), element(9, "AXButton", "OK")],
            "",
        );
        let spec = ElementSpec {
            role: Some("button".into()),
            name: Some("OK".into()),
            ..Default::default()
        };
        let miss = pick(&read, &spec).unwrap_err();
        assert_eq!(miss.code, "ambiguous");
        assert!(
            miss.message.contains("nth 1: [9] AXButton \"OK\""),
            "{}",
            miss.message
        );
        let second = ElementSpec {
            nth: Some(1),
            ..spec
        };
        assert_eq!(pick(&read, &second).unwrap().element["element_index"], 9);
    }

    #[test]
    fn a_miss_lists_labels_sharing_a_word_first_across_roles() {
        // v036: "Create event" had become "Update event"; the miss listed
        // the first five buttons instead.
        let mut elements: Vec<Value> = [
            "Aster kickoff",
            "Boreal review",
            "Cinder decision",
            "Dune handoff",
            "Lighthouse lunch",
            "Update event",
        ]
        .iter()
        .enumerate()
        .map(|(at, label)| element(at as u64 + 1, "AXButton", label))
        .collect();
        elements.push(element(9, "AXLink", "Event history"));
        let read = read(elements, "");
        let spec = ElementSpec {
            role: Some("button".into()),
            name: Some("Create event".into()),
            ..Default::default()
        };
        let miss = pick(&read, &spec).unwrap_err();
        let nearest = miss.message.split("nearest: ").nth(1).unwrap();
        assert!(
            nearest.starts_with("[6] AXButton \"Update event\"; [9] AXLink \"Event history\""),
            "{nearest}"
        );

        // With no role, only labels sharing a word are offered.
        let any_role = ElementSpec {
            name: Some("event log".into()),
            ..Default::default()
        };
        let miss = pick(&read, &any_role).unwrap_err();
        assert!(!miss.message.contains("Aster"), "{}", miss.message);
        assert!(miss.message.contains("Update event"), "{}", miss.message);
    }

    #[test]
    fn a_miss_names_the_nearest_elements() {
        let read = read(vec![element(1, "AXButton", "Save Draft")], "");
        let spec = ElementSpec {
            role: Some("button".into()),
            name: Some("Submit".into()),
            ..Default::default()
        };
        let miss = pick(&read, &spec).unwrap_err();
        assert_eq!(miss.code, "not_found");
        assert!(
            miss.message
                .contains("nearest: [1] AXButton \"Save Draft\""),
            "{}",
            miss.message
        );
    }

    #[test]
    fn checks_read_values_static_text_and_absence() {
        let mut field = element(5, "AXTextField", "Email");
        field["value"] = json!("ada@example.com");
        let read = read(
            vec![field],
            "- [0] AXWindow \"Doc\"\n  - AXStaticText \"Saved to disk\"\n  - [5] AXTextField \"Email\"",
        );
        let check = |raw: Value| Check::parse(&raw, 0).unwrap();
        assert!(evaluate(
            &check(json!({"role": "textfield", "name": "email", "value": "ada@example.com"})),
            &read
        )
        .is_ok());
        let wrong = evaluate(&check(json!({"name": "Email", "value": "bob"})), &read).unwrap_err();
        assert_eq!(wrong.code, "unexpected_state");
        assert!(
            wrong.message.contains("value=\"ada@example.com\""),
            "{}",
            wrong.message
        );
        assert!(evaluate(&check(json!({"text": "saved to disk"})), &read).is_ok());
        assert!(evaluate(&check(json!({"text": "Error", "gone": true})), &read).is_ok());
        let still = evaluate(&check(json!({"text": "Saved", "gone": true})), &read).unwrap_err();
        assert_eq!(still.code, "still_present");
    }

    #[test]
    fn selected_falls_back_to_a_checkbox_value() {
        let mut checkbox = element(3, "AXCheckBox", "Accept terms");
        checkbox["value"] = json!("1");
        // As Chromium reports it: checked, but not list-selected.
        checkbox["selected"] = json!(false);
        let read = read(vec![checkbox], "");
        let check = Check::parse(
            &json!({"role": "checkbox", "name": "Accept terms", "selected": true}),
            0,
        )
        .unwrap();
        assert!(evaluate(&check, &read).is_ok());
        let check = Check::parse(
            &json!({"role": "checkbox", "name": "Accept terms", "selected": false}),
            0,
        )
        .unwrap();
        assert_eq!(
            evaluate(&check, &read).unwrap_err().code,
            "unexpected_state"
        );
    }

    #[test]
    fn check_parsing_rejects_unclear_predicates() {
        for (raw, needle) in [
            (json!({}), "needs `role`, `name` or `text`"),
            (json!({"name": "x", "bogus": 1}), "unknown field `bogus`"),
            (
                json!({"name": "x", "gone": true, "value": "y"}),
                "cannot be combined",
            ),
            (json!({"text": "x", "value": "y"}), "need `role` or `name`"),
            (json!({"name": "x", "timeout_ms": 60000}), "timeout_ms"),
            (json!({"name": "x", "label": "y"}), "not both"),
        ] {
            let error = Check::parse(&raw, 0).unwrap_err();
            assert!(error.contains(needle), "{raw}: {error}");
        }
    }

    #[test]
    fn selector_keys_leave_the_action_arguments() {
        let mut args =
            json!({"role": "button", "name": "Go", "app": "Safari", "pid": 3, "count": 2})
                .as_object()
                .cloned()
                .unwrap();
        let (window, element) = take_selector(&mut args).unwrap();
        assert_eq!(window.app.as_deref(), Some("Safari"));
        assert_eq!(window.pid, Some(3));
        assert_eq!(element.name.as_deref(), Some("Go"));
        assert_eq!(args.keys().collect::<Vec<_>>(), ["count", "pid"]);
    }
}
