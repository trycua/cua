// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Preparing the ask-first Space while the person is still in onboarding.
//!
//! The first Space that has a coding agent ready needs four things that take
//! minutes on a new Mac and a slow network: the **Linux runtime** (a small VM
//! the Mac runs Linux Spaces in), the **agent image** (the headless Linux
//! the agent's home runs on), the **home Space** itself, and the **agent**
//! (the coding app installed, its session open). This module is the one
//! state machine for all four, so every app starts them as early as it can
//! (first launch, sign-in, onboarding), says honestly where they are, never
//! loses a message typed meanwhile, and carries on from wherever it was after
//! a lost connection or a quit.
//!
//! * [`reduce`] moves a [`State`] by one [`Action`] and says what the host
//!   does next ([`Effect`]). It never reads a clock, a disk or a network: the
//!   host passes the time in every action and does what the effects say.
//! * [`view`] is what the page draws: the step list, the one honest status
//!   line, the progress, the queued messages and the one button (Try again),
//!   in the core's words, the same on every app.
//!
//! The state is plain JSON (`schema` 1): the host keeps it between launches.
//! It is a record of progress, not the truth: what exists (the Space, its
//! agent) is read from the host, and a state that says more than the host
//! knows is corrected by [`Action::Resume`] and [`Action::HomeGone`].
//!
//! # Messages
//!
//! A message sent while the Space is not ready is **queued**: kept in the
//! state (so a quit does not lose it) and delivered, in order, once the agent
//! is ready, **exactly once**:
//!
//! * a message is claimed (`delivering`) before the host posts it, and is
//!   marked delivered when the post returned;
//! * a claimed message found after a quit or a crash is never posted again
//!   blindly: [`Action::Resume`] asks the host to look in the Space's thread
//!   ([`Effect::VerifyDelivery`]) and the answer
//!   ([`Action::ThreadHas`]) decides: found is delivered, not found goes back
//!   to the queue;
//! * a message id seen twice ([`Action::Send`]) is one message.
//!
//! # Time
//!
//! The state records when each step began and ended and, per message, when it
//! was sent, delivered and first answered, so "onboarding finished to ready",
//! "send to the agent session" and "send to the first model token" are
//! measured the same way everywhere ([`metrics`]).

use cua_create_failure::preflight::IssueKind;
use serde::{Deserialize, Serialize};

/// The state's schema; a state of another schema is dropped (preparing starts over).
pub const SCHEMA: u32 = 1;

/// Seconds before the first retry after a lost connection; it doubles up to
/// [`RETRY_MAX_MS`].
pub const RETRY_FIRST_MS: u64 = 2_000;
/// The longest wait between two retries.
pub const RETRY_MAX_MS: u64 = 60_000;
/// Failed attempts that are not the network's (a full disk, a refused pull)
/// before the page stops retrying and offers Try again.
pub const OTHER_ATTEMPTS: u32 = 2;
/// Times a message is posted again after its post failed, before it stays
/// queued with an error (the person can retry the whole thing).
pub const DELIVERY_ATTEMPTS: u32 = 5;

/// The four steps, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    /// The Linux runtime (the VM Linux Spaces run in on a Mac).
    Runtime,
    /// The agent image.
    Image,
    /// The home Space.
    Home,
    /// The coding agent in it.
    Agent,
}

/// The steps in order.
pub const STEPS: [Step; 4] = [Step::Runtime, Step::Image, Step::Home, Step::Agent];

impl Step {
    fn rank(self) -> usize {
        STEPS.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The step's name in the list.
    pub fn label(self) -> &'static str {
        match self {
            Step::Runtime => "Linux runtime",
            Step::Image => "Agent image",
            Step::Home => "Home Space",
            Step::Agent => "Coding agent",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Step::Runtime => "runtime",
            Step::Image => "image",
            Step::Home => "home",
            Step::Agent => "agent",
        }
    }
}

/// Where one step is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepState {
    /// Not started.
    #[default]
    Pending,
    /// Running.
    Running,
    /// Waiting for the network (it carries on by itself).
    Waiting,
    /// Stopped with an error the person can retry.
    Failed,
    /// Done.
    Done,
    /// Not needed on this machine (a Docker engine is already there).
    Skipped,
}

/// One step's record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepRecord {
    pub state: StepState,
    /// Bytes done and in all, while it downloads.
    pub done: u64,
    pub total: u64,
    /// Bytes per second, smoothed (none: not known).
    pub per_second: Option<u64>,
    /// When it began and ended (ms since the epoch).
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
}

/// Why preparing is not moving.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Trouble {
    /// A lost or missing connection (carries on by itself).
    pub network: bool,
    /// What went wrong, as the host said it.
    pub message: String,
}

/// Where a message is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageState {
    /// Waiting for the agent.
    #[default]
    Queued,
    /// Claimed: the host is posting it.
    Delivering,
    /// In the Space's thread.
    Delivered,
}

/// A message the person sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    /// The caller's id for it; the same id is the same message.
    pub id: String,
    pub text: String,
    /// The coding app and model the person chose, and the repository.
    pub agent: String,
    pub model: Option<String>,
    pub repo: Option<String>,
    pub state: MessageState,
    /// Posts that failed.
    pub attempts: u32,
    pub sent_ms: u64,
    pub claimed_ms: Option<u64>,
    pub delivered_ms: Option<u64>,
    /// The agent's first words in answer.
    pub first_token_ms: Option<u64>,
}

/// The whole record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub schema: u32,
    /// Preparing was asked for (at first launch).
    pub wanted: bool,
    /// When it began, and when onboarding ended (none: still in it).
    pub started_ms: Option<u64>,
    pub onboarding_done_ms: Option<u64>,
    pub runtime: StepRecord,
    pub image: StepRecord,
    pub home: StepRecord,
    pub agent: StepRecord,
    /// The coding app the home is for (`claude-code`).
    pub harness: String,
    /// The home Space, once it exists; the id its row has while it is made.
    pub home_space_id: Option<String>,
    pub pending_id: Option<String>,
    /// Lost connections since the last success (the retry wait's counter).
    pub attempt: u32,
    /// Non-network failures in a row.
    pub failures: u32,
    pub retry_at_ms: Option<u64>,
    pub trouble: Option<Trouble>,
    /// The machine has a connection (the host's reading).
    pub online: bool,
    /// Everything is ready.
    pub ready_ms: Option<u64>,
    /// When a person first wrote into the home (a message sent from Home, or one written in its own
    /// thread). Until then the home is invisible infrastructure: see [`hidden_spaces`].
    pub asked_ms: Option<u64>,
    pub messages: Vec<Message>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            wanted: false,
            started_ms: None,
            onboarding_done_ms: None,
            runtime: StepRecord::default(),
            image: StepRecord::default(),
            home: StepRecord::default(),
            agent: StepRecord::default(),
            harness: "claude-code".into(),
            home_space_id: None,
            pending_id: None,
            attempt: 0,
            failures: 0,
            retry_at_ms: None,
            trouble: None,
            online: true,
            ready_ms: None,
            asked_ms: None,
            messages: Vec::new(),
        }
    }
}

impl State {
    fn rec(&self, s: Step) -> &StepRecord {
        match s {
            Step::Runtime => &self.runtime,
            Step::Image => &self.image,
            Step::Home => &self.home,
            Step::Agent => &self.agent,
        }
    }

    fn rec_mut(&mut self, s: Step) -> &mut StepRecord {
        match s {
            Step::Runtime => &mut self.runtime,
            Step::Image => &mut self.image,
            Step::Home => &mut self.home,
            Step::Agent => &mut self.agent,
        }
    }

    /// Every step is done (or not needed).
    pub fn is_ready(&self) -> bool {
        STEPS
            .iter()
            .all(|s| matches!(self.rec(*s).state, StepState::Done | StepState::Skipped))
    }

    /// The step in progress: the first that is not done.
    fn current(&self) -> Option<Step> {
        STEPS
            .iter()
            .copied()
            .find(|s| !matches!(self.rec(*s).state, StepState::Done | StepState::Skipped))
    }

    fn delivering(&self) -> Option<&Message> {
        self.messages
            .iter()
            .find(|m| m.state == MessageState::Delivering)
    }
}

/// What the host reports or the person does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// Begin preparing (at first launch). Asking again changes nothing.
    Start {
        now_ms: u64,
        /// The coding app (`claude-code`); none keeps the last.
        #[serde(default)]
        harness: Option<String>,
        /// The id the home's row has while it is made.
        #[serde(default)]
        pending_id: Option<String>,
    },
    /// Onboarding ended (the clock for "onboarding finished to ready").
    OnboardingDone { now_ms: u64 },
    /// The create's progress: the daemon's phase word (`preparing`,
    /// `pulling`, `creating`, `booting`, `waiting_for_services`,
    /// `connecting`, `ready`), its detail line and bytes.
    Phase {
        now_ms: u64,
        phase: String,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        done: Option<u64>,
        #[serde(default)]
        total: Option<u64>,
        #[serde(default)]
        per_second: Option<u64>,
    },
    /// The machine already has a Linux engine: the runtime step is not needed.
    RuntimeNotNeeded { now_ms: u64 },
    /// The home Space exists and is running.
    HomeReady { now_ms: u64, space_id: String },
    /// The coding app is in the home with its session open.
    AgentReady { now_ms: u64 },
    /// A step failed. `network` is a lost or missing connection.
    Failed {
        now_ms: u64,
        message: String,
        #[serde(default)]
        network: bool,
    },
    /// The machine went offline or came back.
    Online { now_ms: u64, online: bool },
    /// Time passed (the host calls it about once a second while waiting).
    Tick { now_ms: u64 },
    /// The person pressed Try again.
    Retry { now_ms: u64 },
    /// The app started again with this record: whatever was in flight is
    /// looked at again.
    Resume { now_ms: u64 },
    /// The home Space is gone (removed, or never made): start that part over.
    HomeGone { now_ms: u64 },
    /// The person sent a message.
    Send {
        now_ms: u64,
        id: String,
        text: String,
        #[serde(default)]
        agent: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        repo: Option<String>,
    },
    /// The host posted a claimed message.
    Delivered { now_ms: u64, id: String },
    /// Posting a claimed message failed.
    DeliveryFailed { now_ms: u64, id: String },
    /// The answer to [`Effect::VerifyDelivery`]: the message is (or is not)
    /// already in the Space's thread.
    ThreadHas {
        now_ms: u64,
        id: String,
        found: bool,
    },
    /// The agent's first words in answer to a message.
    FirstToken { now_ms: u64, id: String },
    /// A person wrote into the home's own thread (not through the box on Home).
    Asked { now_ms: u64 },
}

/// What the host does next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Effect {
    /// Make (or find, or carry on making) the home Space. It is safe to
    /// ask while one is being made: the host follows that one.
    Create,
    /// Put the coding app in the home and open its session.
    AttachAgent,
    /// Post this claimed message to the home's thread, then answer
    /// [`Action::Delivered`] (or [`Action::DeliveryFailed`]).
    Deliver { id: String },
    /// Look for this message in the thread and answer [`Action::ThreadHas`].
    VerifyDelivery { id: String },
    /// Call [`Action::Tick`] at this time.
    WakeAt { at_ms: u64 },
}

/// A step and what the host does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reduced {
    pub state: State,
    pub effects: Vec<Effect>,
}

fn backoff(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(16);
    (RETRY_FIRST_MS << shift).min(RETRY_MAX_MS)
}

/// A step ends. The runtime and the image that moved no bytes were already
/// there: the page says so (skipped) rather than claiming a download.
fn finish(s: &mut State, step: Step, now: u64) {
    let downloads = matches!(step, Step::Runtime | Step::Image);
    let rec = s.rec_mut(step);
    if !matches!(rec.state, StepState::Done | StepState::Skipped) {
        rec.state = if downloads && rec.total == 0 {
            StepState::Skipped
        } else {
            StepState::Done
        };
        rec.finished_ms = Some(now);
        rec.started_ms.get_or_insert(now);
        rec.per_second = None;
    }
}

/// Marks `step` running and every step before it done.
fn enter(s: &mut State, step: Step, now: u64) {
    for earlier in STEPS.iter().copied().filter(|e| e.rank() < step.rank()) {
        finish(s, earlier, now);
    }
    let r = s.rec_mut(step);
    if matches!(
        r.state,
        StepState::Pending | StepState::Waiting | StepState::Failed
    ) {
        r.state = StepState::Running;
        r.started_ms.get_or_insert(now);
    }
}

/// The step a daemon phase belongs to.
fn step_of(phase: &str, detail: &str) -> Option<Step> {
    match phase {
        "preparing" => {
            let d = detail.to_ascii_lowercase();
            if d.contains("runtime") {
                Some(Step::Runtime)
            } else {
                None
            }
        }
        "pulling" => Some(Step::Image),
        "creating" | "booting" | "waiting_for_services" | "connecting" => Some(Step::Home),
        _ => None,
    }
}

fn claim_next(s: &mut State, now: u64, effects: &mut Vec<Effect>) {
    if !s.is_ready() || s.delivering().is_some() || s.home_space_id.is_none() {
        return;
    }
    if let Some(m) = s
        .messages
        .iter_mut()
        .find(|m| m.state == MessageState::Queued)
    {
        m.state = MessageState::Delivering;
        m.claimed_ms = Some(now);
        effects.push(Effect::Deliver { id: m.id.clone() });
    }
}

fn mark_ready_if_done(s: &mut State, now: u64) {
    if s.is_ready() && s.ready_ms.is_none() {
        s.ready_ms = Some(now);
        s.trouble = None;
        s.retry_at_ms = None;
        s.attempt = 0;
        s.failures = 0;
    }
}

/// The failing step: the one running, else the first not done.
fn failing(s: &State) -> Step {
    STEPS
        .iter()
        .copied()
        .find(|st| {
            matches!(
                s.rec(*st).state,
                StepState::Running | StepState::Waiting | StepState::Failed
            )
        })
        .or_else(|| s.current())
        .unwrap_or(Step::Home)
}

/// One action. Pure: the same state and action give the same answer.
pub fn reduce(state: &State, action: &Action) -> Reduced {
    let mut s = state.clone();
    if s.schema != SCHEMA {
        s = State::default();
    }
    let mut fx: Vec<Effect> = Vec::new();
    match action {
        Action::Start {
            now_ms,
            harness,
            pending_id,
        } => {
            if let Some(h) = harness.as_ref().filter(|h| !h.is_empty())
                && !s.wanted
            {
                s.harness = h.clone();
            }
            if !s.wanted {
                s.wanted = true;
                s.started_ms = Some(*now_ms);
                s.pending_id = pending_id.clone().or(s.pending_id.take());
                s.attempt = 0;
                enter(&mut s, Step::Runtime, *now_ms);
                fx.push(Effect::Create);
            }
        }
        Action::OnboardingDone { now_ms } => {
            s.onboarding_done_ms.get_or_insert(*now_ms);
        }
        Action::Phase {
            now_ms,
            phase,
            detail,
            done,
            total,
            per_second,
        } => {
            if phase == "ready" {
                // The create is over; HomeReady says so with the Space's id.
            } else if let Some(step) = step_of(phase, detail)
                .filter(|st| !matches!(s.rec(*st).state, StepState::Done | StepState::Skipped))
            {
                // Progress after a pause is the end of the pause. A report
                // of a step that is done is late and changes nothing.
                s.trouble = None;
                s.retry_at_ms = None;
                enter(&mut s, step, *now_ms);
                let r = s.rec_mut(step);
                r.state = StepState::Running;
                if let Some(d) = done {
                    r.done = *d;
                }
                if let Some(t) = total.filter(|t| *t > 0) {
                    r.total = t;
                }
                r.per_second = *per_second;
            }
        }
        Action::RuntimeNotNeeded { now_ms } => {
            if !matches!(s.runtime.state, StepState::Done | StepState::Skipped) {
                s.runtime.state = StepState::Skipped;
                s.runtime.finished_ms = Some(*now_ms);
            }
        }
        Action::HomeReady { now_ms, space_id } => {
            s.home_space_id = Some(space_id.clone());
            s.pending_id = None;
            s.trouble = None;
            s.retry_at_ms = None;
            for st in [Step::Runtime, Step::Image, Step::Home] {
                finish(&mut s, st, *now_ms);
            }
            if s.agent.state != StepState::Done {
                enter(&mut s, Step::Agent, *now_ms);
                fx.push(Effect::AttachAgent);
            }
        }
        Action::AgentReady { now_ms } => {
            for st in STEPS {
                finish(&mut s, st, *now_ms);
            }
            mark_ready_if_done(&mut s, *now_ms);
            claim_next(&mut s, *now_ms, &mut fx);
        }
        Action::Failed {
            now_ms,
            message,
            network,
        } => {
            let step = failing(&s);
            if *network || !s.online {
                s.attempt = s.attempt.saturating_add(1);
                s.rec_mut(step).state = StepState::Waiting;
                s.rec_mut(step).per_second = None;
                s.trouble = Some(Trouble {
                    network: true,
                    message: message.clone(),
                });
                if s.online {
                    let at = now_ms + backoff(s.attempt);
                    s.retry_at_ms = Some(at);
                    fx.push(Effect::WakeAt { at_ms: at });
                } else {
                    // Offline: nothing to retry until the connection is back.
                    s.retry_at_ms = None;
                }
            } else {
                s.failures += 1;
                s.trouble = Some(Trouble {
                    network: false,
                    message: message.clone(),
                });
                // A refusal the app's own check words (this Mac can't run virtual computers, no
                // engine, no room) does not pass by waiting: it stops at once with its next step.
                if s.failures < OTHER_ATTEMPTS && lasting(message).is_none() {
                    s.rec_mut(step).state = StepState::Waiting;
                    let at = now_ms + backoff(s.failures);
                    s.retry_at_ms = Some(at);
                    fx.push(Effect::WakeAt { at_ms: at });
                } else {
                    s.rec_mut(step).state = StepState::Failed;
                    s.retry_at_ms = None;
                }
            }
        }
        Action::Online { online, .. } => {
            let was = s.online;
            s.online = *online;
            if !online {
                // The retry timer is moot; the step says it is waiting.
                if let Some(step) = s.current().filter(|_| s.wanted && !s.is_ready())
                    && s.rec(step).state == StepState::Running
                {
                    s.rec_mut(step).state = StepState::Waiting;
                }
                if s.wanted && !s.is_ready() {
                    s.trouble.get_or_insert(Trouble {
                        network: true,
                        message: String::new(),
                    });
                }
                s.retry_at_ms = None;
            } else if !was && s.wanted && !s.is_ready() {
                let waiting = matches!(&s.trouble, Some(t) if t.network)
                    || STEPS
                        .iter()
                        .any(|st| s.rec(*st).state == StepState::Waiting);
                if waiting {
                    s.retry_at_ms = None;
                    s.trouble = None;
                    s.attempt = s.attempt.saturating_add(1);
                    let step = failing(&s);
                    s.rec_mut(step).state = StepState::Running;
                    fx.push(Effect::Create);
                }
            }
        }
        Action::Tick { now_ms } => {
            if s.online
                && s.wanted
                && !s.is_ready()
                && s.retry_at_ms.is_some_and(|at| at <= *now_ms)
            {
                s.retry_at_ms = None;
                s.trouble = None;
                let step = failing(&s);
                s.rec_mut(step).state = StepState::Running;
                fx.push(Effect::Create);
            }
            // A message whose post failed waits a little, then goes again.
            if s.delivering().is_none() {
                claim_next(&mut s, *now_ms, &mut fx);
            }
        }
        Action::Retry { now_ms } => {
            if s.wanted && !s.is_ready() {
                s.failures = 0;
                s.attempt = 0;
                s.trouble = None;
                s.retry_at_ms = None;
                let step = failing(&s);
                s.rec_mut(step).state = StepState::Pending;
                enter(&mut s, step, *now_ms);
                fx.push(Effect::Create);
            }
        }
        Action::Resume { now_ms } => {
            if s.wanted && !s.is_ready() {
                // Whatever was running was interrupted: it runs again, from
                // where the host finds it (the Space, the downloaded bytes).
                s.trouble = None;
                s.retry_at_ms = None;
                let step = failing(&s);
                s.rec_mut(step).state = StepState::Pending;
                s.rec_mut(step).per_second = None;
                enter(&mut s, step, *now_ms);
                s.attempt = 0;
                fx.push(Effect::Create);
            } else if s.wanted && s.agent.state == StepState::Done {
                // Ready last time: the host checks the Space is still there
                // with HomeGone, otherwise nothing runs.
            }
            if let Some(m) = s.delivering() {
                fx.push(Effect::VerifyDelivery { id: m.id.clone() });
            }
        }
        Action::HomeGone { now_ms } => {
            if s.wanted {
                s.home_space_id = None;
                s.pending_id = None;
                s.ready_ms = None;
                s.home = StepRecord::default();
                s.agent = StepRecord::default();
                // The image is probably still cached: the create finds out.
                s.attempt = 0;
                s.failures = 0;
                s.trouble = None;
                enter(&mut s, Step::Home, *now_ms);
                // Delivering messages may be lost with the Space: back in the queue.
                for m in &mut s.messages {
                    if m.state == MessageState::Delivering {
                        m.state = MessageState::Queued;
                    }
                }
                fx.push(Effect::Create);
            }
        }
        Action::Send {
            now_ms,
            id,
            text,
            agent,
            model,
            repo,
        } => {
            let text = text.trim();
            if !text.is_empty() && !s.messages.iter().any(|m| &m.id == id) {
                s.asked_ms.get_or_insert(*now_ms);
                s.messages.push(Message {
                    id: id.clone(),
                    text: text.to_string(),
                    agent: agent.clone(),
                    model: model.clone(),
                    repo: repo.clone(),
                    state: MessageState::Queued,
                    attempts: 0,
                    sent_ms: *now_ms,
                    claimed_ms: None,
                    delivered_ms: None,
                    first_token_ms: None,
                });
                // A message before anything started is what starts it.
                if !s.wanted {
                    s.wanted = true;
                    s.started_ms = Some(*now_ms);
                    s.attempt = 0;
                    enter(&mut s, Step::Runtime, *now_ms);
                    fx.push(Effect::Create);
                }
                claim_next(&mut s, *now_ms, &mut fx);
            }
        }
        Action::Delivered { now_ms, id } => {
            if let Some(m) = s
                .messages
                .iter_mut()
                .find(|m| &m.id == id && m.state != MessageState::Delivered)
            {
                m.state = MessageState::Delivered;
                m.delivered_ms = Some(*now_ms);
            }
            claim_next(&mut s, *now_ms, &mut fx);
        }
        Action::DeliveryFailed { now_ms, id } => {
            let mut wake = None;
            if let Some(m) = s
                .messages
                .iter_mut()
                .find(|m| &m.id == id && m.state == MessageState::Delivering)
            {
                m.state = MessageState::Queued;
                m.attempts += 1;
                if m.attempts < DELIVERY_ATTEMPTS {
                    wake = Some(now_ms + backoff(m.attempts));
                }
            }
            if let Some(at) = wake {
                fx.push(Effect::WakeAt { at_ms: at });
            }
        }
        Action::ThreadHas { now_ms, id, found } => {
            if let Some(m) = s
                .messages
                .iter_mut()
                .find(|m| &m.id == id && m.state == MessageState::Delivering)
            {
                if *found {
                    m.state = MessageState::Delivered;
                    m.delivered_ms = Some(*now_ms);
                } else {
                    m.state = MessageState::Queued;
                }
            }
            claim_next(&mut s, *now_ms, &mut fx);
        }
        Action::Asked { now_ms } => {
            s.asked_ms.get_or_insert(*now_ms);
        }
        Action::FirstToken { now_ms, id } => {
            if let Some(m) = s.messages.iter_mut().find(|m| &m.id == id) {
                m.first_token_ms.get_or_insert(*now_ms);
            }
        }
    }
    Reduced {
        state: s,
        effects: fx,
    }
}

// ---------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------

/// A step in the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub id: String,
    pub label: String,
    /// `pending`, `running`, `waiting`, `failed`, `done`, `skipped`.
    pub state: String,
    /// Muted text beside the label, while it has something to say ("120 MB of 440 MB, about 1 minute left").
    pub detail: Option<String>,
    /// 0 to 1 for the step that downloads, when its size is known: the one thin bar of the block, drawn
    /// under the current step only.
    pub fraction: Option<f64>,
}

/// A queued message, as the thread shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedView {
    pub id: String,
    pub text: String,
    /// "Sends when your agent is ready", "Sending", "Sent".
    pub note: String,
}

/// What the page draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// `idle`, `preparing`, `waiting`, `failed` or `ready`.
    pub phase: String,
    /// The heading.
    pub title: String,
    /// A line for what the steps cannot say: the connection is lost, preparing stopped, the
    /// home is ready. None while preparing (the current step's row says it).
    pub line: Option<String>,
    pub steps: Vec<StepView>,
    pub queued: Vec<QueuedView>,
    /// Under the message box while it is not ready.
    pub composer_hint: Option<String>,
    /// Try again, only after a failure that will not clear by itself.
    pub action: Option<ActionView>,
}

/// The failed view's button when this computer can't run Spaces itself:
/// the app opens "Add a computer".
pub const ADD_COMPUTER_ACTION: &str = "add-computer";

/// The app's own check's refusal `message` is, if it is one (its fixed
/// sentences: [`IssueKind::of_text`]). Waiting does not fix any of them.
fn lasting(message: &str) -> Option<IssueKind> {
    IssueKind::of_text(&crate::errors::plain_error(message))
}

/// A button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionView {
    pub id: String,
    pub label: String,
}

/// "120 MB", "1.4 GB": binary units named MB and GB, as the apps size downloads.
pub fn bytes(n: u64) -> String {
    const MIB: f64 = 1_048_576.0;
    const GIB: f64 = 1_073_741_824.0;
    let f = n as f64;
    if f >= GIB {
        format!("{:.1} GB", f / GIB)
    } else if f >= MIB {
        if f / MIB < 10.0 {
            format!("{:.1} MB", f / MIB)
        } else {
            format!("{:.0} MB", f / MIB)
        }
    } else if n >= 1024 {
        format!("{:.0} KB", f / 1024.0)
    } else if n == 0 {
        "0 MB".to_string()
    } else {
        format!("{n} bytes")
    }
}

fn time_left(secs: u64) -> String {
    if secs < 45 {
        "less than a minute left".into()
    } else if secs < 90 {
        "about 1 minute left".into()
    } else if secs < 3600 {
        format!("about {} minutes left", (secs + 30) / 60)
    } else {
        format!("about {} hours left", (secs + 1800) / 3600)
    }
}

fn app_name(harness: &str) -> &'static str {
    match harness {
        "claude-code" => "Claude Code",
        "codex" => "Codex",
        "hermes" => "Hermes",
        "pi" => "Pi",
        "cursor" => "Cursor",
        _ => "your coding agent",
    }
}

fn transfer_line(r: &StepRecord) -> Option<String> {
    if r.total == 0 {
        return None;
    }
    let mut out = format!("{} of {}", bytes(r.done.min(r.total)), bytes(r.total));
    if let Some(rate) = r.per_second.filter(|r| *r > 0) {
        let left = r.total.saturating_sub(r.done) / rate;
        out.push_str(", ");
        out.push_str(&time_left(left));
    }
    Some(out)
}

fn step_detail(s: &State, step: Step) -> Option<String> {
    let r = s.rec(step);
    match r.state {
        StepState::Running => match step {
            Step::Runtime | Step::Image => transfer_line(r).or_else(|| {
                Some(
                    if step == Step::Runtime {
                        "Setting up"
                    } else {
                        "Getting the image"
                    }
                    .into(),
                )
            }),
            Step::Home => Some("Starting".into()),
            Step::Agent => Some("Getting ready".into()),
        },
        StepState::Waiting => Some(if s.trouble.as_ref().is_some_and(|t| t.network) {
            "Waiting for the connection".into()
        } else {
            "Trying again".into()
        }),
        StepState::Failed => Some("Stopped".into()),
        StepState::Skipped => match step {
            Step::Runtime => Some("Already set up".into()),
            Step::Image => Some("Already downloaded".into()),
            _ => None,
        },
        _ => None,
    }
}

fn state_word(st: StepState) -> &'static str {
    match st {
        StepState::Pending => "pending",
        StepState::Running => "running",
        StepState::Waiting => "waiting",
        StepState::Failed => "failed",
        StepState::Done => "done",
        StepState::Skipped => "skipped",
    }
}

fn note_of(m: &Message) -> &'static str {
    match m.state {
        MessageState::Queued => "Sends when your agent is ready",
        MessageState::Delivering => "Sending",
        MessageState::Delivered => "Sent",
    }
}

/// The page for a state. Delivered messages are the thread's, not shown here.
pub fn view(s: &State) -> View {
    let app = app_name(&s.harness);
    let current = s.current();
    let steps: Vec<StepView> = STEPS
        .iter()
        .map(|st| {
            let r = s.rec(*st);
            StepView {
                id: st.id().into(),
                label: match st {
                    Step::Agent => app.to_string(),
                    _ => st.label().to_string(),
                },
                state: state_word(r.state).into(),
                detail: step_detail(s, *st),
                // One thin bar, under the current step that is downloading.
                fraction: (Some(*st) == current && r.state == StepState::Running && r.total > 0)
                    .then(|| (r.done as f64 / r.total as f64).clamp(0.0, 1.0)),
            }
        })
        .collect();
    let queued: Vec<QueuedView> = s
        .messages
        .iter()
        .filter(|m| m.state != MessageState::Delivered)
        .map(|m| QueuedView {
            id: m.id.clone(),
            text: m.text.clone(),
            note: note_of(m).into(),
        })
        .collect();
    let failed = STEPS.iter().any(|st| s.rec(*st).state == StepState::Failed);
    let waiting_net = s.trouble.as_ref().is_some_and(|t| t.network);

    let (phase, title, line): (&str, String, Option<String>) = if !s.wanted {
        ("idle", "Getting your agent ready".into(), None)
    } else if s.is_ready() {
        (
            "ready",
            format!("{app} is ready"),
            Some("Your home Space is running".into()),
        )
    } else if failed {
        // The host's words through the create's own wording: never the SDK's error tags.
        let why = s
            .trouble
            .as_ref()
            .map(|t| crate::spaces::creating::failure_text(&t.message, None, None, false))
            .filter(|m| !m.is_empty())
            .map(|m| format!(": {m}"))
            .unwrap_or_default();
        (
            "failed",
            "Getting your agent ready stopped".into(),
            Some(format!("Preparing stopped{why}")),
        )
    } else if !s.online || waiting_net {
        let line = if !s.online {
            "No internet connection. Preparing carries on when you are back online".to_string()
        } else {
            "The connection dropped. Trying again, nothing downloaded is lost".to_string()
        };
        ("waiting", "Getting your agent ready".into(), Some(line))
    } else {
        ("preparing", "Getting your agent ready".into(), None)
    };

    let composer_hint = match phase {
        "ready" | "idle" => None,
        _ => Some("Your message sends as soon as your agent is ready".to_string()),
    };
    let can_not_run = s
        .trouble
        .as_ref()
        .and_then(|t| lasting(&t.message))
        .is_some_and(|k| k == IssueKind::VirtualizationUnavailable);
    let action = (phase == "failed").then(|| {
        if can_not_run {
            // This computer can't run Spaces itself: the way on is another computer.
            ActionView {
                id: ADD_COMPUTER_ACTION.into(),
                label: "Add a computer".into(),
            }
        } else {
            ActionView {
                id: "retry".into(),
                label: "Try again".into(),
            }
        }
    });
    View {
        phase: phase.into(),
        title,
        line,
        steps,
        queued,
        composer_hint,
        action,
    }
}

// ---------------------------------------------------------------------
// What the lists do not show
// ---------------------------------------------------------------------

/// The Spaces no list shows: the home that warms in the background for Home's prompt box, until a person
/// has written into it. Before that it is invisible infrastructure: not on the Spaces list, in the sidebar,
/// the machine lists, the palette or the Where menu; its own page (reached from Home's preparing line) and
/// that line still show it, and it still counts toward the budget and usage, which are the host's numbers.
///
/// The rule, once, for every app:
/// * it is the home this record prepares (the Space once made, and the row it has while it is made, a
///   failed one too: the error shows on Home, not in a list);
/// * it stops being hidden when a person writes into it: a message sent from Home counts at once, even
///   while it is only queued (the message belongs to that Space and its page says it sends when the agent
///   is ready), and so does one written in the home's own thread ([`Action::Asked`]);
/// * the record is kept across a quit, so a relaunch with a warm home and no message still hides it;
/// * a home this record did not prepare (made before, or by another path) is never hidden.
pub fn hidden_spaces(s: &State) -> Vec<String> {
    if !s.wanted || s.asked_ms.is_some() {
        return Vec::new();
    }
    s.home_space_id
        .iter()
        .chain(s.pending_id.iter())
        .cloned()
        .collect()
}

/// `ids` without the hidden Spaces, in their order: what a list shows.
pub fn listed(s: &State, ids: &[String]) -> Vec<String> {
    let hidden = hidden_spaces(s);
    ids.iter()
        .filter(|i| !hidden.contains(i))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------
// The numbers
// ---------------------------------------------------------------------

/// How long each part took, in milliseconds (none: not reached). Each is
/// what it says and nothing more: preparing time is not a message's latency.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub runtime_ms: Option<u64>,
    pub image_ms: Option<u64>,
    pub home_ms: Option<u64>,
    pub agent_ms: Option<u64>,
    /// From the start of preparing to everything ready.
    pub prepare_ms: Option<u64>,
    /// From the end of onboarding to everything ready: zero when it was
    /// ready before onboarding ended.
    pub onboarding_to_ready_ms: Option<u64>,
    /// Per message: send to the agent session (the post reached the thread),
    /// and send to the agent's first words.
    pub messages: Vec<MessageMetrics>,
}

/// One message's numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMetrics {
    pub id: String,
    pub send_to_delivered_ms: Option<u64>,
    pub send_to_first_token_ms: Option<u64>,
}

fn span(r: &StepRecord) -> Option<u64> {
    Some(r.finished_ms?.saturating_sub(r.started_ms?))
}

/// The numbers of a state.
pub fn metrics(s: &State) -> Metrics {
    Metrics {
        runtime_ms: span(&s.runtime),
        image_ms: span(&s.image),
        home_ms: span(&s.home),
        agent_ms: span(&s.agent),
        prepare_ms: s
            .started_ms
            .zip(s.ready_ms)
            .map(|(a, b)| b.saturating_sub(a)),
        onboarding_to_ready_ms: s
            .onboarding_done_ms
            .zip(s.ready_ms)
            .map(|(a, b)| b.saturating_sub(a)),
        messages: s
            .messages
            .iter()
            .map(|m| MessageMetrics {
                id: m.id.clone(),
                send_to_delivered_ms: m.delivered_ms.map(|d| d.saturating_sub(m.sent_ms)),
                send_to_first_token_ms: m.first_token_ms.map(|d| d.saturating_sub(m.sent_ms)),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(s: &State, a: Action) -> Reduced {
        reduce(s, &a)
    }

    fn start() -> State {
        go(
            &State::default(),
            Action::Start {
                now_ms: 1_000,
                harness: Some("claude-code".into()),
                pending_id: Some("pending-1".into()),
            },
        )
        .state
    }

    fn ready() -> State {
        let s = start();
        let s = go(
            &s,
            Action::HomeReady {
                now_ms: 9_000,
                space_id: "local:home-1".into(),
            },
        )
        .state;
        go(&s, Action::AgentReady { now_ms: 10_000 }).state
    }

    #[test]
    fn start_begins_the_runtime_once_and_creates() {
        let r = go(
            &State::default(),
            Action::Start {
                now_ms: 5,
                harness: None,
                pending_id: None,
            },
        );
        assert_eq!(r.effects, vec![Effect::Create]);
        assert_eq!(r.state.runtime.state, StepState::Running);
        assert_eq!(r.state.started_ms, Some(5));
        // Asking again changes nothing and starts nothing.
        let again = go(
            &r.state,
            Action::Start {
                now_ms: 99,
                harness: Some("codex".into()),
                pending_id: None,
            },
        );
        assert_eq!(again.state, r.state);
        assert!(again.effects.is_empty());
    }

    #[test]
    fn phases_walk_the_steps_and_finish_the_earlier_ones() {
        let s = start();
        let s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "preparing".into(),
                detail: "Setting up the Linux runtime".into(),
                done: Some(100),
                total: Some(400),
                per_second: Some(10),
            },
        )
        .state;
        assert_eq!(s.runtime.state, StepState::Running);
        assert_eq!((s.runtime.done, s.runtime.total), (100, 400));
        let s = go(
            &s,
            Action::Phase {
                now_ms: 3_000,
                phase: "pulling".into(),
                detail: "ghcr.io/trycua/linux:24.04-slim".into(),
                done: Some(1),
                total: Some(300),
                per_second: None,
            },
        )
        .state;
        assert_eq!(s.runtime.state, StepState::Done);
        assert_eq!(s.image.state, StepState::Running);
        let s = go(
            &s,
            Action::Phase {
                now_ms: 4_000,
                phase: "booting".into(),
                detail: String::new(),
                done: None,
                total: None,
                per_second: None,
            },
        )
        .state;
        assert_eq!(s.image.state, StepState::Done);
        assert_eq!(s.home.state, StepState::Running);
        // A later report of an earlier phase does not move a step backwards.
        let s = go(
            &s,
            Action::Phase {
                now_ms: 4_500,
                phase: "pulling".into(),
                detail: String::new(),
                done: Some(5),
                total: Some(300),
                per_second: None,
            },
        )
        .state;
        assert_eq!(s.image.state, StepState::Done);
        assert_eq!(s.home.state, StepState::Running);
    }

    #[test]
    fn lost_connection_waits_with_backoff_and_resumes_when_online() {
        let s = start();
        let r = go(
            &s,
            Action::Failed {
                now_ms: 10_000,
                message: "connection reset".into(),
                network: true,
            },
        );
        assert_eq!(r.state.runtime.state, StepState::Waiting);
        assert_eq!(r.effects, vec![Effect::WakeAt { at_ms: 12_000 }]);
        // Too early: nothing.
        assert!(
            go(&r.state, Action::Tick { now_ms: 11_000 })
                .effects
                .is_empty()
        );
        // On time: create again (the host resumes the transfer).
        let t = go(&r.state, Action::Tick { now_ms: 12_000 });
        assert_eq!(t.effects, vec![Effect::Create]);
        assert_eq!(t.state.runtime.state, StepState::Running);
        // Failing again waits longer, up to the cap.
        let mut s = t.state;
        let mut last = 0;
        for i in 0..12 {
            let r = go(
                &s,
                Action::Failed {
                    now_ms: 100_000 + i,
                    message: "x".into(),
                    network: true,
                },
            );
            let Effect::WakeAt { at_ms } = r.effects[0] else {
                panic!()
            };
            last = at_ms - (100_000 + i);
            s = go(&r.state, Action::Tick { now_ms: at_ms }).state;
        }
        assert_eq!(last, RETRY_MAX_MS);
        // Network trouble never ends in Failed.
        assert_ne!(s.runtime.state, StepState::Failed);
    }

    #[test]
    fn offline_waits_without_a_timer_and_resumes_on_online() {
        let s = start();
        let off = go(
            &s,
            Action::Online {
                now_ms: 2_000,
                online: false,
            },
        );
        assert_eq!(off.state.runtime.state, StepState::Waiting);
        assert!(off.effects.is_empty());
        let v = view(&off.state);
        assert_eq!(v.phase, "waiting");
        assert!(
            v.line
                .as_deref()
                .unwrap()
                .starts_with("No internet connection")
        );
        // A failure while offline sets no timer either.
        let f = go(
            &off.state,
            Action::Failed {
                now_ms: 2_500,
                message: "dns".into(),
                network: true,
            },
        );
        assert_eq!(f.state.retry_at_ms, None);
        assert!(f.effects.is_empty());
        let on = go(
            &f.state,
            Action::Online {
                now_ms: 9_000,
                online: true,
            },
        );
        assert_eq!(on.effects, vec![Effect::Create]);
        assert_eq!(on.state.runtime.state, StepState::Running);
        assert_eq!(view(&on.state).phase, "preparing");
    }

    #[test]
    /// CUA-3193: a Mac that can't run Spaces stopped on "Preparing stopped: local runtime: ...",
    /// after waiting to try again, with Try again as the only way on.
    fn a_mac_that_cannot_run_spaces_stops_at_once_in_plain_words() {
        let mac = cua_create_failure::preflight::MAC_UNSUPPORTED;
        for raw in [
            format!("local runtime: {mac}"),
            format!("local runtime: [local] {mac}"),
        ] {
            let r = go(
                &start(),
                Action::Failed {
                    now_ms: 1,
                    message: raw.clone(),
                    network: false,
                },
            );
            assert_eq!(r.state.runtime.state, StepState::Failed, "{raw}");
            assert!(
                r.effects
                    .iter()
                    .all(|e| !matches!(e, Effect::WakeAt { .. }))
            );
            let v = view(&r.state);
            let line = v.line.clone().unwrap();
            assert_eq!(line, format!("Preparing stopped: {mac}"));
            for internal in crate::errors::INTERNAL_TAGS {
                assert!(!line.contains(internal.trim()), "{line}");
            }
            let action = v.action.unwrap();
            assert_eq!(
                (action.id.as_str(), action.label.as_str()),
                (ADD_COMPUTER_ACTION, "Add a computer")
            );
        }
    }

    #[test]
    fn other_failures_retry_once_then_offer_try_again() {
        let s = start();
        let r = go(
            &s,
            Action::Failed {
                now_ms: 1,
                message: "the disk is full".into(),
                network: false,
            },
        );
        assert_eq!(r.state.runtime.state, StepState::Waiting);
        let t = go(&r.state, Action::Tick { now_ms: 5_000 });
        assert_eq!(t.effects, vec![Effect::Create]);
        let r2 = go(
            &t.state,
            Action::Failed {
                now_ms: 6_000,
                message: "the disk is full".into(),
                network: false,
            },
        );
        assert_eq!(r2.state.runtime.state, StepState::Failed);
        let v = view(&r2.state);
        assert_eq!(v.phase, "failed");
        assert_eq!(
            v.line.as_deref(),
            Some("Preparing stopped: the disk is full")
        );
        assert_eq!(v.action.as_ref().unwrap().label, "Try again");
        // Try again runs it again from the failed step.
        let again = go(&r2.state, Action::Retry { now_ms: 7_000 });
        assert_eq!(again.effects, vec![Effect::Create]);
        assert_eq!(again.state.runtime.state, StepState::Running);
        assert!(view(&again.state).action.is_none());
    }

    #[test]
    fn quit_mid_prepare_resumes_from_the_failing_step_not_the_start() {
        let mut s = start();
        s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "pulling".into(),
                detail: String::new(),
                done: Some(50),
                total: Some(300),
                per_second: Some(5),
            },
        )
        .state;
        // Saved, the app quit, started again.
        let saved = serde_json::to_string(&s).unwrap();
        let loaded: State = serde_json::from_str(&saved).unwrap();
        let r = go(&loaded, Action::Resume { now_ms: 50_000 });
        assert_eq!(r.effects, vec![Effect::Create]);
        assert_eq!(
            r.state.runtime.state,
            StepState::Skipped,
            "a runtime that moved no bytes was already there"
        );
        assert_eq!(r.state.image.state, StepState::Running);
        assert_eq!(r.state.started_ms, Some(1_000), "the clock is not reset");
        // A ready state does nothing on resume.
        let ok = go(&ready(), Action::Resume { now_ms: 60_000 });
        assert!(ok.effects.is_empty());
    }

    #[test]
    fn a_message_queues_until_ready_then_is_delivered_once() {
        let s = start();
        let sent = go(
            &s,
            Action::Send {
                now_ms: 2_000,
                id: "m1".into(),
                text: "  fix the failing tests ".into(),
                agent: "claude-code".into(),
                model: Some("claude-sonnet-5".into()),
                repo: None,
            },
        );
        assert!(sent.effects.is_empty(), "nothing is posted before ready");
        assert_eq!(sent.state.messages[0].text, "fix the failing tests");
        let v = view(&sent.state);
        assert_eq!(v.queued.len(), 1);
        assert_eq!(v.queued[0].note, "Sends when your agent is ready");
        assert!(v.composer_hint.is_some());
        // The same id again is the same message.
        let dup = go(
            &sent.state,
            Action::Send {
                now_ms: 2_100,
                id: "m1".into(),
                text: "fix the failing tests".into(),
                agent: "claude-code".into(),
                model: None,
                repo: None,
            },
        );
        assert_eq!(dup.state.messages.len(), 1);
        // Ready: the first is claimed and delivered.
        let home = go(
            &dup.state,
            Action::HomeReady {
                now_ms: 8_000,
                space_id: "local:home".into(),
            },
        );
        assert_eq!(home.effects, vec![Effect::AttachAgent]);
        let agent = go(&home.state, Action::AgentReady { now_ms: 9_000 });
        assert_eq!(agent.effects, vec![Effect::Deliver { id: "m1".into() }]);
        assert_eq!(agent.state.messages[0].state, MessageState::Delivering);
        // Ticks and a second AgentReady do not claim it again.
        assert!(
            go(&agent.state, Action::Tick { now_ms: 9_500 })
                .effects
                .is_empty()
        );
        assert!(
            go(&agent.state, Action::AgentReady { now_ms: 9_600 })
                .effects
                .is_empty()
        );
        let done = go(
            &agent.state,
            Action::Delivered {
                now_ms: 9_700,
                id: "m1".into(),
            },
        );
        assert_eq!(done.state.messages[0].state, MessageState::Delivered);
        assert!(done.effects.is_empty());
        // Delivered twice is still one delivery.
        let twice = go(
            &done.state,
            Action::Delivered {
                now_ms: 9_900,
                id: "m1".into(),
            },
        );
        assert_eq!(twice.state.messages[0].delivered_ms, Some(9_700));
        assert!(view(&twice.state).queued.is_empty());
    }

    #[test]
    fn messages_go_one_at_a_time_in_order() {
        let mut s = ready();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            s = go(
                &s,
                Action::Send {
                    now_ms: 20_000 + i as u64,
                    id: (*id).into(),
                    text: format!("job {id}"),
                    agent: "claude-code".into(),
                    model: None,
                    repo: None,
                },
            )
            .state;
        }
        assert_eq!(
            s.messages.iter().map(|m| m.state).collect::<Vec<_>>(),
            vec![
                MessageState::Delivering,
                MessageState::Queued,
                MessageState::Queued
            ],
            "ready: the first goes at once, the others wait their turn"
        );
        let r = go(
            &s,
            Action::Delivered {
                now_ms: 21_000,
                id: "a".into(),
            },
        );
        assert_eq!(r.effects, vec![Effect::Deliver { id: "b".into() }]);
    }

    #[test]
    fn quit_while_delivering_checks_the_thread_before_posting_again() {
        let mut s = ready();
        s = go(
            &s,
            Action::Send {
                now_ms: 20_000,
                id: "m1".into(),
                text: "hello".into(),
                agent: "claude-code".into(),
                model: None,
                repo: None,
            },
        )
        .state;
        assert_eq!(s.messages[0].state, MessageState::Delivering);
        let loaded: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let r = go(&loaded, Action::Resume { now_ms: 90_000 });
        assert_eq!(r.effects, vec![Effect::VerifyDelivery { id: "m1".into() }]);
        // It was posted before the quit: delivered, not posted again.
        let found = go(
            &r.state,
            Action::ThreadHas {
                now_ms: 90_100,
                id: "m1".into(),
                found: true,
            },
        );
        assert_eq!(found.state.messages[0].state, MessageState::Delivered);
        assert!(found.effects.is_empty());
        // It was not: back to the queue and delivered now.
        let missing = go(
            &r.state,
            Action::ThreadHas {
                now_ms: 90_100,
                id: "m1".into(),
                found: false,
            },
        );
        assert_eq!(missing.effects, vec![Effect::Deliver { id: "m1".into() }]);
    }

    #[test]
    fn a_failed_post_goes_back_and_is_retried_with_a_wait() {
        let mut s = ready();
        s = go(
            &s,
            Action::Send {
                now_ms: 20_000,
                id: "m1".into(),
                text: "hello".into(),
                agent: "claude-code".into(),
                model: None,
                repo: None,
            },
        )
        .state;
        let f = go(
            &s,
            Action::DeliveryFailed {
                now_ms: 20_100,
                id: "m1".into(),
            },
        );
        assert_eq!(f.state.messages[0].state, MessageState::Queued);
        assert_eq!(f.state.messages[0].attempts, 1);
        assert_eq!(f.effects, vec![Effect::WakeAt { at_ms: 22_100 }]);
        let t = go(&f.state, Action::Tick { now_ms: 22_100 });
        assert_eq!(t.effects, vec![Effect::Deliver { id: "m1".into() }]);
    }

    #[test]
    fn a_removed_home_is_made_again_and_claimed_messages_are_not_lost() {
        let mut s = ready();
        s = go(
            &s,
            Action::Send {
                now_ms: 20_000,
                id: "m1".into(),
                text: "hello".into(),
                agent: "claude-code".into(),
                model: None,
                repo: None,
            },
        )
        .state;
        let r = go(&s, Action::HomeGone { now_ms: 30_000 });
        assert_eq!(r.effects, vec![Effect::Create]);
        assert_eq!(r.state.messages[0].state, MessageState::Queued);
        assert_eq!(r.state.home.state, StepState::Running);
        assert_eq!(r.state.runtime.state, StepState::Skipped, "kept as it was");
        assert_eq!(r.state.ready_ms, None);
        assert_eq!(view(&r.state).phase, "preparing");
    }

    #[test]
    fn steps_that_downloaded_are_done_and_steps_that_did_not_are_already_there() {
        let s = start();
        let s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "preparing".into(),
                detail: "Setting up the Linux runtime".into(),
                done: Some(10),
                total: Some(400),
                per_second: None,
            },
        )
        .state;
        // The image never reported bytes (cached): the home's boot ends both.
        let s = go(
            &s,
            Action::Phase {
                now_ms: 3_000,
                phase: "booting".into(),
                detail: String::new(),
                done: None,
                total: None,
                per_second: None,
            },
        )
        .state;
        assert_eq!(s.runtime.state, StepState::Done);
        assert_eq!(s.image.state, StepState::Skipped);
        let v = view(&s);
        assert_eq!(v.steps[1].detail.as_deref(), Some("Already downloaded"));
        assert_eq!(v.steps[0].detail, None);
    }

    #[test]
    fn a_machine_with_its_own_engine_skips_the_runtime() {
        let s = start();
        let s = go(&s, Action::RuntimeNotNeeded { now_ms: 1_500 }).state;
        assert_eq!(s.runtime.state, StepState::Skipped);
        let v = view(&s);
        assert_eq!(v.steps[0].state, "skipped");
        // The next step runs; the skipped one is never marked done later.
        let s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "pulling".into(),
                detail: String::new(),
                done: Some(1),
                total: Some(10),
                per_second: None,
            },
        )
        .state;
        assert_eq!(s.runtime.state, StepState::Skipped);
        assert_eq!(s.image.state, StepState::Running);
    }

    #[test]
    fn a_message_before_anything_started_starts_it() {
        let r = go(
            &State::default(),
            Action::Send {
                now_ms: 5,
                id: "m".into(),
                text: "go".into(),
                agent: String::new(),
                model: None,
                repo: None,
            },
        );
        assert!(r.state.wanted);
        assert_eq!(r.effects, vec![Effect::Create]);
    }

    #[test]
    fn metrics_say_what_each_span_is() {
        let mut s = start();
        s = go(&s, Action::OnboardingDone { now_ms: 6_000 }).state;
        s = go(
            &s,
            Action::Send {
                now_ms: 7_000,
                id: "m1".into(),
                text: "hi".into(),
                agent: "claude-code".into(),
                model: None,
                repo: None,
            },
        )
        .state;
        s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "pulling".into(),
                detail: String::new(),
                done: None,
                total: None,
                per_second: None,
            },
        )
        .state;
        s = go(
            &s,
            Action::HomeReady {
                now_ms: 9_000,
                space_id: "h".into(),
            },
        )
        .state;
        s = go(&s, Action::AgentReady { now_ms: 10_000 }).state;
        s = go(
            &s,
            Action::Delivered {
                now_ms: 10_050,
                id: "m1".into(),
            },
        )
        .state;
        s = go(
            &s,
            Action::FirstToken {
                now_ms: 11_200,
                id: "m1".into(),
            },
        )
        .state;
        let m = metrics(&s);
        assert_eq!(m.prepare_ms, Some(9_000));
        assert_eq!(m.onboarding_to_ready_ms, Some(4_000));
        assert_eq!(m.messages[0].send_to_delivered_ms, Some(3_050));
        assert_eq!(m.messages[0].send_to_first_token_ms, Some(4_200));
        // Ready before onboarding ended: zero, never negative.
        let mut early = ready();
        early = go(&early, Action::OnboardingDone { now_ms: 99_000 }).state;
        assert_eq!(metrics(&early).onboarding_to_ready_ms, Some(0));
    }

    #[test]
    fn every_step_has_a_mark_and_only_the_current_one_a_bar() {
        let mut s = start();
        // Running with no size yet: a word, no bar.
        let v = view(&s);
        assert_eq!(v.steps[0].detail.as_deref(), Some("Setting up"));
        assert!(v.steps.iter().all(|x| x.fraction.is_none()));
        s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "pulling".into(),
                detail: String::new(),
                done: Some(1 << 20),
                total: Some(300 << 20),
                per_second: None,
            },
        )
        .state;
        let v = view(&s);
        let bars: Vec<_> = v
            .steps
            .iter()
            .filter(|x| x.fraction.is_some())
            .map(|x| x.id.as_str())
            .collect();
        assert_eq!(bars, ["image"]);
        assert_eq!(
            v.steps[2].detail, None,
            "a waiting step is just the dot and the label"
        );
        assert_eq!(
            v.steps[0].detail.as_deref(),
            Some("Already set up"),
            "a done step is a check"
        );
    }

    #[test]
    fn a_state_of_another_schema_starts_over() {
        let mut old = start();
        old.schema = 0;
        let r = go(&old, Action::Tick { now_ms: 1 });
        assert_eq!(r.state, State::default());
    }

    #[test]
    fn the_words_have_no_middle_dots_and_the_honest_progress_line() {
        let s = start();
        let s = go(
            &s,
            Action::Phase {
                now_ms: 2_000,
                phase: "preparing".into(),
                detail: "Setting up the Linux runtime".into(),
                done: Some(120 << 20),
                total: Some(440 << 20),
                per_second: Some(5 << 20),
            },
        )
        .state;
        let v = view(&s);
        // One quiet block: no headline sentence repeating the row, the row says it, one thin bar.
        assert_eq!(v.line, None);
        assert_eq!(
            v.steps[0].detail.as_deref(),
            Some("120 MB of 440 MB, about 1 minute left")
        );
        assert!((v.steps[0].fraction.unwrap() - 120.0 / 440.0).abs() < 1e-9);
        assert!(
            v.steps[1..].iter().all(|x| x.fraction.is_none()),
            "a bar under the current step only"
        );
        let all = serde_json::to_string(&v).unwrap();
        assert!(!all.contains('\u{b7}'));
        for st in [
            view(&State::default()),
            view(&ready()),
            view(
                &go(
                    &s,
                    Action::Online {
                        now_ms: 3,
                        online: false,
                    },
                )
                .state,
            ),
        ] {
            assert!(!serde_json::to_string(&st).unwrap().contains('\u{b7}'));
        }
        assert_eq!(view(&ready()).title, "Claude Code is ready");
        assert!(view(&ready()).composer_hint.is_none());
    }
}
