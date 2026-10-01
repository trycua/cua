// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The roster of Bots and their threads, shared by the shell's commands, the
//! routine scheduler and group chats (the Swift app's `BotStore`).
//!
//! Routines and group chats are the SDK's model (`cua_spaces::routines`,
//! `cua_spaces::groups`); this module is only the live seam: [`Bots`]
//! implements `RoutineRunner` and `GroupMessenger` over the Bots' real agent
//! threads in the one shared Space, with the Swift app's semantics:
//!
//! - a routine firing on a Bot mid-turn is **refused** (the slot is used);
//!   a Bot with a thread that takes a message gets another turn on it; any
//!   other Bot is hired with the routine's text;
//! - a group message hires a member without a thread, and is refused (and
//!   shown) for a member mid-turn.

use crate::thread::{BotThread, Line, Speaker};
use crate::{Core, Result};
use cua_spaces::agents::{Endpoint, RunOptions, RunStatus};
use cua_spaces::groups::{GroupDelivery, GroupMessenger};
use cua_spaces::routines::{Routine, RoutineFiring, RoutineRunner};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// A Bot as the shell names it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BotInfo {
    pub id: String,
    pub name: String,
    pub agent: String,
    /// The Space it works in.
    pub space: String,
}

/// A thread as the shell renders it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ThreadView {
    pub run_id: Option<String>,
    pub status: Option<String>,
    pub accepts_message: bool,
    pub transcript: Vec<Line>,
    /// The roster preview: the agent's last message on one line (the SDK's
    /// `Transcript::preview`; activity never becomes a preview).
    pub preview: Option<String>,
}

impl ThreadView {
    fn of(t: &BotThread) -> Self {
        Self {
            run_id: t.run_id().map(str::to_string),
            status: t.last_status().map(|s| s.status.as_str().to_string()),
            accepts_message: t.last_status().is_none_or(|s| s.accepts_message),
            transcript: t.transcript(),
            preview: t.preview(),
        }
    }
}

/// Agent start options for every run this roster starts: a custom model
/// endpoint and the provider key variables forwarded from this process.
#[derive(Clone, Debug, Default)]
pub struct AgentOptions {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub env_from_host: Vec<String>,
}

impl AgentOptions {
    /// `OPENKOALABOTS_MODEL_URL`, `OPENKOALABOTS_MODEL`, and
    /// `OPENKOALABOTS_MODEL_KEY_VAR` (a provider key variable to forward).
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            base_url: var("OPENKOALABOTS_MODEL_URL"),
            model: var("OPENKOALABOTS_MODEL"),
            env_from_host: var("OPENKOALABOTS_MODEL_KEY_VAR").into_iter().collect(),
        }
    }

    /// The run options: the endpoint, and the forwarded keys (a missing or
    /// disallowed variable is an error, never guessed).
    pub fn run_options(&self) -> Result<RunOptions> {
        let (env, missing) = cua_spaces::agents::env_from_host(&self.env_from_host)?;
        if !missing.is_empty() {
            return Err(crate::Error::Invalid(format!(
                "not set in this process: {}",
                missing.join(", ")
            )));
        }
        Ok(RunOptions {
            env,
            endpoint: self.base_url.clone().map(|base_url| Endpoint {
                base_url,
                wire: None,
                model: self.model.clone(),
            }),
            model: self.model.clone(),
            ..Default::default()
        })
    }
}

/// A Bot's avatar color, the same as its cursor: while the Bot is present
/// on the Space, the color the server assigned it (a requested color is kept
/// unless another participant already holds it), read from the presence
/// roster; before that (or with no roster), its stable presence color, the
/// one it requests when it joins. The shell's `botAvatarColor` is the
/// TypeScript twin, fed from the same roster's avatars.
pub fn avatar_color(roster: Option<&cua_spaces::presence::Roster>, bot_id: &str) -> String {
    match roster {
        Some(r) => r.color_of(bot_id).to_lowercase(),
        None => cua_spaces::presence::color_for(bot_id).to_string(),
    }
}

/// The reply of a Bot's latest turn: its agent lines (never the prompt,
/// tool notices or turn markers), `None` before it has said anything.
pub fn latest_reply(transcript: &[Line]) -> Option<String> {
    let turn = transcript.iter().map(|l| l.turn).max()?;
    let text = transcript
        .iter()
        .filter(|l| l.turn == turn && l.speaker == Speaker::Agent)
        .map(|l| l.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

#[derive(Default)]
struct Inner {
    info: HashMap<String, BotInfo>,
    threads: HashMap<String, BotThread>,
}

/// Every Bot and its thread.
pub struct Bots {
    core: Core,
    options: AgentOptions,
    inner: Mutex<Inner>,
}

impl Bots {
    pub fn new(core: Core, options: AgentOptions) -> Self {
        Self {
            core,
            options,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Adds or renames Bots (the shell's list).
    pub async fn upsert(&self, bots: Vec<BotInfo>) {
        let mut g = self.inner.lock().await;
        for b in bots {
            g.info.insert(b.id.clone(), b);
        }
    }

    pub async fn info(&self, bot: &str) -> Option<BotInfo> {
        self.inner.lock().await.info.get(bot).cloned()
    }

    /// Sends a user turn: the first starts the Bot's run, later ones go to it.
    pub async fn send(&self, bot: &str, text: &str) -> Result<ThreadView> {
        let mut g = self.inner.lock().await;
        if !g.threads.contains_key(bot) {
            let info = g
                .info
                .get(bot)
                .cloned()
                .ok_or_else(|| crate::Error::Invalid(format!("no Bot {bot}")))?;
            let t = BotThread::new(&self.core.space(&info.space).await?, &info.agent)
                .await?
                .with_options(self.options.run_options()?);
            g.threads.insert(bot.to_string(), t);
        }
        let t = g.threads.get_mut(bot).expect("inserted");
        let sent = t.send(text).await;
        let v = ThreadView::of(t);
        sent.map(|_| v)
    }

    /// One poll of a started thread.
    pub async fn poll(&self, bot: &str) -> Result<ThreadView> {
        let mut g = self.inner.lock().await;
        let t = g
            .threads
            .get_mut(bot)
            .ok_or_else(|| crate::Error::Invalid(format!("no thread for {bot}")))?;
        if t.run_id().is_some() {
            t.poll().await?;
        }
        Ok(ThreadView::of(t))
    }

    /// The thread's current view, without a poll.
    pub async fn view(&self, bot: &str) -> Option<ThreadView> {
        self.inner.lock().await.threads.get(bot).map(ThreadView::of)
    }

    /// Waits for a Bot's current turn (see [`BotThread::wait_turn`]).
    pub async fn wait_turn(
        &self,
        bot: &str,
        expect: Option<&str>,
        every: std::time::Duration,
        max_polls: u32,
    ) -> Result<ThreadView> {
        let mut g = self.inner.lock().await;
        let t = g
            .threads
            .get_mut(bot)
            .ok_or_else(|| crate::Error::Invalid(format!("no thread for {bot}")))?;
        t.wait_turn(expect, every, max_polls).await?;
        Ok(ThreadView::of(t))
    }

    /// Stops every run this roster started.
    pub async fn stop_all(&self) {
        let mut g = self.inner.lock().await;
        for t in g.threads.values_mut() {
            let _ = t.stop().await;
        }
    }

    /// `(has a started thread, last status, accepts a message)` after a fresh poll.
    async fn state(&self, bot: &str) -> (bool, Option<RunStatus>, bool) {
        let mut g = self.inner.lock().await;
        match g.threads.get_mut(bot) {
            Some(t) if t.run_id().is_some() => {
                let _ = t.poll().await;
                let s = t.last_status();
                (
                    true,
                    s.map(|s| s.status),
                    s.is_none_or(|s| s.accepts_message),
                )
            }
            _ => (false, None, false),
        }
    }

    async fn name(&self, bot: &str) -> String {
        self.info(bot)
            .await
            .map(|b| b.name)
            .unwrap_or_else(|| bot.to_string())
    }

    /// Replaces a Bot's thread with a fresh one and sends `text` (a hire).
    async fn hire(&self, bot: &str, text: &str) -> Result<String> {
        self.inner.lock().await.threads.remove(bot);
        let v = self.send(bot, text).await?;
        Ok(v.run_id.unwrap_or_default())
    }
}

#[async_trait::async_trait]
impl RoutineRunner for Bots {
    async fn fire(&self, routine: &Routine) -> RoutineFiring {
        let text = routine.turn_text();
        let (started, status, accepts) = self.state(&routine.bot_id).await;
        if status == Some(RunStatus::Running) {
            return RoutineFiring::Refused {
                reason: format!("{} is mid-turn; the slot was skipped", routine.bot_id),
            };
        }
        let result = if started && accepts {
            self.send(&routine.bot_id, &text)
                .await
                .map(|v| v.run_id.unwrap_or_default())
        } else {
            self.hire(&routine.bot_id, &text).await
        };
        match result {
            Ok(run_id) => RoutineFiring::Started { run_id },
            Err(e) => RoutineFiring::Failed {
                reason: e.to_string(),
            },
        }
    }
}

#[async_trait::async_trait]
impl GroupMessenger for Bots {
    async fn deliver(&self, text: &str, bot_id: &str) -> GroupDelivery {
        let (started, status, _) = self.state(bot_id).await;
        let d = |accepted: bool, reason: String| GroupDelivery {
            bot_id: bot_id.to_string(),
            accepted,
            reason,
        };
        if !started {
            return match self.hire(bot_id, text).await {
                Ok(_) => d(true, "started for this group".into()),
                Err(e) => d(false, e.to_string()),
            };
        }
        if status == Some(RunStatus::Running) {
            let name = self.name(bot_id).await;
            return d(
                false,
                format!("{name} is mid-turn; a message is refused rather than queued"),
            );
        }
        match self.send(bot_id, text).await {
            Ok(_) => d(true, "delivered".into()),
            Err(e) => d(false, e.to_string()),
        }
    }

    async fn latest_reply(&self, bot_id: &str) -> Option<String> {
        let mut g = self.inner.lock().await;
        let t = g.threads.get_mut(bot_id)?;
        if t.run_id().is_some() {
            let _ = t.poll().await;
        }
        latest_reply(&t.transcript())
    }

    async fn is_working(&self, bot_id: &str) -> bool {
        let g = self.inner.lock().await;
        g.threads
            .get(bot_id)
            .and_then(|t| t.last_status())
            .is_some_and(|s| s.status == RunStatus::Running)
    }

    async fn display_name(&self, bot_id: &str) -> String {
        self.name(bot_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bots_avatar_is_its_presence_cursor_color() {
        for bot in ["ada", "bo", "openkoalabots-koala-1"] {
            // The color a Bot's cursor requests when it joins presence.
            let cursor = cua_spaces::presence::Identity::agent(bot, bot).color;
            assert_eq!(avatar_color(None, bot), cursor, "{bot}");
        }
        assert_eq!(
            avatar_color(None, "ada"),
            "#bcf60c",
            "pinned to the TypeScript twin"
        );
    }

    /// The operator holds Ada's stable color, so the server gave Ada another:
    /// Ada's avatar follows her cursor, and falls back once she leaves.
    #[test]
    fn the_avatar_follows_the_server_assigned_color() {
        use cua_spaces::presence::{Participant, PresenceEvent, Roster};
        let stable = cua_spaces::presence::color_for("ada");
        let p = |pid: &str, principal: &str, color: &str, kind: &str| Participant {
            participant_id: pid.into(),
            principal_id: principal.into(),
            display_name: principal.into(),
            color: color.into(),
            kind: kind.into(),
        };
        let mut r = Roster {
            me: "me".into(),
            entries: vec![(p("me", "operator", stable, "human"), None)],
        };
        r.apply(&PresenceEvent::Joined {
            participant: p("p-ada", "ada", "#123456", "agent"),
        });
        let cursor = crate::presence::avatars(&r)
            .into_iter()
            .find(|a| a.principal_id == "ada")
            .unwrap()
            .color;
        assert_eq!(avatar_color(Some(&r), "ada"), cursor);
        assert_eq!(cursor, "#123456");
        r.apply(&PresenceEvent::Left {
            participant_id: "p-ada".into(),
            reason: String::new(),
        });
        assert_eq!(avatar_color(Some(&r), "ada"), stable);
    }

    fn l(turn: usize, speaker: Speaker, text: &str) -> Line {
        Line {
            turn,
            speaker,
            text: text.into(),
            steps: vec![],
        }
    }

    #[test]
    fn the_latest_reply_is_the_last_turns_agent_text_only() {
        assert_eq!(latest_reply(&[]), None);
        let t = vec![
            l(1, Speaker::User, "[routine] Check-in: mock: say one"),
            l(1, Speaker::Agent, "one"),
            l(2, Speaker::User, "again"),
            l(2, Speaker::Activity, "1 step"),
        ];
        assert_eq!(latest_reply(&t), None, "turn 2 has not answered yet");
        let mut t = t;
        t.push(l(2, Speaker::Agent, " two \n"));
        t.push(l(2, Speaker::Agent, "three"));
        assert_eq!(latest_reply(&t).as_deref(), Some("two\nthree"));
    }

    #[test]
    fn agent_options_forward_only_named_keys_and_the_endpoint() {
        let none = AgentOptions::default().run_options().unwrap();
        assert!(none.endpoint.is_none() && none.env.is_empty());
        let o = AgentOptions {
            base_url: Some("http://mock:8787".into()),
            model: Some("claude-mock-1".into()),
            env_from_host: vec![],
        }
        .run_options()
        .unwrap();
        let e = o.endpoint.unwrap();
        assert_eq!(e.base_url, "http://mock:8787");
        assert_eq!(e.model.as_deref(), Some("claude-mock-1"));
        assert!(
            AgentOptions {
                env_from_host: vec!["HOME".into()],
                ..Default::default()
            }
            .run_options()
            .is_err(),
            "HOME is not a provider key"
        );
    }

    #[tokio::test]
    async fn without_a_space_firings_fail_and_deliveries_are_refused_with_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::new(crate::CoreConfig::in_dir(dir.path())).unwrap();
        let bots = Bots::new(core, AgentOptions::default());
        bots.upsert(vec![BotInfo {
            id: "ada".into(),
            name: "Ada".into(),
            agent: "claude-code".into(),
            space: "direct:127.0.0.1:1".into(),
        }])
        .await;
        let r = cua_spaces::routines::Routine {
            id: "r".into(),
            bot_id: "ghost".into(),
            title: "T".into(),
            prompt: "p".into(),
            schedule: cua_spaces::routines::Schedule::EveryMinutes { minutes: 1 },
            is_enabled: true,
            created_at: chrono::Utc::now(),
            last_fired_at: None,
            last_run_id: None,
            last_outcome: None,
        };
        match bots.fire(&r).await {
            RoutineFiring::Failed { reason } => {
                assert!(reason.contains("no Bot ghost"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        let d = bots.deliver("hi", "ada").await;
        assert!(!d.accepted && !d.reason.is_empty());
        assert_eq!(bots.display_name("ada").await, "Ada");
        assert_eq!(bots.display_name("ghost").await, "ghost");
        assert!(!bots.is_working("ada").await);
        assert_eq!(bots.latest_reply("ada").await, None);
    }
}
