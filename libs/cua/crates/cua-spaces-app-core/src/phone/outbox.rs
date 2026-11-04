// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's outbox: what a person sent that the Space has not taken yet.
//!
//! A message, a task, or an answer to an approval is written down before
//! anything is posted, so it survives a lost connection and a quit, and it is
//! delivered **exactly once**: one id per message, claim before posting,
//! never post a claimed message blindly after a quit, back off on a lost
//! connection, give up loudly after [`DELIVERY_ATTEMPTS`] real failures. It
//! is made per Space and kept for the life of the app:
//!
//! * a message is claimed (`delivering`) before the host posts it, and is
//!   marked delivered when the post returned;
//! * a claimed message found after a quit or a crash is not posted again
//!   blindly: [`Action::Resume`] asks the host to look in the Space's thread
//!   ([`Effect::VerifyDelivery`]); the answer ([`Action::ThreadHas`]) decides
//!   (found: delivered; not found: back to the queue);
//! * an id seen twice ([`Action::Send`]) is one message;
//! * messages of one Space go in the order they were sent, one at a time; a
//!   message that could not be sent holds the ones behind it until it is
//!   retried or deleted, so a thread never reads out of order.
//!
//! [`reduce`] never reads a clock, a disk or a network: the shell passes the
//! time in every action, keeps the [`State`] (sealed on disk) and does what
//! the [`Effect`]s say. [`view`] is what the thread and Home draw, in the
//! core's words.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Seconds (in milliseconds) before the first retry after a lost connection;
/// it doubles up to [`RETRY_MAX_MS`].
pub const RETRY_FIRST_MS: u64 = 2_000;
/// The longest wait between two retries.
pub const RETRY_MAX_MS: u64 = 60_000;
/// Times a message is posted again after its post failed, before it stays
/// queued with an error (the person can retry the whole thing).
pub const DELIVERY_ATTEMPTS: u32 = 5;

/// The state's schema; a state of another schema is dropped.
pub const SCHEMA: u32 = 1;

/// Delivered messages kept for this long, so a repeat of their id is still one message.
pub const KEEP_DELIVERED_MS: u64 = 10 * 60_000;

/// Where a message is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageState {
    /// Waiting to be sent.
    #[default]
    Queued,
    /// Claimed: the host is posting it.
    Delivering,
    /// In the Space's thread.
    Delivered,
    /// Could not be sent; the person retries or deletes it.
    Failed,
}

/// Something the person sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    /// The caller's id for it (the idempotency key sent to the daemon).
    pub id: String,
    pub space: String,
    /// `post` (a message), `answer` (an approval answer), `task` (a task start).
    pub kind: String,
    pub text: String,
    pub reply_to: Option<u64>,
    /// `send`, `queue` or `steer`: how it reaches an agent that is running.
    pub mode: String,
    /// What the shell needs to post it (the view, the answer, the task request).
    pub extra: Option<Value>,
    pub state: MessageState,
    /// Real failures (not lost connections).
    pub attempts: u32,
    pub sent_ms: u64,
    pub claimed_ms: Option<u64>,
    pub delivered_ms: Option<u64>,
    /// Not before this time (the backoff after a lost connection).
    pub retry_at_ms: Option<u64>,
    pub error: Option<String>,
}

/// The whole record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub schema: u32,
    /// The phone has a connection (the shell's reading).
    pub online: bool,
    /// Lost connections since the last delivery (the backoff's counter).
    pub lost: u32,
    pub messages: Vec<Message>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            online: true,
            lost: 0,
            messages: Vec::new(),
        }
    }
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// The person sent something. The same `id` again changes nothing.
    Send {
        now_ms: u64,
        id: String,
        space: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        text: String,
        #[serde(default)]
        reply_to: Option<u64>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        extra: Option<Value>,
    },
    /// The phone went offline or came back.
    Online { now_ms: u64, online: bool },
    /// Time passed (the shell calls it at [`Effect::WakeAt`] and when the app wakes).
    Tick { now_ms: u64 },
    /// The host posted a claimed message.
    Delivered { now_ms: u64, id: String },
    /// Posting a claimed message failed. `network`: a lost or missing connection.
    DeliveryFailed {
        now_ms: u64,
        id: String,
        #[serde(default)]
        network: bool,
        #[serde(default)]
        error: String,
    },
    /// The app started again with this record: whatever was in flight is looked at again.
    Resume { now_ms: u64 },
    /// The answer to [`Effect::VerifyDelivery`].
    ThreadHas {
        now_ms: u64,
        id: String,
        found: bool,
    },
    /// The person pressed Retry on a message that did not send.
    Retry { now_ms: u64, id: String },
    /// The person deleted a message that did not send.
    Discard { now_ms: u64, id: String },
}

/// What the host does next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Effect {
    /// Post this claimed message, then answer [`Action::Delivered`] or [`Action::DeliveryFailed`].
    Deliver { id: String },
    /// Look for this message in the Space's thread and answer [`Action::ThreadHas`].
    VerifyDelivery { id: String, space: String },
    /// Call [`Action::Tick`] at this time.
    WakeAt { at_ms: u64 },
}

/// A step and what the host does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reduced {
    pub state: State,
    pub effects: Vec<Effect>,
}

fn backoff(lost: u32) -> u64 {
    let shift = lost.saturating_sub(1).min(16);
    (RETRY_FIRST_MS << shift).min(RETRY_MAX_MS)
}

fn mode_of(m: Option<&str>) -> String {
    match m {
        Some("queue") => "queue".into(),
        Some("steer") => "steer".into(),
        _ => "send".into(),
    }
}

/// Claims what can go now and says when to look again.
fn pump(s: &mut State, now: u64, effects: &mut Vec<Effect>) {
    // Drop old delivered messages.
    s.messages.retain(|m| {
        m.state != MessageState::Delivered
            || m.delivered_ms
                .is_none_or(|d| now.saturating_sub(d) < KEEP_DELIVERED_MS)
    });
    let mut next_wake: Option<u64> = None;
    let mut spaces: Vec<String> = vec![];
    for m in &s.messages {
        if !spaces.contains(&m.space) {
            spaces.push(m.space.clone());
        }
    }
    for space in spaces {
        // The head of a Space: its first message not yet delivered.
        let Some(i) = s
            .messages
            .iter()
            .position(|m| m.space == space && m.state != MessageState::Delivered)
        else {
            continue;
        };
        let m = &mut s.messages[i];
        if m.state != MessageState::Queued || !s.online {
            continue;
        }
        match m.retry_at_ms {
            Some(at) if at > now => {
                next_wake = Some(next_wake.map_or(at, |w| w.min(at)));
            }
            _ => {
                m.state = MessageState::Delivering;
                m.claimed_ms = Some(now);
                m.retry_at_ms = None;
                effects.push(Effect::Deliver { id: m.id.clone() });
            }
        }
    }
    if let Some(at_ms) = next_wake {
        effects.push(Effect::WakeAt { at_ms });
    }
}

/// Moves the state by one action and says what the host does next.
pub fn reduce(state: &State, action: &Action) -> Reduced {
    let mut s = state.clone();
    if s.schema != SCHEMA {
        s = State::default();
    }
    let mut effects = vec![];
    let now;
    match action {
        Action::Send {
            now_ms,
            id,
            space,
            kind,
            text,
            reply_to,
            mode,
            extra,
        } => {
            now = *now_ms;
            if !id.is_empty() && !s.messages.iter().any(|m| &m.id == id) {
                s.messages.push(Message {
                    id: id.clone(),
                    space: space.clone(),
                    kind: kind
                        .clone()
                        .filter(|k| !k.is_empty())
                        .unwrap_or_else(|| "post".into()),
                    text: text.clone(),
                    reply_to: *reply_to,
                    mode: mode_of(mode.as_deref()),
                    extra: extra.clone(),
                    sent_ms: now,
                    ..Default::default()
                });
            }
        }
        Action::Online { now_ms, online } => {
            now = *now_ms;
            s.online = *online;
            if *online {
                // Back: whatever waited for the connection goes now.
                s.lost = 0;
                for m in &mut s.messages {
                    if m.state == MessageState::Queued {
                        m.retry_at_ms = None;
                    }
                }
            }
        }
        Action::Tick { now_ms } => now = *now_ms,
        Action::Delivered { now_ms, id } => {
            now = *now_ms;
            if let Some(m) = s.messages.iter_mut().find(|m| &m.id == id)
                && matches!(m.state, MessageState::Delivering | MessageState::Queued)
            {
                m.state = MessageState::Delivered;
                m.delivered_ms = Some(now);
                m.error = None;
                s.lost = 0;
                s.online = true;
            }
        }
        Action::DeliveryFailed {
            now_ms,
            id,
            network,
            error,
        } => {
            now = *now_ms;
            if let Some(m) = s.messages.iter_mut().find(|m| &m.id == id)
                && m.state == MessageState::Delivering
            {
                if *network {
                    s.lost = s.lost.saturating_add(1);
                    m.state = MessageState::Queued;
                    m.retry_at_ms = Some(now + backoff(s.lost));
                } else {
                    m.attempts += 1;
                    m.error = Some(error.clone()).filter(|e| !e.is_empty());
                    if m.attempts >= DELIVERY_ATTEMPTS {
                        m.state = MessageState::Failed;
                        m.retry_at_ms = None;
                    } else {
                        m.state = MessageState::Queued;
                        m.retry_at_ms = Some(now + backoff(m.attempts));
                    }
                }
            }
        }
        Action::Resume { now_ms } => {
            now = *now_ms;
            for m in &s.messages {
                if m.state == MessageState::Delivering {
                    effects.push(Effect::VerifyDelivery {
                        id: m.id.clone(),
                        space: m.space.clone(),
                    });
                }
            }
        }
        Action::ThreadHas { now_ms, id, found } => {
            now = *now_ms;
            if let Some(m) = s.messages.iter_mut().find(|m| &m.id == id)
                && m.state == MessageState::Delivering
            {
                if *found {
                    m.state = MessageState::Delivered;
                    m.delivered_ms = Some(now);
                } else {
                    m.state = MessageState::Queued;
                    m.retry_at_ms = None;
                }
            }
        }
        Action::Retry { now_ms, id } => {
            now = *now_ms;
            if let Some(m) = s.messages.iter_mut().find(|m| &m.id == id)
                && m.state == MessageState::Failed
            {
                m.state = MessageState::Queued;
                m.attempts = 0;
                m.error = None;
                m.retry_at_ms = None;
            }
        }
        Action::Discard { now_ms, id } => {
            now = *now_ms;
            s.messages.retain(|m| {
                !(&m.id == id && matches!(m.state, MessageState::Failed | MessageState::Queued))
            });
        }
    }
    // A resume only looks at what was in flight; it claims nothing new until the answers are in.
    if !matches!(action, Action::Resume { .. }) {
        pump(&mut s, now, &mut effects);
    }
    Reduced { state: s, effects }
}

/// One message as the thread draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub id: String,
    pub space: String,
    pub kind: String,
    pub text: String,
    /// `waiting`, `sending`, `failed`.
    pub state: String,
    /// The line under it: "Waiting for a connection", "Sending…", "Not sent".
    pub label: String,
    pub can_retry: bool,
    pub can_discard: bool,
    pub retry_label: String,
    pub discard_label: String,
    pub mode: String,
}

/// What the outbox says to the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub rows: Vec<RowView>,
    /// A line for the top of a thread or Home: "You're offline. 2 messages send when you're back."
    pub banner: Option<String>,
    pub waiting: usize,
    pub failed: usize,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The outbox as it shows for `space` (all Spaces with `None`).
pub fn view(s: &State, space: Option<&str>) -> View {
    let pending: Vec<&Message> = s
        .messages
        .iter()
        .filter(|m| m.state != MessageState::Delivered)
        .filter(|m| space.is_none_or(|sp| m.space == sp))
        .collect();
    let mut rows = vec![];
    // Messages of a Space go in order: one with another still ahead of it waits for that one.
    let mut ahead: Vec<&str> = vec![];
    for m in &pending {
        let behind = ahead.contains(&m.space.as_str());
        ahead.push(m.space.as_str());
        let (state, label) = match m.state {
            MessageState::Failed => ("failed", "Not sent".to_string()),
            MessageState::Delivering => ("sending", "Sending\u{2026}".to_string()),
            _ if !s.online => ("waiting", "Waiting for a connection".to_string()),
            _ if behind => ("waiting", "Waiting for the message above".to_string()),
            _ if m.retry_at_ms.is_some() => ("waiting", "Trying again soon".to_string()),
            _ => ("sending", "Sending\u{2026}".to_string()),
        };
        rows.push(RowView {
            id: m.id.clone(),
            space: m.space.clone(),
            kind: m.kind.clone(),
            text: m.text.clone(),
            state: state.into(),
            label,
            can_retry: m.state == MessageState::Failed,
            can_discard: matches!(m.state, MessageState::Failed | MessageState::Queued),
            retry_label: "Retry".into(),
            discard_label: "Delete".into(),
            mode: m.mode.clone(),
        });
    }
    let waiting = rows.iter().filter(|r| r.state == "waiting").count();
    let failed = rows.iter().filter(|r| r.state == "failed").count();
    let banner = if !s.online && !pending.is_empty() {
        Some(format!(
            "You're offline. {} will send when you're back.",
            plural(pending.len(), "message", "messages")
        ))
    } else if !s.online {
        Some("You're offline. What you see may be behind.".into())
    } else if failed > 0 {
        Some(format!(
            "{} did not send.",
            plural(failed, "message", "messages")
        ))
    } else {
        None
    };
    View {
        rows,
        banner,
        waiting,
        failed,
    }
}

/// The line over cached content while the phone is offline: what is shown is what was saved, and when.
/// Nothing while online (the content is live), and nothing is claimed to be current when it is not.
pub fn cache_line(saved_ms: Option<u64>, now_ms: u64, offline: bool) -> Option<String> {
    if !offline {
        return None;
    }
    Some(match saved_ms.filter(|m| *m > 0) {
        Some(saved) => format!(
            "You're offline. Showing what was saved {}.",
            crate::persistent::ago(saved, now_ms)
        ),
        None => "You're offline. Nothing was saved to show yet.".into(),
    })
}

/// One thread entry, as much as finding a message needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EntryLite {
    pub id: String,
    /// The `client_id` the daemon echoes, when it supports one.
    pub client_id: Option<String>,
    pub author_id: String,
    pub text: String,
    pub ts_ms: u64,
}

/// Skew allowed between the phone's clock and the daemon's when no client id comes back.
pub const CLOCK_SKEW_MS: u64 = 60_000;

/// Whether `m` is already in the thread `entries` (`me` is this account's id).
/// An echoed client id decides. A daemon that does not echo one is matched by
/// the author, the exact text and a time at or after the claim (less the skew).
pub fn found_in_thread(m: &Message, entries: &[EntryLite], me: &str) -> bool {
    if entries
        .iter()
        .any(|e| e.client_id.as_deref() == Some(m.id.as_str()))
    {
        return true;
    }
    let since = m
        .claimed_ms
        .unwrap_or(m.sent_ms)
        .saturating_sub(CLOCK_SKEW_MS);
    entries.iter().any(|e| {
        e.client_id.is_none()
            && e.author_id == me
            && e.text.trim() == m.text.trim()
            && e.ts_ms >= since
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(now: u64, id: &str, space: &str, text: &str) -> Action {
        Action::Send {
            now_ms: now,
            id: id.into(),
            space: space.into(),
            kind: None,
            text: text.into(),
            reply_to: None,
            mode: None,
            extra: None,
        }
    }
    fn step(s: &State, a: Action) -> Reduced {
        reduce(s, &a)
    }
    fn state_of(s: &State, id: &str) -> MessageState {
        s.messages.iter().find(|m| m.id == id).unwrap().state
    }

    #[test]
    fn a_message_is_claimed_before_it_is_posted_and_delivered_once() {
        let r = step(&State::default(), send(1_000, "a", "s1", "hello"));
        assert_eq!(r.effects, vec![Effect::Deliver { id: "a".into() }]);
        assert_eq!(state_of(&r.state, "a"), MessageState::Delivering);
        // The same id again is the same message: nothing is posted twice.
        let again = step(&r.state, send(1_100, "a", "s1", "hello"));
        assert!(again.effects.is_empty());
        assert_eq!(again.state.messages.len(), 1);
        let done = step(
            &again.state,
            Action::Delivered {
                now_ms: 1_200,
                id: "a".into(),
            },
        );
        assert_eq!(state_of(&done.state, "a"), MessageState::Delivered);
        let twice = step(
            &done.state,
            Action::Delivered {
                now_ms: 1_300,
                id: "a".into(),
            },
        );
        assert_eq!(twice.state.messages, done.state.messages);
        // A repeat of a delivered id is still one message.
        let repeat = step(&twice.state, send(2_000, "a", "s1", "hello"));
        assert_eq!(repeat.state.messages.len(), 1);
        assert!(repeat.effects.is_empty());
    }

    #[test]
    fn offline_messages_wait_and_all_go_in_order_when_back() {
        let mut s = step(
            &State::default(),
            Action::Online {
                now_ms: 0,
                online: false,
            },
        )
        .state;
        s = step(&s, send(10, "a", "s1", "one")).state;
        s = step(&s, send(20, "b", "s1", "two")).state;
        assert!(s.messages.iter().all(|m| m.state == MessageState::Queued));
        let v = view(&s, Some("s1"));
        assert_eq!(v.rows[0].label, "Waiting for a connection");
        assert_eq!(
            v.banner.as_deref(),
            Some("You're offline. 2 messages will send when you're back.")
        );
        let back = step(
            &s,
            Action::Online {
                now_ms: 30,
                online: true,
            },
        );
        // Only the head of the Space goes; the second waits for it.
        assert_eq!(back.effects, vec![Effect::Deliver { id: "a".into() }]);
        let a = step(
            &back.state,
            Action::Delivered {
                now_ms: 40,
                id: "a".into(),
            },
        );
        assert_eq!(a.effects, vec![Effect::Deliver { id: "b".into() }]);
    }

    #[test]
    fn spaces_do_not_wait_for_each_other() {
        let mut s = step(&State::default(), send(1, "a", "s1", "x")).state;
        let r = step(&s, send(2, "b", "s2", "y"));
        s = r.state;
        assert_eq!(r.effects, vec![Effect::Deliver { id: "b".into() }]);
        assert_eq!(state_of(&s, "a"), MessageState::Delivering);
    }

    #[test]
    fn a_lost_connection_backs_off_and_does_not_count_against_the_message() {
        let mut s = step(&State::default(), send(0, "a", "s1", "x")).state;
        let r = step(
            &s,
            Action::DeliveryFailed {
                now_ms: 100,
                id: "a".into(),
                network: true,
                error: String::new(),
            },
        );
        s = r.state;
        assert_eq!(s.messages[0].attempts, 0);
        assert_eq!(s.messages[0].retry_at_ms, Some(100 + RETRY_FIRST_MS));
        assert_eq!(
            r.effects,
            vec![Effect::WakeAt {
                at_ms: 100 + RETRY_FIRST_MS
            }]
        );
        assert_eq!(view(&s, None).rows[0].label, "Trying again soon");
        // Too early: nothing. On time: it goes again.
        assert!(
            step(&s, Action::Tick { now_ms: 500 })
                .effects
                .iter()
                .all(|e| matches!(e, Effect::WakeAt { .. }))
        );
        let go = step(
            &s,
            Action::Tick {
                now_ms: 100 + RETRY_FIRST_MS,
            },
        );
        assert_eq!(go.effects, vec![Effect::Deliver { id: "a".into() }]);
        // A second loss doubles the wait, to the cap.
        let r2 = step(
            &go.state,
            Action::DeliveryFailed {
                now_ms: 5_000,
                id: "a".into(),
                network: true,
                error: String::new(),
            },
        );
        assert_eq!(
            r2.state.messages[0].retry_at_ms,
            Some(5_000 + RETRY_FIRST_MS * 2)
        );
        let mut t = r2.state;
        for i in 0..30 {
            t.messages[0].state = MessageState::Delivering;
            t = step(
                &t,
                Action::DeliveryFailed {
                    now_ms: 10_000 + i,
                    id: "a".into(),
                    network: true,
                    error: String::new(),
                },
            )
            .state;
        }
        assert_eq!(
            t.messages[0].retry_at_ms.unwrap() - (10_000 + 29),
            RETRY_MAX_MS
        );
    }

    #[test]
    fn real_failures_give_up_loudly_and_hold_the_messages_behind() {
        let mut s = step(&State::default(), send(0, "a", "s1", "x")).state;
        s = step(&s, send(1, "b", "s1", "y")).state;
        let mut now = 10;
        for n in 1..=DELIVERY_ATTEMPTS {
            let r = step(
                &s,
                Action::DeliveryFailed {
                    now_ms: now,
                    id: "a".into(),
                    network: false,
                    error: "forbidden".into(),
                },
            );
            s = r.state;
            if n < DELIVERY_ATTEMPTS {
                now = s.messages[0].retry_at_ms.unwrap();
                s = step(&s, Action::Tick { now_ms: now }).state;
                assert_eq!(state_of(&s, "a"), MessageState::Delivering);
            }
        }
        assert_eq!(state_of(&s, "a"), MessageState::Failed);
        let v = view(&s, Some("s1"));
        assert_eq!(
            (v.rows[0].state.as_str(), v.rows[0].label.as_str()),
            ("failed", "Not sent")
        );
        assert_eq!(v.rows[1].label, "Waiting for the message above");
        assert_eq!(v.banner.as_deref(), Some("1 message did not send."));
        // b did not move.
        assert_eq!(state_of(&s, "b"), MessageState::Queued);
        // Retry sends a, then b follows.
        let r = step(
            &s,
            Action::Retry {
                now_ms: 99_999,
                id: "a".into(),
            },
        );
        assert_eq!(r.effects, vec![Effect::Deliver { id: "a".into() }]);
        let d = step(
            &r.state,
            Action::Delivered {
                now_ms: 100_000,
                id: "a".into(),
            },
        );
        assert_eq!(d.effects, vec![Effect::Deliver { id: "b".into() }]);
    }

    #[test]
    fn discarding_the_failed_message_frees_the_ones_behind() {
        let mut s = step(&State::default(), send(0, "a", "s1", "x")).state;
        s = step(&s, send(1, "b", "s1", "y")).state;
        s.messages[0].state = MessageState::Failed;
        let r = step(
            &s,
            Action::Discard {
                now_ms: 5,
                id: "a".into(),
            },
        );
        assert_eq!(r.state.messages.len(), 1);
        assert_eq!(r.effects, vec![Effect::Deliver { id: "b".into() }]);
    }

    #[test]
    fn a_claimed_message_after_a_quit_is_looked_for_never_posted_blindly() {
        let s = step(&State::default(), send(0, "a", "s1", "hello")).state;
        // The app quit while "a" was being posted: the record says delivering.
        let r = step(&s, Action::Resume { now_ms: 50_000 });
        assert_eq!(
            r.effects,
            vec![Effect::VerifyDelivery {
                id: "a".into(),
                space: "s1".into()
            }]
        );
        assert_eq!(state_of(&r.state, "a"), MessageState::Delivering);
        // The thread has it: delivered, never posted again.
        let found = step(
            &r.state,
            Action::ThreadHas {
                now_ms: 50_100,
                id: "a".into(),
                found: true,
            },
        );
        assert_eq!(state_of(&found.state, "a"), MessageState::Delivered);
        assert!(found.effects.is_empty());
        // The thread does not: back to the queue and posted once.
        let missing = step(
            &r.state,
            Action::ThreadHas {
                now_ms: 50_100,
                id: "a".into(),
                found: false,
            },
        );
        assert_eq!(missing.effects, vec![Effect::Deliver { id: "a".into() }]);
    }

    #[test]
    fn delivered_messages_are_forgotten_after_a_while() {
        let mut s = step(&State::default(), send(0, "a", "s1", "x")).state;
        s = step(
            &s,
            Action::Delivered {
                now_ms: 10,
                id: "a".into(),
            },
        )
        .state;
        assert_eq!(s.messages.len(), 1);
        s = step(
            &s,
            Action::Tick {
                now_ms: 10 + KEEP_DELIVERED_MS,
            },
        )
        .state;
        assert!(s.messages.is_empty());
    }

    #[test]
    fn a_state_of_another_schema_is_dropped() {
        let s = State {
            schema: 99,
            messages: vec![Message {
                id: "old".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let r = step(&s, send(0, "a", "s1", "x"));
        assert_eq!(r.state.schema, SCHEMA);
        assert_eq!(r.state.messages.len(), 1);
    }

    #[test]
    fn the_state_survives_a_round_trip_through_json() {
        let mut s = step(
            &State::default(),
            Action::Online {
                now_ms: 0,
                online: false,
            },
        )
        .state;
        s = step(
            &s,
            Action::Send {
                now_ms: 1,
                id: "a".into(),
                space: "s".into(),
                kind: Some("answer".into()),
                text: "Approved".into(),
                reply_to: Some(7),
                mode: Some("steer".into()),
                extra: Some(serde_json::json!({"approval": 7, "approve": true, "scope": "space"})),
            },
        )
        .state;
        let text = serde_json::to_string(&s).unwrap();
        let back: State = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.messages[0].kind, "answer");
        assert_eq!(back.messages[0].mode, "steer");
    }

    #[test]
    fn cached_content_says_when_it_was_saved_and_only_while_offline() {
        assert_eq!(cache_line(Some(1_000), 5_000_000, false), None);
        assert_eq!(
            cache_line(Some(1_000_000), 1_000_000 + 7_200_000, true).as_deref(),
            Some("You're offline. Showing what was saved 2h ago.")
        );
        assert_eq!(
            cache_line(Some(1_000_000), 1_030_000, true).as_deref(),
            Some("You're offline. Showing what was saved just now.")
        );
        assert_eq!(
            cache_line(None, 5, true).as_deref(),
            Some("You're offline. Nothing was saved to show yet.")
        );
        assert_eq!(
            cache_line(Some(0), 5, true).as_deref(),
            Some("You're offline. Nothing was saved to show yet.")
        );
    }

    #[test]
    fn a_message_is_found_by_its_client_id_or_by_author_text_and_time() {
        let m = Message {
            id: "c1".into(),
            space: "s".into(),
            text: "hello".into(),
            sent_ms: 1_000_000,
            claimed_ms: Some(1_000_100),
            state: MessageState::Delivering,
            ..Default::default()
        };
        let by_id = [EntryLite {
            id: "e9".into(),
            client_id: Some("c1".into()),
            author_id: "x".into(),
            text: "other".into(),
            ts_ms: 1,
        }];
        assert!(found_in_thread(&m, &by_id, "me"));
        let by_text = [EntryLite {
            id: "e1".into(),
            client_id: None,
            author_id: "me".into(),
            text: " hello ".into(),
            ts_ms: 1_000_500,
        }];
        assert!(found_in_thread(&m, &by_text, "me"));
        // Someone else's identical words, an older message, a different text, or another client id: not found.
        let someone = [EntryLite {
            author_id: "you".into(),
            ..by_text[0].clone()
        }];
        assert!(!found_in_thread(&m, &someone, "me"));
        let old = [EntryLite {
            ts_ms: 10,
            ..by_text[0].clone()
        }];
        assert!(!found_in_thread(&m, &old, "me"));
        let other = [EntryLite {
            text: "bye".into(),
            ..by_text[0].clone()
        }];
        assert!(!found_in_thread(&m, &other, "me"));
        let theirs = [EntryLite {
            client_id: Some("c2".into()),
            ..by_text[0].clone()
        }];
        assert!(!found_in_thread(&m, &theirs, "me"));
    }
}
