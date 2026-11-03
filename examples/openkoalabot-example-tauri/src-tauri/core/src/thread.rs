// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Bot is one long-lived agent thread in the one shared Space (never a
//! Space per Bot, as in the Swift app's `BotStore`). A thread starts a run
//! (`agent_start`), sends follow-ups (`agent_message`; a busy run queues
//! them), and reads the run's event log from a cursor (`agent_events`)
//! into the SDK's transcript fold ([`Transcript`]): the agent's messages,
//! and its activity (install, tools, thinking, turn ends, notices) as muted
//! step groups. What the user typed is kept here and interleaved by turn,
//! so the prompt is never shown twice.

use crate::{Error, Result};
use cua_spaces::Space;
use cua_spaces::agents::{Agents, RunInfo, RunOptions, RunStatus, Transcript};
use std::time::Duration;

/// Who a transcript line is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    /// What the user typed.
    User,
    /// The agent's own words (the SDK's `message` items).
    Agent,
    /// A muted group of one-line steps (the SDK's `activity` items).
    Activity,
}

/// One transcript line: a user message, an agent message, or an activity
/// group (`text` its summary, `steps` its rows).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Line {
    pub turn: usize,
    pub speaker: Speaker,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
}

/// A roster row: a run the Space knows about.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RosterEntry {
    pub run_id: String,
    pub agent: Option<String>,
    pub status: String,
    pub reason: String,
    pub accepts_message: bool,
    pub summary: Option<String>,
}

impl From<&RunInfo> for RosterEntry {
    fn from(s: &RunInfo) -> Self {
        Self {
            run_id: s.run_id.clone(),
            agent: s
                .harness
                .clone()
                .or_else(|| s.meta.as_ref().map(|m| m.harness.clone())),
            status: s.status.as_str().to_string(),
            reason: s.reason.clone(),
            accepts_message: s.accepts_message,
            summary: s
                .meta
                .as_ref()
                .map(|m| m.prompt.chars().take(100).collect()),
        }
    }
}

/// Consecutive `crashed` polls before a crash is reported.
pub const FAILURE_POLLS: u32 = 10;

/// Largest event page read per request.
const EVENT_PAGE: usize = 500;
/// Pages read per poll before the rest waits for the next poll.
const EVENT_PAGES_PER_POLL: usize = 20;

/// No turn is running and the state is known.
pub fn turn_over(s: RunStatus) -> bool {
    !matches!(s, RunStatus::Running | RunStatus::Unknown)
}

/// Interleaves what the user typed (`(turn, text)`, oldest first) with the
/// SDK's transcript items: each prompt goes before the first item of its
/// turn. The SDK's own `user` items (the prompt as the agent got it) are
/// skipped, so the prompt is never repeated.
pub fn merge(user: &[(usize, String)], fold: &Transcript) -> Vec<Line> {
    let mut out = Vec::new();
    let mut pending = user.iter().peekable();
    for item in fold.items() {
        let turn = item.turn as usize;
        let speaker = match item.kind {
            "message" => Speaker::Agent,
            "activity" => Speaker::Activity,
            _ => continue,
        };
        while let Some((t, text)) = pending.next_if(|(t, _)| *t <= turn && turn > 0) {
            out.push(Line {
                turn: *t,
                speaker: Speaker::User,
                text: text.clone(),
                steps: vec![],
            });
        }
        out.push(Line {
            turn,
            speaker,
            text: item.text.clone(),
            steps: item.steps.clone(),
        });
    }
    for (t, text) in pending {
        out.push(Line {
            turn: *t,
            speaker: Speaker::User,
            text: text.clone(),
            steps: vec![],
        });
    }
    out
}

fn agents_err(e: impl Into<cua_spaces::Error>) -> Error {
    Error::Spaces(e.into())
}

/// One Bot's agent thread.
pub struct BotThread {
    agents: Agents,
    agent: String,
    run_id: Option<String>,
    cursor: u64,
    turn: usize,
    /// What the user typed, by turn.
    user: Vec<(usize, String)>,
    /// The run's events, folded by the SDK.
    fold: Transcript,
    last: Option<RunInfo>,
    options: RunOptions,
}

impl BotThread {
    /// A thread for `agent` (`claude-code`, `openai-codex`, ...). No host
    /// credentials are copied into the Space: pass provider keys with
    /// [`BotThread::with_options`] (`RunOptions::env`).
    pub async fn new(space: &Space, agent: &str) -> Result<Self> {
        Ok(Self {
            agents: space.agents().await?,
            agent: agent.to_string(),
            run_id: None,
            cursor: 0,
            turn: 0,
            user: Vec::new(),
            fold: Transcript::new(),
            last: None,
            options: RunOptions::default(),
        })
    }

    /// Options for the run the first message starts.
    pub fn with_options(mut self, options: RunOptions) -> Self {
        self.options = options;
        self
    }

    /// The run, once started.
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    /// The transcript so far: user messages, agent messages and activity
    /// groups, in order.
    pub fn transcript(&self) -> Vec<Line> {
        merge(&self.user, &self.fold)
    }

    /// The roster preview: the agent's last message on one line.
    pub fn preview(&self) -> Option<String> {
        self.fold.preview()
    }

    /// The last polled status.
    pub fn last_status(&self) -> Option<&RunInfo> {
        self.last.as_ref()
    }

    /// Sends a user turn: the first starts the run, later ones are
    /// delivered to it (queued behind a running turn).
    pub async fn send(&mut self, text: &str) -> Result<()> {
        self.turn += 1;
        self.user.push((self.turn, text.to_string()));
        match &self.run_id {
            None => {
                // #region docs:rs-agent
                let r = self
                    .agents
                    .start(&self.agent, text, self.options.clone())
                    .await
                    .map_err(agents_err)?;
                // #endregion docs:rs-agent
                for note in &r.notes {
                    self.system(note.clone());
                }
                self.run_id = Some(r.run_id);
            }
            Some(run) => {
                // #region docs:rs-agent-message
                let r = self
                    .agents
                    .send(run, text, vec![])
                    .await
                    .map_err(agents_err)?;
                if !r.accepts_message {
                    self.system(format!("queued while {}: {}", r.phase, r.reason));
                }
                // #endregion docs:rs-agent-message
            }
        }
        Ok(())
    }

    fn system(&mut self, text: String) {
        self.fold.note(self.turn as u32, &text);
    }

    /// One poll: new events join the transcript, then the status is read.
    pub async fn poll(&mut self) -> Result<&RunInfo> {
        let run = self
            .run_id
            .clone()
            .ok_or_else(|| Error::Invalid("the thread has not started".into()))?;
        for _ in 0..EVENT_PAGES_PER_POLL {
            let page = self
                .agents
                .events(&run, self.cursor, EVENT_PAGE)
                .await
                .map_err(agents_err)?;
            for ev in &page.events {
                self.fold.absorb(ev);
            }
            self.cursor = page.cursor;
            if page.caught_up || page.events.is_empty() {
                break;
            }
        }
        let s = self.agents.status(&run).await.map_err(agents_err)?;
        Ok(self.last.insert(s))
    }

    /// Polls until the current turn is over and its output contains
    /// `expect` (when given): at most `max_polls` polls, `every` apart.
    pub async fn wait_turn(
        &mut self,
        expect: Option<&str>,
        every: Duration,
        max_polls: u32,
    ) -> Result<RunInfo> {
        // `failed` is recorded by the runner and final. `crashed` (process
        // gone without a record) is believed only when it persists.
        let mut crashed = 0u32;
        for _ in 0..max_polls {
            let s = self.poll().await?.clone();
            let has = expect.is_none_or(|e| self.turn_output(self.turn).contains(e));
            crashed = if s.status == RunStatus::Crashed {
                crashed + 1
            } else {
                0
            };
            if s.status == RunStatus::Failed || crashed >= FAILURE_POLLS {
                return Err(Error::Invalid(format!(
                    "turn {} ended {}: {} (output {:?})",
                    self.turn,
                    s.status.as_str(),
                    s.reason,
                    self.turn_output(self.turn)
                )));
            }
            // The runner counts turns too: an idle run that has not reached
            // this turn has not picked the prompt up yet.
            if turn_over(s.status) && s.turn as usize >= self.turn && has {
                return Ok(s);
            }
            tokio::time::sleep(every).await;
        }
        Err(Error::Timeout(format!(
            "turn {} after {max_polls} polls (last {:?}; output {:?})",
            self.turn,
            self.last.as_ref().map(|s| s.status),
            self.turn_output(self.turn)
        )))
    }

    /// The agent's messages in `turn` (never activity).
    pub fn turn_output(&self, turn: usize) -> String {
        self.fold
            .items()
            .iter()
            .filter(|i| i.turn as usize == turn && i.kind == "message")
            .map(|i| i.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Stops the run (a finished turn is not an error).
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(run) = &self.run_id {
            self.agents.stop(run).await.map_err(agents_err)?;
        }
        Ok(())
    }
}

/// Every run in the Space (one `agent_list` round trip).
pub async fn roster(space: &Space) -> Result<Vec<RosterEntry>> {
    let agents = space.agents().await?;
    Ok(agents
        .list()
        .await
        .map_err(agents_err)?
        .iter()
        .map(RosterEntry::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spaces::agents::AgentEvent;

    fn ev(line: &str) -> AgentEvent {
        AgentEvent::parse(line).unwrap()
    }

    fn chunk(seq: u32, turn: u32, text: &str) -> AgentEvent {
        ev(&format!(
            r#"{{"seq":{seq},"ts":0,"turn":{turn},"type":"update","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"{text}"}}}}}}"#
        ))
    }

    #[test]
    fn message_chunks_of_one_turn_join_one_line_after_the_prompt() {
        let mut f = Transcript::new();
        f.absorb(&chunk(1, 1, "hello "));
        f.absorb(&chunk(2, 1, "koala"));
        f.absorb(&chunk(3, 2, "again"));
        let t = merge(&[(1, "hi".into()), (2, "more".into())], &f);
        let got: Vec<(usize, Speaker, &str)> = t
            .iter()
            .map(|l| (l.turn, l.speaker, l.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (1, Speaker::User, "hi"),
                (1, Speaker::Agent, "hello koala"),
                (2, Speaker::User, "more"),
                (2, Speaker::Agent, "again"),
            ]
        );
    }

    #[test]
    fn harness_output_is_activity_and_prompts_are_not_repeated() {
        let mut f = Transcript::new();
        for l in [
            r#"{"seq":1,"ts":0,"turn":0,"type":"install","id":"node","phase":"cached","detail":""}"#,
            r#"{"seq":2,"ts":0,"turn":1,"type":"turn_started","prompt":"hi"}"#,
            r#"{"seq":3,"ts":0,"turn":1,"type":"error","message":"no key"}"#,
            r#"{"seq":4,"ts":0,"turn":1,"type":"turn_ended","stopReason":"end_turn"}"#,
        ] {
            f.absorb(&ev(l));
        }
        f.note(1, "queued while working");
        let t = merge(&[(1, "hi".into())], &f);
        assert_eq!(t.len(), 3, "{t:?}");
        assert_eq!(
            (t[0].speaker, t[0].text.as_str()),
            (Speaker::Activity, "1 step")
        );
        assert_eq!(t[0].steps, ["Install node: cached"]);
        assert_eq!((t[1].speaker, t[1].text.as_str()), (Speaker::User, "hi"));
        assert_eq!(t[2].speaker, Speaker::Activity);
        assert_eq!(
            t[2].steps,
            [
                "Error: no key",
                "Turn 1 ended (end_turn)",
                "queued while working"
            ]
        );
        assert_eq!(t[2].text, "3 steps, 1 error");
    }

    #[test]
    fn a_prompt_without_output_yet_is_last() {
        let t = merge(&[(1, "hi".into())], &Transcript::new());
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].speaker, Speaker::User);
    }

    #[test]
    fn running_and_unknown_are_not_over() {
        assert!(!turn_over(RunStatus::Running));
        assert!(!turn_over(RunStatus::Unknown));
        assert!(turn_over(RunStatus::Idle));
        assert!(turn_over(RunStatus::Failed));
    }
}
