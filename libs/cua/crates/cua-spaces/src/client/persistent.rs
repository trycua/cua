//! Persistent agents, routines, notifications and per-agent computer
//! access behind typed calls (the `persistent-agents` tools).
//!
//! Records come back as JSON objects with the field names the contract's
//! `result` documents, so a binding reads them without a second model.

use serde_json::{Map, Value, json};

use crate::client::control::Connection;
use crate::client::error::Result;
use crate::client::transport::expect_array;

/// One JSON record.
pub type Record = Map<String, Value>;

/// When a routine fires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutineSchedule {
    EveryMinutes(i64),
    /// `HH:MM`, local time.
    DailyAt(String),
    /// `<weekday> HH:MM`, local time.
    WeeklyOn(String),
}

fn records(payload: &Value, key: &str) -> Vec<Record> {
    expect_array(payload, Some(key))
        .into_iter()
        .filter_map(|v| v.as_object().cloned())
        .collect()
}

impl Connection {
    /// `persistent_agent_create`.
    pub fn create_persistent_agent(
        &self,
        name: &str,
        harness: &str,
        space_id: &str,
        model: Option<&str>,
        base_url: Option<&str>,
        env_from_host: &[String],
    ) -> Result<Record> {
        self.call_object(
            "persistent_agent_create",
            json!({"name": name, "agent": harness, "space": space_id, "model": model,
                   "base_url": base_url, "env_from_host": env_from_host}),
        )
    }

    /// `persistent_agent_list`.
    pub fn persistent_agents(&self) -> Result<Vec<Record>> {
        Ok(records(
            &self.call_json("persistent_agent_list", json!({}))?,
            "agents",
        ))
    }

    /// `persistent_agent_remove`.
    pub fn remove_persistent_agent(&self, name: &str) -> Result<Record> {
        self.call_object("persistent_agent_remove", json!({"name": name}))
    }

    /// `persistent_agent_send`.
    pub fn send_to_persistent_agent(&self, name: &str, text: &str) -> Result<Record> {
        self.call_object("persistent_agent_send", json!({"name": name, "text": text}))
    }

    /// `persistent_agent_save`.
    pub fn save_agent_home(&self, name: &str) -> Result<Record> {
        self.call_object("persistent_agent_save", json!({"name": name}))
    }

    /// `agent_pause`.
    pub fn pause_agent(&self, name: &str) -> Result<Record> {
        self.call_object("agent_pause", json!({"name": name}))
    }

    /// `agent_resume`.
    pub fn resume_agent(&self, name: &str, prompt: Option<&str>) -> Result<Record> {
        self.call_object("agent_resume", json!({"name": name, "prompt": prompt}))
    }

    /// `routine_add`.
    pub fn add_routine(
        &self,
        agent: &str,
        title: &str,
        prompt: &str,
        schedule: &RoutineSchedule,
    ) -> Result<Record> {
        let mut args = json!({"agent": agent, "title": title, "prompt": prompt});
        match schedule {
            RoutineSchedule::EveryMinutes(m) => args["every_minutes"] = json!(m),
            RoutineSchedule::DailyAt(t) => args["daily_at"] = json!(t),
            RoutineSchedule::WeeklyOn(w) => args["weekly_on"] = json!(w),
        }
        self.call_object("routine_add", args)
    }

    /// `routine_list`.
    pub fn routines(&self, agent: Option<&str>) -> Result<Vec<Record>> {
        Ok(records(
            &self.call_json("routine_list", json!({"agent": agent}))?,
            "routines",
        ))
    }

    /// `routine_remove`.
    pub fn remove_routine(&self, id: &str) -> Result<()> {
        self.call_object("routine_remove", json!({"id": id}))?;
        Ok(())
    }

    /// `routine_set_enabled`.
    pub fn set_routine_enabled(&self, id: &str, enabled: bool) -> Result<Record> {
        self.call_object("routine_set_enabled", json!({"id": id, "enabled": enabled}))
    }

    /// `notify_user`. Returns the notification id.
    pub fn notify_user(&self, title: &str, body: &str) -> Result<String> {
        let r = self.call_object("notify_user", json!({"title": title, "body": body}))?;
        Ok(r.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// `notifications_list`.
    pub fn notifications(&self, unread_only: bool) -> Result<Vec<Record>> {
        Ok(records(
            &self.call_json("notifications_list", json!({"unread_only": unread_only}))?,
            "notifications",
        ))
    }

    /// `notifications_ack`. Returns how many changed.
    pub fn mark_notifications_read(&self, ids: &[String]) -> Result<u64> {
        let r = self.call_object("notifications_ack", json!({"ids": ids}))?;
        Ok(r.get("marked").and_then(Value::as_u64).unwrap_or(0))
    }

    /// `computer_access_grant`.
    pub fn allow_computer(&self, agent: &str, machine: &str) -> Result<Record> {
        self.call_object(
            "computer_access_grant",
            json!({"agent": agent, "machine": machine}),
        )
    }

    /// `computer_access_revoke`. Returns how many grants.
    pub fn revoke_computer(&self, agent: &str, machine: Option<&str>) -> Result<u64> {
        let r = self.call_object(
            "computer_access_revoke",
            json!({"agent": agent, "machine": machine}),
        )?;
        Ok(r.get("revoked").and_then(Value::as_u64).unwrap_or(0))
    }

    /// `computer_access_list`.
    pub fn computer_access(&self, agent: Option<&str>) -> Result<Vec<Record>> {
        Ok(records(
            &self.call_json("computer_access_list", json!({"agent": agent}))?,
            "grants",
        ))
    }
}
