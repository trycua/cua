//! A Space's thread and its agent members behind typed calls (the `thread`
//! tools). Records come back as the JSON the contract's `result` documents.

use serde_json::{Value, json};

use crate::client::control::Connection;
use crate::client::error::Result;
use crate::client::persistent::Record;

impl Connection {
    /// `thread_read`.
    pub fn thread_read(
        &self,
        space_id: &str,
        after: u64,
        limit: Option<u32>,
        mark_read: bool,
        follow_ms: Option<u64>,
    ) -> Result<Record> {
        self.call_object(
            "thread_read",
            json!({"space": space_id, "after": after, "limit": limit,
                   "mark_read": mark_read, "follow_ms": follow_ms}),
        )
    }

    /// `thread_post`.
    pub fn thread_post(
        &self,
        space_id: &str,
        text: &str,
        reply_to: Option<u64>,
        attachments: &[String],
    ) -> Result<Record> {
        self.call_object(
            "thread_post",
            json!({"space": space_id, "text": text, "reply_to": reply_to,
                   "attachments": attachments}),
        )
    }

    /// `thread_inbox`.
    pub fn thread_inbox(&self, space_id: &str, member: Option<&str>) -> Result<Record> {
        self.call_object("thread_inbox", json!({"space": space_id, "member": member}))
    }

    /// `thread_claim`.
    pub fn thread_claim(&self, space_id: &str, task: u64) -> Result<Record> {
        self.call_object("thread_claim", json!({"space": space_id, "task": task}))
    }

    /// `thread_reply`.
    pub fn thread_reply(
        &self,
        space_id: &str,
        task: u64,
        text: &str,
        state: Option<&str>,
    ) -> Result<Record> {
        self.call_object(
            "thread_reply",
            json!({"space": space_id, "task": task, "text": text, "state": state}),
        )
    }

    /// `thread_import`: `entries` as the tool's input documents them.
    pub fn thread_import(&self, space_id: &str, source: &str, entries: Value) -> Result<Record> {
        self.call_object(
            "thread_import",
            json!({"space": space_id, "source": source, "entries": entries}),
        )
    }

    /// `agent_members`.
    pub fn agent_members(&self, space_id: &str) -> Result<Record> {
        self.call_object("agent_members", json!({"space": space_id}))
    }

    /// `agent_set_rights`: `rights` is `{task?, control?, approve?}`, each
    /// `owner`, `admins`, `members` or a list of account ids.
    pub fn agent_set_rights(&self, name: &str, rights: Value) -> Result<Record> {
        let mut args = json!({"name": name});
        if let (Some(a), Some(r)) = (args.as_object_mut(), rights.as_object()) {
            for (k, v) in r {
                a.insert(k.clone(), v.clone());
            }
        }
        self.call_object("agent_set_rights", args)
    }

    /// `thread_join`: an agent member sets its name, harness, version and
    /// icon (an agent's own call; `None` leaves a field out).
    pub fn thread_join(
        &self,
        space_id: &str,
        name: &str,
        harness: Option<&str>,
        version: Option<&str>,
        icon: Option<&str>,
    ) -> Result<Record> {
        self.call_object(
            "thread_join",
            json!({"space": space_id, "name": name, "harness": harness, "version": version, "icon": icon}),
        )
    }

    /// `thread_join_code_create`.
    pub fn thread_join_code_create(
        &self,
        space_id: &str,
        ttl_s: Option<u64>,
        can_task: Option<&str>,
        ask_before_join: Option<bool>,
    ) -> Result<Record> {
        self.call_object(
            "thread_join_code_create",
            json!({"space": space_id, "ttl_s": ttl_s, "can_task": can_task, "ask_before_join": ask_before_join}),
        )
    }

    /// `thread_join_code_list`.
    pub fn thread_join_code_list(&self, space_id: &str) -> Result<Record> {
        self.call_object("thread_join_code_list", json!({"space": space_id}))
    }

    /// `thread_join_code_cancel`.
    pub fn thread_join_code_cancel(&self, space_id: &str, id: &str) -> Result<Record> {
        self.call_object(
            "thread_join_code_cancel",
            json!({"space": space_id, "id": id}),
        )
    }

    /// `thread_join_redeem`: `device` is `{id, name, public_key?}`. The
    /// result carries the member key once.
    pub fn thread_join_redeem(&self, code: &str, device: Value) -> Result<Record> {
        self.call_object(
            "thread_join_redeem",
            json!({"code": code, "device": device}),
        )
    }

    /// `thread_settings` (reads when `ask_before_join` is `None`).
    pub fn thread_settings(&self, space_id: &str, ask_before_join: Option<bool>) -> Result<Record> {
        self.call_object(
            "thread_settings",
            json!({"space": space_id, "ask_before_join": ask_before_join}),
        )
    }

    /// `thread_device_revoke`.
    pub fn thread_device_revoke(&self, device: &str) -> Result<Record> {
        self.call_object("thread_device_revoke", json!({"device": device}))
    }
}
