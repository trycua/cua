// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tells the person's phone that something needs them or ended.
//!
//! This machine's daemon sees the events: an agent's approval is posted to
//! a Space's thread, a task finishes or fails, an agent is blocked on a
//! sign-in. It reports each to the relay with the account it is signed in
//! as (`POST /v1/push/events`); the relay alone holds the push provider's
//! key and the account's registered phones, builds the alert from a fixed
//! phrase and sends it ([`cua_push`]). What leaves this machine is the
//! event's kind, the Space and entry it is about, the agent's short name
//! and a unique id: no message text, no tool output, no file content, no
//! secret.
//!
//! Without a sign-in or a relay, or in a test process, nothing is sent
//! and nothing waits: the events are the same ones the desktop app shows
//! as system notifications, and they are still there.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cua_push::{EventKind, PushEvent};

use crate::persistent::notify::Notification;
use crate::thread::{EntryKind, ThreadEntry};

/// Where events go.
pub trait Transport: Send + Sync {
    fn send(&self, event: PushEvent) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

static TRANSPORT: OnceLock<Arc<dyn Transport>> = OnceLock::new();

/// Sets where this process sends events. Only the first call counts.
pub fn install(t: Arc<dyn Transport>) -> bool {
    TRANSPORT.set(t).is_ok()
}

/// The relay: `POST <relay>/v1/push/events` with the signed-in account.
pub struct RelayTransport {
    http: reqwest::Client,
}

impl RelayTransport {
    pub fn new() -> RelayTransport {
        RelayTransport {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("cua-spaces")
                .build()
                .unwrap_or_default(),
        }
    }
}

impl Default for RelayTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for RelayTransport {
    fn send(&self, event: PushEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let http = self.http.clone();
        Box::pin(async move {
            let session = std::sync::Arc::new(cua_auth::Session::from_env());
            // Not signed in: there is no account to push to.
            let Ok(token) = session.access_token(false).await else {
                return;
            };
            let relay = cua_host::relay::relay_url_from_env();
            let url = format!("{}/v1/push/events", relay.trim_end_matches('/'));
            // The relay takes an event only from an enrolled computer of the
            // account (this one): its device session rides with the token.
            let device = {
                let store = session.store().clone();
                cua_host::DeviceAuth::new(
                    &relay,
                    std::sync::Arc::new(SessionTokens(session.clone())),
                    std::sync::Arc::new(store),
                    cua_host::device_name(),
                )
                .ok()
            };
            let mut request = http.post(url).bearer_auth(token).json(&event);
            if let Some(device) = device
                && let Some(device_session) = device.try_session().await
            {
                request = request.header(cua_host::relay::DEVICE_SESSION_HEADER, device_session);
            }
            match request.send().await {
                Ok(r) if r.status().is_success() => {}
                // A relay without push answers 404: nothing to do.
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {}
                Ok(r) => tracing::debug!(status = %r.status(), "push event not accepted"),
                Err(e) => tracing::debug!("push event not sent: {e}"),
            }
        })
    }
}

/// The signed-in account's tokens, for this computer's device session.
struct SessionTokens(std::sync::Arc<cua_auth::Session>);

#[async_trait::async_trait]
impl cua_host::AccountTokens for SessionTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        self.0
            .access_token(false)
            .await
            .map_err(|e| cua_host::Error::Unauthenticated(e.to_string()))
    }

    async fn signed_in(&self) -> bool {
        !matches!(self.0.credentials(), Ok(None))
    }
}

/// Sets up the process: the relay as the transport (not in a test), and
/// the two hooks that see the events.
pub fn register() {
    if !cua_home::is_test_process() {
        install(Arc::new(RelayTransport::new()));
    }
    crate::thread::set_append_hook(Box::new(on_entries));
    crate::persistent::notify::set_sink(Box::new(on_notification));
}

/// Reports `event` to the installed transport (only while the experiment
/// `mobile` is on). The hooks call this; tests may too.
pub fn report(event: PushEvent) {
    dispatch(event);
}

fn dispatch(event: PushEvent) {
    // Only a machine whose person turned the phone app on reports events.
    if !cua_spaces::experiments::enabled(cua_spaces::experiments::Gate::Mobile) {
        return;
    }
    let Some(t) = TRANSPORT.get() else { return };
    // The hooks run inside a lock or a request: send from a task.
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let fut = t.send(event);
        rt.spawn(fut);
    }
}

/// The Space as the phone opens it: `relay:<machine>` for a Space this
/// machine provides, else the id as it is.
pub fn phone_space(local: &str) -> String {
    let dir = cua_host::Host::new(cua_home::cua_home())
        .paths()
        .dir
        .clone();
    cua_host::provided::load_provided(&dir)
        .ok()
        .and_then(|all| {
            all.into_iter()
                .find(|p| p.local_space == local)
                .map(|p| format!("relay:{}", p.relay_machine))
        })
        .unwrap_or_else(|| local.to_string())
}

/// The event an entry makes, if any: an approval nobody has answered.
/// `batch` is the entries written with it (an approval a standing answer
/// covered is answered in the same write and asks nobody).
pub fn event_of_entry(space: &str, e: &ThreadEntry, batch: &[ThreadEntry]) -> Option<PushEvent> {
    if e.kind != EntryKind::Approval && e.join_request.is_none() {
        return None;
    }
    let answered = batch
        .iter()
        .any(|x| x.answer.as_ref().is_some_and(|a| a.approval == e.seq));
    if answered {
        return None;
    }
    let agent = e
        .author
        .agent_name()
        .map(str::to_string)
        // The host's own card names the agent it is for.
        .or_else(|| {
            e.approval
                .as_ref()
                .and_then(|a| a.card.as_ref())
                .and_then(|c| c.get("agent"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            e.join_request
                .as_ref()
                .map(|j| j.member.display_name.clone())
        });
    Some(PushEvent {
        id: format!("approval:{space}:{}", e.seq),
        kind: EventKind::ApprovalNeeded,
        space: Some(phone_space(space)),
        entry: Some(e.seq),
        agent,
        scope_choices: crate::thread::grants::scope_choices(e)
            .iter()
            .map(|c| c.to_string())
            .collect(),
        at_ms: e.ts_ms,
    })
}

fn on_entries(space: &str, entries: &[ThreadEntry]) {
    for e in entries {
        if let Some(ev) = event_of_entry(space, e, entries) {
            dispatch(ev);
        }
    }
}

/// The event a notification makes, if any: a turn that ended, a run that
/// failed, an agent that needs the person to connect it, a request that
/// waits.
pub fn event_of_notification(n: &Notification) -> Option<PushEvent> {
    let kind = match n.kind.as_str() {
        "turn_ended" => EventKind::TaskFinished,
        "error" if cua_agents::failure::is_auth_failure(&n.body) => EventKind::AgentBlocked,
        "error" => EventKind::TaskFailed,
        "approval" => EventKind::ApprovalNeeded,
        // A message an agent chose to send and a cloud's budget alert are
        // for the desktop's notification; their words are not ours to send.
        _ => return None,
    };
    Some(PushEvent {
        id: format!("note:{}", n.id),
        kind,
        space: n.space.as_deref().map(phone_space),
        entry: n.entry_seq,
        agent: n.agent.clone(),
        scope_choices: vec![],
        at_ms: n.at_ms,
    })
}

fn on_notification(n: &Notification) {
    if let Some(ev) = event_of_notification(n) {
        dispatch(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{Approval, Author, Draft, ThreadStore};

    fn store() -> (tempfile::TempDir, ThreadStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = ThreadStore::open(dir.path(), "local:push-test");
        (dir, s)
    }

    fn approval(s: &ThreadStore, author: Author, action: &str) -> ThreadEntry {
        let mut d = Draft::new(author, EntryKind::Approval, "asks");
        d.approval = Some(Approval {
            action: action.into(),
            details: "SECRET DETAIL".into(),
            state: "open".into(),
            card: None,
        });
        s.append(d).unwrap()
    }

    #[test]
    fn an_open_approval_is_an_event_without_its_words() {
        let (_d, s) = store();
        let e = approval(&s, Author::agent("claude"), "install a package");
        let ev = event_of_entry("local:push-test", &e, std::slice::from_ref(&e)).unwrap();
        assert_eq!(ev.kind, EventKind::ApprovalNeeded);
        assert_eq!(ev.entry, Some(e.seq));
        assert_eq!(ev.agent.as_deref(), Some("claude"));
        assert_eq!(ev.scope_choices, ["once", "space"]);
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            !json.contains("SECRET DETAIL") && !json.contains("install a package"),
            "{json}"
        );
    }

    #[test]
    fn an_approval_a_standing_answer_covered_asks_nobody() {
        let (_d, s) = store();
        let e = approval(&s, Author::agent("claude"), "install a package");
        let mut d = Draft::new(Author::system("grant"), EntryKind::Answer, "");
        d.answer = Some(crate::thread::Answer {
            approval: e.seq,
            approve: true,
            ..Default::default()
        });
        let a = s.append(d).unwrap();
        assert!(event_of_entry("local:push-test", &e, &[e.clone(), a]).is_none());
        // A message is not an event.
        let m = s
            .append(Draft::new(Author::owner(), EntryKind::Message, "hi"))
            .unwrap();
        assert!(event_of_entry("local:push-test", &m, std::slice::from_ref(&m)).is_none());
    }

    fn note(kind: &str, body: &str) -> Notification {
        Notification {
            id: "n1".into(),
            at_ms: 5,
            agent: Some("ada".into()),
            kind: kind.into(),
            title: "TITLE WITH MODEL WORDS".into(),
            body: body.into(),
            run_id: None,
            space: Some("local:push-test".into()),
            read: false,
            entry_seq: Some(9),
        }
    }

    #[test]
    fn notifications_map_to_the_four_events() {
        assert_eq!(
            event_of_notification(&note("turn_ended", "Your research is ready."))
                .unwrap()
                .kind,
            EventKind::TaskFinished
        );
        assert_eq!(
            event_of_notification(&note("error", "rate limit reached"))
                .unwrap()
                .kind,
            EventKind::TaskFailed
        );
        assert_eq!(
            event_of_notification(&note("error", "Invalid API key · Please run /login"))
                .unwrap()
                .kind,
            EventKind::AgentBlocked
        );
        assert_eq!(
            event_of_notification(&note("approval", "")).unwrap().kind,
            EventKind::ApprovalNeeded
        );
        assert!(event_of_notification(&note("message", "hello")).is_none());
        assert!(event_of_notification(&note("budget", "80%")).is_none());
        let ev = event_of_notification(&note("turn_ended", "Your research is ready.")).unwrap();
        let json = serde_json::to_string(&ev).unwrap();
        assert!(
            !json.contains("research") && !json.contains("MODEL WORDS"),
            "{json}"
        );
        assert_eq!(ev.entry, Some(9));
    }
}
