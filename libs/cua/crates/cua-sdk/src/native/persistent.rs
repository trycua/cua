//! Persistent agents, routines, notifications and per-agent computer
//! access on [`Spaces`] (the Spaces `persistent-agents` tools, in process or
//! in the daemon).

use serde::Deserialize;
use serde_json::{Value, json};

use super::run;
use super::spaces::{Spaces, parse};
use crate::Result;

/// A persistent agent: a named harness whose memory (its home in the Cua
/// Drive) outlives its runs and its Space.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct PersistentAgentInfo {
    pub name: String,
    /// Harness id.
    pub harness: String,
    /// The Space it works in.
    pub space: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub env_from_host: Vec<String>,
    #[serde(default)]
    pub paused: bool,
    /// `running`, `suspended` (a paused local Space) or `released` (a
    /// paused cloud Space).
    #[serde(default)]
    pub space_state: String,
    /// The current run.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Unix ms of the last home save.
    #[serde(default)]
    pub saved_ms: u64,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// How to create a persistent agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct PersistentAgentOptions {
    #[uniffi(default = None)]
    pub model: Option<String>,
    #[uniffi(default = None)]
    pub base_url: Option<String>,
    /// Provider key variables forwarded from the host's environment at each
    /// start.
    #[uniffi(default)]
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets).
    #[uniffi(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// What moved between the drive and a Space.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, uniffi::Record)]
pub struct HomeTransfer {
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub unchanged: u64,
    #[serde(default)]
    pub removed: u64,
    /// `path (kind)` of each file the secret scanner kept out.
    #[serde(default, deserialize_with = "blocked")]
    pub blocked: Vec<String>,
    #[serde(default)]
    pub millis: u64,
}

fn blocked<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    let pairs: Vec<(String, String)> = Deserialize::deserialize(d)?;
    Ok(pairs
        .into_iter()
        .map(|(p, k)| format!("{p} ({k})"))
        .collect())
}

/// What [`Spaces::persistent_agent_send`] did.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct AgentDelivery {
    pub run_id: String,
    /// A new run started (with the home restored) rather than a follow-up.
    pub started: bool,
    #[serde(default)]
    pub restored: Option<HomeTransfer>,
}

/// What [`Spaces::agent_pause`] did.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct AgentPauseReport {
    #[serde(default)]
    pub stopped_run: Option<String>,
    #[serde(default)]
    pub saved: Option<HomeTransfer>,
    pub space_state: String,
    #[serde(default)]
    pub millis: u64,
}

/// What [`Spaces::agent_resume`] did.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct AgentResumeReport {
    pub space: String,
    #[serde(default)]
    pub recreated: bool,
    #[serde(default)]
    pub restored: Option<HomeTransfer>,
    #[serde(default)]
    pub run_id: Option<String>,
    /// From the call to the home being back in a ready Space.
    #[serde(default)]
    pub ready_ms: u64,
}

/// A routine: a recurring turn of a persistent agent, fired by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoutineInfo {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub prompt: String,
    /// `Every day at 8:00 AM`.
    pub label: String,
    pub enabled: bool,
    /// RFC 3339.
    pub next_fire: Option<String>,
    pub last_fired_at: Option<String>,
    pub last_outcome: Option<String>,
    pub last_run_id: Option<String>,
}

impl RoutineInfo {
    fn from_json(v: &Value) -> RoutineInfo {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        RoutineInfo {
            id: s("id").unwrap_or_default(),
            agent: s("botID").unwrap_or_default(),
            title: s("title").unwrap_or_default(),
            prompt: s("prompt").unwrap_or_default(),
            label: s("label").unwrap_or_default(),
            enabled: v.get("isEnabled").and_then(Value::as_bool).unwrap_or(true),
            next_fire: s("next_fire"),
            last_fired_at: s("lastFiredAt"),
            last_outcome: s("lastOutcome"),
            last_run_id: s("lastRunID"),
        }
    }
}

/// One entry of the notifications feed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct NotificationInfo {
    pub id: String,
    pub at_ms: u64,
    #[serde(default)]
    pub agent: Option<String>,
    /// `turn_ended`, `message`, `approval` or `error`.
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub read: bool,
}

/// A persistent agent's grant on one of the user's computers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, uniffi::Record)]
pub struct ComputerGrant {
    pub id: String,
    pub agent: String,
    pub machine: String,
    pub created_ms: u64,
    #[serde(default)]
    pub expires_ms: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
}

fn list<T: serde::de::DeserializeOwned>(v: &Value, key: &str) -> Result<Vec<T>> {
    parse(v.get(key).cloned().unwrap_or(json!([])))
}

#[uniffi::export]
impl Spaces {
    /// Creates a persistent agent: `harness` working in `space`, its home at
    /// `agents/<name>/` in the Cua Volume.
    pub async fn persistent_agent_create(
        &self,
        name: String,
        harness: String,
        space: String,
        options: Option<PersistentAgentOptions>,
    ) -> Result<PersistentAgentInfo> {
        let host = self.tool_caller();
        let o = options.unwrap_or_default();
        run(async move {
            parse(
                host.tool_value(
                    "persistent_agent_create",
                    json!({"name": name, "agent": harness, "space": space, "model": o.model,
                           "base_url": o.base_url, "env_from_host": o.env_from_host, "env": o.env}),
                )
                .await?,
            )
        })
        .await
    }

    /// Every persistent agent.
    pub async fn persistent_agents(&self) -> Result<Vec<PersistentAgentInfo>> {
        let host = self.tool_caller();
        run(async move {
            list(
                &host.tool_value("persistent_agent_list", json!({})).await?,
                "agents",
            )
        })
        .await
    }

    /// Forgets a persistent agent (its home stays in the drive).
    pub async fn persistent_agent_remove(&self, name: String) -> Result<PersistentAgentInfo> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value("persistent_agent_remove", json!({"name": name}))
                .await?;
            parse(v.get("removed").cloned().unwrap_or(Value::Null))
        })
        .await
    }

    /// Gives a persistent agent a turn: a follow-up to its idle run, or a
    /// new run with its home restored.
    pub async fn persistent_agent_send(&self, name: String, text: String) -> Result<AgentDelivery> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("persistent_agent_send", json!({"name": name, "text": text}))
                    .await?,
            )
        })
        .await
    }

    /// Saves a persistent agent's home into the drive now.
    pub async fn persistent_agent_save(&self, name: String) -> Result<HomeTransfer> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("persistent_agent_save", json!({"name": name}))
                    .await?,
            )
        })
        .await
    }

    /// Pauses a persistent agent: run, routines and (local) Space.
    pub async fn agent_pause(&self, name: String) -> Result<AgentPauseReport> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("agent_pause", json!({"name": name}))
                    .await?,
            )
        })
        .await
    }

    /// Resumes a paused persistent agent; with `prompt`, starts a run.
    pub async fn agent_resume(
        &self,
        name: String,
        prompt: Option<String>,
    ) -> Result<AgentResumeReport> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value("agent_resume", json!({"name": name, "prompt": prompt}))
                    .await?,
            )
        })
        .await
    }

    /// Adds a routine. Give exactly one of `every_minutes`, `daily_at`
    /// (`HH:MM`) or `weekly_on` (`mon 09:00`).
    pub async fn routine_add(
        &self,
        agent: String,
        title: String,
        prompt: String,
        every_minutes: Option<i64>,
        daily_at: Option<String>,
        weekly_on: Option<String>,
    ) -> Result<RoutineInfo> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value(
                    "routine_add",
                    json!({"agent": agent, "title": title, "prompt": prompt,
                           "every_minutes": every_minutes, "daily_at": daily_at,
                           "weekly_on": weekly_on}),
                )
                .await?;
            Ok(RoutineInfo::from_json(&v))
        })
        .await
    }

    /// Routines (of one agent, or all).
    pub async fn routines(&self, agent: Option<String>) -> Result<Vec<RoutineInfo>> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value("routine_list", json!({"agent": agent}))
                .await?;
            Ok(v.get("routines")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(RoutineInfo::from_json).collect())
                .unwrap_or_default())
        })
        .await
    }

    /// Deletes a routine.
    pub async fn routine_remove(&self, id: String) -> Result<()> {
        let host = self.tool_caller();
        run(async move {
            host.tool_value("routine_remove", json!({"id": id})).await?;
            Ok(())
        })
        .await
    }

    /// Turns a routine on or off.
    pub async fn routine_set_enabled(&self, id: String, enabled: bool) -> Result<RoutineInfo> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value("routine_set_enabled", json!({"id": id, "enabled": enabled}))
                .await?;
            Ok(RoutineInfo::from_json(&v))
        })
        .await
    }

    /// Posts a notification to the Cua app. Returns its id.
    pub async fn notify_user(
        &self,
        title: String,
        body: Option<String>,
        agent: Option<String>,
    ) -> Result<String> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value(
                    "notify_user",
                    json!({"title": title, "body": body, "agent": agent}),
                )
                .await?;
            Ok(v.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        })
        .await
    }

    /// The notifications feed, newest first.
    pub async fn notifications(
        &self,
        unread_only: bool,
        since_ms: Option<u64>,
    ) -> Result<Vec<NotificationInfo>> {
        let host = self.tool_caller();
        run(async move {
            list(
                &host
                    .tool_value(
                        "notifications_list",
                        json!({"unread_only": unread_only, "since_ms": since_ms}),
                    )
                    .await?,
                "notifications",
            )
        })
        .await
    }

    /// Marks notifications read (every one when `ids` is empty).
    pub async fn notifications_ack(&self, ids: Vec<String>) -> Result<u64> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value("notifications_ack", json!({"ids": ids}))
                .await?;
            Ok(v.get("marked").and_then(Value::as_u64).unwrap_or(0))
        })
        .await
    }

    /// Lets one persistent agent use one of the user's computers (asks for
    /// presence).
    pub async fn computer_access_grant(
        &self,
        agent: String,
        machine: String,
        expires_in_secs: Option<u64>,
    ) -> Result<ComputerGrant> {
        let host = self.tool_caller();
        run(async move {
            parse(
                host.tool_value(
                    "computer_access_grant",
                    json!({"agent": agent, "machine": machine, "expires_in_secs": expires_in_secs}),
                )
                .await?,
            )
        })
        .await
    }

    /// Takes a persistent agent's computer access back (every machine when
    /// `machine` is empty). Returns how many grants.
    pub async fn computer_access_revoke(
        &self,
        agent: String,
        machine: Option<String>,
    ) -> Result<u64> {
        let host = self.tool_caller();
        run(async move {
            let v = host
                .tool_value(
                    "computer_access_revoke",
                    json!({"agent": agent, "machine": machine}),
                )
                .await?;
            Ok(v.get("revoked").and_then(Value::as_u64).unwrap_or(0))
        })
        .await
    }

    /// Per-agent computer grants (of one agent, or all).
    pub async fn computer_access(&self, agent: Option<String>) -> Result<Vec<ComputerGrant>> {
        let host = self.tool_caller();
        run(async move {
            list(
                &host
                    .tool_value("computer_access_list", json!({"agent": agent}))
                    .await?,
                "grants",
            )
        })
        .await
    }
}
