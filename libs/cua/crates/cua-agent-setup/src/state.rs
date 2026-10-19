//! `~/.cua/agent-setup.json`: what cua wrote, so `remove` only undoes cua's
//! own changes and `update` knows which skill copies it owns.

use crate::{Error, Result, edit::Format, fsutil};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// State file schema version.
pub const STATE_VERSION: u32 = 1;

/// One MCP entry cua wrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpRecord {
    /// Agents that use this config file.
    pub agents: BTreeSet<String>,
    /// File syntax.
    pub format: Format,
    /// Key path of the servers object.
    pub key_path: Vec<String>,
    /// Server name.
    pub name: String,
    /// The exact entry cua wrote.
    pub value: Value,
    /// Whether an entry of that name existed before cua first wrote it (then
    /// `remove` puts the previous value back instead of deleting it).
    #[serde(default)]
    pub previous: Option<Value>,
    /// RFC 3339.
    pub written_at: String,
}

/// One skill copy cua installed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillRecord {
    /// Skill name.
    pub skill: String,
    /// Bundled version at install time.
    pub version: String,
    /// Tree hash of what cua wrote (a differing hash means the user edited it).
    pub hash: String,
    /// Agents that read this skills directory.
    pub agents: BTreeSet<String>,
    /// RFC 3339.
    pub installed_at: String,
}

/// The whole state file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Schema version.
    #[serde(default)]
    pub version: u32,
    /// Config file path → the `cua` entry cua wrote.
    #[serde(default)]
    pub mcp: BTreeMap<PathBuf, McpRecord>,
    /// Other servers cua registered (`cua-driver`): name → config file
    /// path → entry cua wrote.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, BTreeMap<PathBuf, McpRecord>>,
    /// Skill directory → copy cua installed.
    #[serde(default)]
    pub skills: BTreeMap<PathBuf, SkillRecord>,
    /// Config file path → the backup taken before cua first wrote it.
    #[serde(default)]
    pub backups: BTreeMap<PathBuf, PathBuf>,
}

impl State {
    /// Loads `path`; a missing file is an empty state.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| Error::Malformed {
                path: path.to_path_buf(),
                detail: format!("invalid state file: {e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State {
                version: STATE_VERSION,
                ..Default::default()
            }),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// The records of server `name` (`cua` lives in [`State::mcp`]).
    pub fn records(&self, name: &str) -> Option<&BTreeMap<PathBuf, McpRecord>> {
        if name == crate::MCP_SERVER_NAME {
            Some(&self.mcp)
        } else {
            self.servers.get(name)
        }
    }

    /// The record of server `name` in `file`.
    pub fn record(&self, name: &str, file: &Path) -> Option<&McpRecord> {
        self.records(name).and_then(|r| r.get(file))
    }

    /// The records of server `name`, created on first use.
    pub fn records_mut(&mut self, name: &str) -> &mut BTreeMap<PathBuf, McpRecord> {
        if name == crate::MCP_SERVER_NAME {
            &mut self.mcp
        } else {
            self.servers.entry(name.to_string()).or_default()
        }
    }

    /// Forgets server `name`'s record in `file`.
    pub fn forget(&mut self, name: &str, file: &Path) {
        self.records_mut(name).remove(file);
        if name != crate::MCP_SERVER_NAME && self.servers.get(name).is_some_and(|r| r.is_empty()) {
            self.servers.remove(name);
        }
    }

    /// Every server name with a record in `file`, `cua` first.
    pub fn names_in(&self, file: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(r) = self.mcp.get(file) {
            out.push(r.name.clone());
        }
        for (name, recs) in &self.servers {
            if recs.contains_key(file) {
                out.push(name.clone());
            }
        }
        out
    }

    /// Saves atomically.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.version = STATE_VERSION;
        let mut s =
            serde_json::to_string_pretty(self).map_err(|e| Error::Internal(e.to_string()))?;
        s.push('\n');
        fsutil::atomic_write(path, s.as_bytes())
    }
}

/// Now, RFC 3339.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
