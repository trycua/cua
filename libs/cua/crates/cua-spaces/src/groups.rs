//! Group chats: one human and two to six Bots in one thread.
//!
//! A group chat is a fan-out over the members' own agent threads in the one
//! shared Space plus a merged, attributed transcript; it is not a new kind of
//! agent. The harness gives a Bot no way to know who else is in the room, so
//! every message is framed with the room ([`frame_message`]). Delivery is per
//! member and partial success is normal: a Bot mid-turn refuses, and the
//! refusal lands in the transcript as a marked line.
//!
//! The same model as `@trycua/cua/spaces/groups` and the Swift SDK's
//! `GroupChatStore`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// The product bound: a group of one is a thread; every message fans out to
/// every member.
pub const MIN_BOTS: usize = 2;
pub const MAX_BOTS: usize = 6;

/// A membership rule was broken. The message says which number.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(tag = "code", content = "value", rename_all = "camelCase")]
pub enum GroupChatError {
    #[error("A group chat needs at least {MIN_BOTS} bots; {0} selected.")]
    TooFewBots(usize),
    #[error("A group chat holds at most {MAX_BOTS} bots; {0} selected.")]
    TooManyBots(usize),
    #[error("This group is full: {MAX_BOTS} bots is the limit. Remove one to add another.")]
    Full,
    #[error("A group chat needs at least {MIN_BOTS} bots. Add one before removing this one.")]
    AtFloor,
    #[error("{0} is already in this group.")]
    AlreadyAMember(String),
    #[error("{0} is not in this group.")]
    NotAMember(String),
    #[error("No such group chat: {0}")]
    UnknownChat(String),
}

/// Who said a line: the human, a Bot, or the group itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GroupSpeaker {
    Human,
    Bot {
        #[serde(rename = "botID")]
        bot_id: String,
    },
    System,
}

/// One line of a group transcript.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupMessage {
    pub id: String,
    pub speaker: GroupSpeaker,
    pub text: String,
    #[serde(with = "crate::routines::iso")]
    pub at: DateTime<Utc>,
    /// This line records a message that did not reach its Bot.
    pub undelivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction: Option<String>,
}

/// The result of fanning one message out to one member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GroupDelivery {
    #[serde(rename = "botID")]
    pub bot_id: String,
    pub accepted: bool,
    pub reason: String,
}

/// How a group reaches its Bots. `deliver` never errors: a refusal is a
/// result.
#[async_trait::async_trait]
pub trait GroupMessenger: Send + Sync {
    async fn deliver(&self, text: &str, bot_id: &str) -> GroupDelivery;
    /// The Bot's most recent utterance, if any.
    async fn latest_reply(&self, bot_id: &str) -> Option<String>;
    /// Whether the Bot is producing output right now (the typing row).
    async fn is_working(&self, bot_id: &str) -> bool;
    async fn display_name(&self, bot_id: &str) -> String;
}

/// A group chat. The 2..6 bound holds for every value: construction and
/// membership changes refuse rather than clamp.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupChat {
    pub id: String,
    pub title: String,
    #[serde(rename = "memberIDs")]
    member_ids: Vec<String>,
    #[serde(rename = "createdAt", with = "crate::routines::iso")]
    pub created_at: DateTime<Utc>,
    pub messages: Vec<GroupMessage>,
}

fn dedupe(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in ids {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// Whether a Create button should be enabled for `members`.
pub fn can_create(members: &[String]) -> bool {
    (MIN_BOTS..=MAX_BOTS).contains(&dedupe(members).len())
}

impl GroupChat {
    pub fn new(title: &str, members: &[String]) -> Result<Self, GroupChatError> {
        let d = dedupe(members);
        if d.len() < MIN_BOTS {
            return Err(GroupChatError::TooFewBots(d.len()));
        }
        if d.len() > MAX_BOTS {
            return Err(GroupChatError::TooManyBots(d.len()));
        }
        Ok(Self {
            id: crate::routines::new_id(),
            title: title.into(),
            member_ids: d,
            created_at: Utc::now(),
            messages: vec![],
        })
    }

    /// Bot ids in join order. The human is implicit.
    pub fn member_ids(&self) -> &[String] {
        &self.member_ids
    }
    pub fn is_full(&self) -> bool {
        self.member_ids.len() >= MAX_BOTS
    }
    pub fn is_at_floor(&self) -> bool {
        self.member_ids.len() <= MIN_BOTS
    }
    /// `4 of 6 bots`.
    pub fn membership_label(&self) -> String {
        format!("{} of {MAX_BOTS} bots", self.member_ids.len())
    }
    pub fn remaining_seats(&self) -> usize {
        MAX_BOTS.saturating_sub(self.member_ids.len())
    }

    pub fn add(&mut self, bot_id: &str) -> Result<(), GroupChatError> {
        if self.member_ids.iter().any(|m| m == bot_id) {
            return Err(GroupChatError::AlreadyAMember(bot_id.into()));
        }
        if self.is_full() {
            return Err(GroupChatError::Full);
        }
        self.member_ids.push(bot_id.into());
        Ok(())
    }

    pub fn remove(&mut self, bot_id: &str) -> Result<(), GroupChatError> {
        if !self.member_ids.iter().any(|m| m == bot_id) {
            return Err(GroupChatError::NotAMember(bot_id.into()));
        }
        if self.is_at_floor() {
            return Err(GroupChatError::AtFloor);
        }
        self.member_ids.retain(|m| m != bot_id);
        Ok(())
    }

    fn line(&mut self, speaker: GroupSpeaker, text: String, undelivered: bool) {
        self.messages.push(GroupMessage {
            id: crate::routines::new_id(),
            speaker,
            text,
            at: Utc::now(),
            undelivered,
            reaction: None,
        });
    }
}

/// The framing each member receives: the room, then the human's words on
/// the last line. `others` are the other members' display names.
pub fn frame_message(text: &str, title: &str, others: &[String]) -> String {
    format!(
        "[group:{title}] You are in a group chat with the user and {}. Answer for your own area only and keep it to a few lines.\n{text}",
        others.join(", ")
    )
}

/// Group chats: membership, fan-out, and the merged transcript.
#[derive(Default)]
pub struct GroupChatStore {
    pub chats: Vec<GroupChat>,
    /// The last membership complaint, cleared on the next success.
    pub last_error: Option<String>,
    /// Bots producing output right now, per chat.
    pub working: HashMap<String, BTreeSet<String>>,
    consumed: HashMap<String, HashMap<String, String>>,
    messenger: Option<Arc<dyn GroupMessenger>>,
}

impl GroupChatStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach(&mut self, messenger: Arc<dyn GroupMessenger>) {
        self.messenger = Some(messenger);
    }

    pub fn chat(&self, id: &str) -> Option<&GroupChat> {
        self.chats.iter().find(|c| c.id == id)
    }

    fn chat_mut(&mut self, id: &str) -> Result<&mut GroupChat, GroupChatError> {
        self.chats
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| GroupChatError::UnknownChat(id.into()))
    }

    async fn name(&self, bot_id: &str) -> String {
        match &self.messenger {
            Some(m) => m.display_name(bot_id).await,
            None => bot_id.into(),
        }
    }

    pub fn create(&mut self, title: &str, members: &[String]) -> Result<GroupChat, GroupChatError> {
        let chat = GroupChat::new(title, members)?;
        self.chats.push(chat.clone());
        self.last_error = None;
        Ok(chat)
    }

    pub fn delete(&mut self, id: &str) {
        self.chats.retain(|c| c.id != id);
        self.consumed.remove(id);
        self.working.remove(id);
    }

    pub async fn add(&mut self, bot_id: &str, chat_id: &str) -> Result<(), GroupChatError> {
        let name = self.name(bot_id).await;
        self.mutate(
            chat_id,
            |c| c.add(bot_id),
            |c| format!("{name} joined, {}.", c.membership_label()),
        )
    }

    pub async fn remove(&mut self, bot_id: &str, chat_id: &str) -> Result<(), GroupChatError> {
        let name = self.name(bot_id).await;
        self.mutate(
            chat_id,
            |c| c.remove(bot_id),
            |c| format!("{name} left, {}.", c.membership_label()),
        )
    }

    fn mutate(
        &mut self,
        chat_id: &str,
        op: impl FnOnce(&mut GroupChat) -> Result<(), GroupChatError>,
        announce: impl FnOnce(&GroupChat) -> String,
    ) -> Result<(), GroupChatError> {
        let chat = self.chat_mut(chat_id)?;
        match op(chat) {
            Ok(()) => {
                let text = announce(chat);
                chat.line(GroupSpeaker::System, text, false);
                self.last_error = None;
                Ok(())
            }
            Err(e) => {
                chat.line(GroupSpeaker::System, e.to_string(), true);
                self.last_error = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// Sends one message to every member; returns each member's delivery.
    pub async fn send(&mut self, text: &str, chat_id: &str) -> Vec<GroupDelivery> {
        let Ok(chat) = self.chat_mut(chat_id) else {
            return vec![];
        };
        chat.line(GroupSpeaker::Human, text.into(), false);
        let (title, members) = (chat.title.clone(), chat.member_ids.clone());
        let mut names = HashMap::new();
        for m in &members {
            names.insert(m.clone(), self.name(m).await);
        }
        let mut out = vec![];
        for bot in &members {
            let others: Vec<String> = members
                .iter()
                .filter(|m| *m != bot)
                .map(|m| names[m].clone())
                .collect();
            let framed = frame_message(text, &title, &others);
            let d = match &self.messenger {
                Some(m) => m.deliver(&framed, bot).await,
                None => GroupDelivery {
                    bot_id: bot.clone(),
                    accepted: false,
                    reason: "not connected to a Space".into(),
                },
            };
            if !d.accepted
                && let Ok(chat) = self.chat_mut(chat_id)
            {
                chat.line(
                    GroupSpeaker::Bot {
                        bot_id: bot.clone(),
                    },
                    format!("Did not receive that message: {}", d.reason),
                    true,
                );
            }
            out.push(d);
        }
        self.refresh_working(chat_id).await;
        out
    }

    /// Folds what members said since the last poll into the transcript,
    /// attributed. Returns the new lines.
    pub async fn collect_replies(&mut self, chat_id: &str) -> Vec<GroupMessage> {
        let (Some(chat), Some(m)) = (self.chat(chat_id), self.messenger.clone()) else {
            return vec![];
        };
        let members = chat.member_ids.clone();
        let mut added = vec![];
        for bot in members {
            let Some(reply) = m.latest_reply(&bot).await else {
                continue;
            };
            let reply = reply.trim().to_string();
            if reply.is_empty() {
                continue;
            }
            let seen = self.consumed.entry(chat_id.into()).or_default();
            if seen.get(&bot) == Some(&reply) {
                continue;
            }
            seen.insert(bot.clone(), reply.clone());
            if let Ok(chat) = self.chat_mut(chat_id) {
                chat.line(GroupSpeaker::Bot { bot_id: bot }, reply, false);
                added.extend(chat.messages.last().cloned());
            }
        }
        self.refresh_working(chat_id).await;
        added
    }

    /// Attaches a reaction to the last line.
    pub fn react(&mut self, emoji: &str, chat_id: &str) {
        if let Ok(chat) = self.chat_mut(chat_id)
            && let Some(last) = chat.messages.last_mut()
        {
            last.reaction = Some(emoji.into());
        }
    }

    pub async fn refresh_working(&mut self, chat_id: &str) {
        let (Some(chat), Some(m)) = (self.chat(chat_id), self.messenger.clone()) else {
            return;
        };
        let mut set = BTreeSet::new();
        for b in chat.member_ids.clone() {
            if m.is_working(&b).await {
                set.insert(b);
            }
        }
        self.working.insert(chat_id.into(), set);
    }

    pub fn working_bots(&self, chat_id: &str) -> Vec<String> {
        self.working
            .get(chat_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Messenger {
        delivered: Mutex<Vec<(String, String)>>,
        replies: Mutex<HashMap<String, String>>,
        busy: Mutex<BTreeSet<String>>,
        refuse: BTreeSet<String>,
    }
    #[async_trait::async_trait]
    impl GroupMessenger for Messenger {
        async fn deliver(&self, text: &str, bot_id: &str) -> GroupDelivery {
            self.delivered
                .lock()
                .unwrap()
                .push((text.into(), bot_id.into()));
            let refused = self.refuse.contains(bot_id);
            GroupDelivery {
                bot_id: bot_id.into(),
                accepted: !refused,
                reason: if refused { "mid-turn" } else { "delivered" }.into(),
            }
        }
        async fn latest_reply(&self, bot_id: &str) -> Option<String> {
            self.replies.lock().unwrap().get(bot_id).cloned()
        }
        async fn is_working(&self, bot_id: &str) -> bool {
            self.busy.lock().unwrap().contains(bot_id)
        }
        async fn display_name(&self, bot_id: &str) -> String {
            match bot_id {
                "ada" => "Ada".into(),
                "bo" => "Bo".into(),
                "cy" => "Cy".into(),
                o => o.into(),
            }
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn bounds_hold_on_every_mutation() {
        assert_eq!(
            GroupChat::new("x", &ids(&["ada"])).unwrap_err(),
            GroupChatError::TooFewBots(1)
        );
        assert_eq!(
            GroupChat::new("x", &ids(&["a", "b", "c", "d", "e", "f", "g"])).unwrap_err(),
            GroupChatError::TooManyBots(7)
        );
        assert_eq!(
            GroupChat::new("x", &ids(&["a", "a"])).unwrap_err(),
            GroupChatError::TooFewBots(1)
        );
        assert!(can_create(&ids(&["a", "a", "b"])));
        let mut s = GroupChatStore::new();
        s.attach(Arc::new(Messenger::default()));
        let full = s
            .create("Full", &ids(&["a", "b", "c", "d", "e", "f"]))
            .unwrap();
        assert_eq!(full.membership_label(), "6 of 6 bots");
        assert_eq!(
            s.add("g", &full.id).await.unwrap_err(),
            GroupChatError::Full
        );
        assert!(
            s.chat(&full.id)
                .unwrap()
                .messages
                .last()
                .unwrap()
                .undelivered
        );
        assert!(
            s.last_error
                .as_deref()
                .unwrap()
                .contains("6 bots is the limit")
        );
        let pair = s.create("Pair", &ids(&["ada", "bo"])).unwrap();
        assert_eq!(
            s.remove("ada", &pair.id).await.unwrap_err(),
            GroupChatError::AtFloor
        );
        s.add("cy", &pair.id).await.unwrap();
        assert_eq!(
            s.chat(&pair.id).unwrap().messages.last().unwrap().text,
            "Cy joined, 3 of 6 bots."
        );
        assert_eq!(s.last_error, None);
        assert_eq!(
            GroupChatError::TooFewBots(1).to_string(),
            "A group chat needs at least 2 bots; 1 selected."
        );
    }

    #[tokio::test]
    async fn fans_out_framed_and_shows_refusals() {
        let m = Arc::new(Messenger {
            refuse: ["bo".to_string()].into(),
            ..Default::default()
        });
        let mut s = GroupChatStore::new();
        s.attach(m.clone());
        let chat = s.create("Launch", &ids(&["ada", "bo", "cy"])).unwrap();
        let ds = s.send("Status?", &chat.id).await;
        assert_eq!(
            ds.iter()
                .map(|d| (d.bot_id.as_str(), d.accepted))
                .collect::<Vec<_>>(),
            [("ada", true), ("bo", false), ("cy", true)]
        );
        let first = m.delivered.lock().unwrap()[0].0.clone();
        assert!(
            first.starts_with("[group:Launch] You are in a group chat with the user and Bo, Cy.")
        );
        assert_eq!(first.lines().last(), Some("Status?"));
        let c = s.chat(&chat.id).unwrap();
        let refused = c.messages.iter().find(|l| l.undelivered).unwrap();
        assert_eq!(
            refused.speaker,
            GroupSpeaker::Bot {
                bot_id: "bo".into()
            }
        );
        assert!(
            refused
                .text
                .contains("Did not receive that message: mid-turn")
        );
    }

    #[tokio::test]
    async fn replies_are_attributed_once() {
        let m = Arc::new(Messenger::default());
        let mut s = GroupChatStore::new();
        s.attach(m.clone());
        let chat = s.create("Launch", &ids(&["ada", "bo"])).unwrap();
        m.replies
            .lock()
            .unwrap()
            .insert("ada".into(), " shipped \n".into());
        m.busy.lock().unwrap().insert("bo".into());
        let added = s.collect_replies(&chat.id).await;
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0].speaker,
            GroupSpeaker::Bot {
                bot_id: "ada".into()
            }
        );
        assert_eq!(added[0].text, "shipped");
        assert_eq!(s.working_bots(&chat.id), ["bo"]);
        assert!(s.collect_replies(&chat.id).await.is_empty());
        m.replies
            .lock()
            .unwrap()
            .insert("ada".into(), "and tested".into());
        assert_eq!(s.collect_replies(&chat.id).await.len(), 1);
        s.react("+1", &chat.id);
        assert_eq!(
            s.chat(&chat.id)
                .unwrap()
                .messages
                .last()
                .unwrap()
                .reaction
                .as_deref(),
            Some("+1")
        );
        let json = serde_json::to_value(s.chat(&chat.id).unwrap()).unwrap();
        assert_eq!(json["memberIDs"], serde_json::json!(["ada", "bo"]));
        assert_eq!(
            json["messages"][0]["speaker"],
            serde_json::json!({"kind": "bot", "botID": "ada"})
        );
    }
}
