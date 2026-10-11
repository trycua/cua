//! Session teleport: pushing a logged-in host app session into a Space.
//!
//! Two facts shape everything here, and they are facts about real data.
//!
//! **A manifest is a consent surface, not a file list.** The Claude Code
//! manifest comes back with three items totalling 935,980,082 bytes, and the
//! largest of them — 14 projects of conversation transcripts — is marked
//! sensitive and deliberately *not* checked by default. A caller that reads
//! `items` as a list of paths to send ships ~900 MB of transcripts onto a
//! machine an autonomous agent is driving, one array element away from the
//! login-only teleport it meant to perform.
//!
//! **Consent is a type, not a defaulted parameter.** [`Approval`] has no
//! public constructor and no public fields; the only ways to make one are
//! [`TeleportManifest::approving`] and
//! [`TeleportManifest::approving_server_default`], both of which refuse a path
//! the manifest does not offer and refuse a sensitive path that was not
//! acknowledged by name. Across the FFI it is an opaque object rather than a
//! record, so a TypeScript caller cannot forge one out of an object literal
//! either — which is a stronger gate than the Swift original had.
//!
//! ## The `nil`-selection rule
//!
//! `selection: None` means **the server's default set**, never "everything".
//! It is carried to the wire as the *absence* of `include`, so the server
//! chooses, and its default leaves the expensive items out. To stop that
//! meaning drifting, an empty-but-present selection is refused rather than
//! silently treated as either one.

use serde_json::{Map, Value};

use crate::client::error::{Result, SpacesError};

/// How much of an app's session to consider. These are the two values
/// `teleport_manifest` and `teleport_app` actually take on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeleportScope {
    /// The whole profile: credentials, cookies, preferences, local state.
    Full,
    /// Open tabs only, and no credentials.
    Tabs,
}

impl TeleportScope {
    pub fn as_str(self) -> &'static str {
        match self {
            TeleportScope::Full => "full",
            TeleportScope::Tabs => "tabs",
        }
    }
}

/// One transferable item in an app's manifest.
///
/// `is_sensitive` and `is_checked_by_default` are the two fields that make a
/// manifest a consent surface rather than a file list: the logged-in session
/// is sensitive and checked by default, while conversation transcripts are
/// sensitive and *unchecked*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeleportItem {
    /// A `rel_path` from the manifest — the exact string `include` takes.
    pub relative_path: String,
    pub label: String,
    pub estimated_bytes: u64,
    /// Credentials, cookies, tokens, transcripts.
    pub is_sensitive: bool,
    /// Whether the server's own default selection includes this. The server
    /// leaves the expensive items unchecked on purpose.
    pub is_checked_by_default: bool,
    pub count: Option<u32>,
    pub count_noun: Option<String>,
    pub explanation: String,
}

fn string(row: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| row.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn boolean(row: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| row.get(*key).and_then(Value::as_bool))
}

fn integer(row: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| row.get(*key).and_then(Value::as_u64))
}

impl TeleportItem {
    /// Read a row without inventing fields. Both key spellings the backends
    /// use are accepted, and an unreadable row yields an empty
    /// `relative_path` the caller filters out rather than a guess.
    pub fn from_row(row: &Map<String, Value>) -> Self {
        TeleportItem {
            relative_path: string(row, &["rel_path", "path"]).unwrap_or_default(),
            label: string(row, &["label"]).unwrap_or_default(),
            estimated_bytes: integer(row, &["est_bytes", "bytes", "size"]).unwrap_or(0),
            is_sensitive: boolean(row, &["sensitive", "is_sensitive"]).unwrap_or(false),
            is_checked_by_default: boolean(row, &["default_checked", "default", "is_default"])
                .unwrap_or(false),
            count: integer(row, &["count"]).map(|count| count as u32),
            count_noun: string(row, &["count_noun"]),
            explanation: string(row, &["note", "why"]).unwrap_or_default(),
        }
    }
}

/// Exactly what would leave this machine, before anything does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeleportManifest {
    /// The id the backend takes, e.g. `chrome`, `claude-code`.
    pub app: String,
    pub display_name: String,
    pub scope: TeleportScope,
    /// The server's own scope string, verbatim — `full_profile` rather than
    /// `full`, on the backend that says so.
    pub server_scope: String,
    pub items: Vec<TeleportItem>,
    /// The server's own total. It is not the sum of `items` on every backend,
    /// so it is carried rather than recomputed.
    pub total_estimated_bytes: u64,
    /// The server's own warnings, verbatim. They say things a caller cannot
    /// derive — that the logged-in session carries an OAuth token, for one.
    pub notes: Vec<String>,
}

impl TeleportManifest {
    /// Read whatever shape the manifest came back in, without inventing items.
    /// The payload arrives as an object on some backends and as a JSON string
    /// on others (`FRICTION.md` §4); both land here.
    pub fn decode(app: &str, scope: TeleportScope, payload: &Value) -> Self {
        let object = match payload.as_object() {
            Some(object) => object.clone(),
            None => payload
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default(),
        };
        let rows: Vec<Map<String, Value>> = match payload.as_array() {
            Some(array) => array.iter().filter_map(Value::as_object).cloned().collect(),
            None => object
                .get("items")
                .or_else(|| object.get("entries"))
                .and_then(Value::as_array)
                .map(|rows| rows.iter().filter_map(Value::as_object).cloned().collect())
                .unwrap_or_default(),
        };
        let items: Vec<TeleportItem> = rows
            .iter()
            .map(TeleportItem::from_row)
            .filter(|item| !item.relative_path.is_empty())
            .collect();
        let app_id = string(&object, &["provider_id"]).unwrap_or_else(|| app.to_string());
        let display_name = string(&object, &["app_display_name"]).unwrap_or_else(|| app_id.clone());
        TeleportManifest {
            total_estimated_bytes: integer(&object, &["total_est_bytes"])
                .unwrap_or_else(|| items.iter().map(|item| item.estimated_bytes).sum()),
            server_scope: string(&object, &["scope"]).unwrap_or_else(|| scope.as_str().to_string()),
            notes: object
                .get("notes")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            app: app_id,
            display_name,
            scope,
            items,
        }
    }

    pub fn sensitive_items(&self) -> Vec<TeleportItem> {
        self.items
            .iter()
            .filter(|item| item.is_sensitive)
            .cloned()
            .collect()
    }

    /// The items the server marks as checked by default — what a teleport
    /// sends when the caller does not choose. **Not everything.**
    pub fn default_selection(&self) -> Vec<TeleportItem> {
        self.items
            .iter()
            .filter(|item| item.is_checked_by_default)
            .cloned()
            .collect()
    }

    /// Just the logged-in session: the smallest teleport that still leaves an
    /// in-Space agent authenticated, and the one that cannot accidentally
    /// carry the ~900 MB transcript item.
    pub fn login_only_selection(&self) -> Vec<TeleportItem> {
        let checked = self.default_selection();
        let sensitive: Vec<TeleportItem> = checked
            .iter()
            .filter(|item| item.is_sensitive)
            .cloned()
            .collect();
        if sensitive.is_empty() {
            checked
        } else {
            sensitive
        }
    }

    /// Mint an approval for an explicit selection of paths.
    ///
    /// Refuses a path this manifest does not offer, and refuses any sensitive
    /// path unless `acknowledging_sensitive_items` is `true`. There is no
    /// overload where the acknowledgement defaults.
    pub fn approving(
        &self,
        space_id: &str,
        relative_paths: &[String],
        acknowledging_sensitive_items: bool,
    ) -> Result<Approval> {
        if relative_paths.is_empty() {
            return Err(SpacesError::TeleportRefused(
                "an empty selection is ambiguous: pass no selection at all to use the server's \
                 default set, which is never everything"
                    .into(),
            ));
        }
        self.mint(
            space_id,
            relative_paths,
            acknowledging_sensitive_items,
            Some(relative_paths.to_vec()),
        )
    }

    /// Approve the **server's** default set without naming it.
    ///
    /// The resulting approval carries no `include` on the wire, which is what
    /// makes the server choose. The consent gate still applies, because the
    /// default set contains the logged-in session and that is sensitive.
    pub fn approving_server_default(
        &self,
        space_id: &str,
        acknowledging_sensitive_items: bool,
    ) -> Result<Approval> {
        let paths: Vec<String> = self
            .default_selection()
            .into_iter()
            .map(|item| item.relative_path)
            .collect();
        self.mint(space_id, &paths, acknowledging_sensitive_items, None)
    }

    fn mint(
        &self,
        space_id: &str,
        relative_paths: &[String],
        acknowledging_sensitive_items: bool,
        includes: Option<Vec<String>>,
    ) -> Result<Approval> {
        let unknown: Vec<&String> = relative_paths
            .iter()
            .filter(|path| !self.items.iter().any(|item| &&item.relative_path == path))
            .collect();
        if !unknown.is_empty() {
            return Err(SpacesError::TeleportRefused(format!(
                "approved entries that are not in this manifest: {}",
                unknown
                    .iter()
                    .map(|path| path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let selected: Vec<&TeleportItem> = relative_paths
            .iter()
            .filter_map(|path| self.items.iter().find(|item| &item.relative_path == path))
            .collect();
        let sensitive: Vec<&&TeleportItem> =
            selected.iter().filter(|item| item.is_sensitive).collect();
        if !sensitive.is_empty() && !acknowledging_sensitive_items {
            return Err(SpacesError::TeleportRefused(format!(
                "{} approved entries are sensitive ({}); acknowledge them explicitly to send them",
                sensitive.len(),
                sensitive
                    .iter()
                    .map(|item| item.relative_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(Approval {
            app: self.app.clone(),
            scope: self.scope,
            space: space_id.to_string(),
            includes,
            approved_bytes: selected.iter().map(|item| item.estimated_bytes).sum(),
        })
    }
}

/// Proof a human agreed, and to what.
///
/// Only [`TeleportManifest::approving`] and
/// [`TeleportManifest::approving_server_default`] mint one, and only the
/// teleport send accepts one. The fields are private on purpose: an approval
/// that can be edited after it is minted is not an approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    app: String,
    scope: TeleportScope,
    space: String,
    /// Per-path `include`, mapping straight onto `teleport_app`'s `include`.
    /// `None` means send no `include` at all and let the **server** pick its
    /// default set — which is never "everything".
    includes: Option<Vec<String>>,
    approved_bytes: u64,
}

impl Approval {
    pub fn app(&self) -> &str {
        &self.app
    }
    pub fn scope(&self) -> TeleportScope {
        self.scope
    }
    pub fn space(&self) -> &str {
        &self.space
    }
    pub fn approved_bytes(&self) -> u64 {
        self.approved_bytes
    }
    /// The paths named explicitly. Empty when the approval deferred to the
    /// server's own default set — which is what `uses_server_default` reports,
    /// so the two are never confused.
    pub fn approved_paths(&self) -> Vec<String> {
        self.includes.clone().unwrap_or_default()
    }
    pub fn uses_server_default(&self) -> bool {
        self.includes.is_none()
    }
    pub(crate) fn includes(&self) -> Option<&Vec<String>> {
        self.includes.as_ref()
    }
}

/// What actually moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeleportReceipt {
    pub app: String,
    pub space: String,
    /// `teleport` or `file` on Local; `teleport` on Fleet.
    pub method: String,
    /// The `rel_path`s that were asked for. Empty when the approval deferred
    /// to the server's own default set.
    pub transferred_paths: Vec<String>,
    /// The backend's own last line, kept rather than parsed into a claim.
    pub raw_result: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The real Claude Code manifest, to the byte counts the server publishes.
    fn claude_code_manifest() -> TeleportManifest {
        TeleportManifest::decode(
            "claude-code",
            TeleportScope::Full,
            &json!({
                "provider_id": "claude-code",
                "app_display_name": "Claude Code",
                "scope": "full_profile",
                "total_est_bytes": 935_980_082u64,
                "items": [
                    { "rel_path": "claude/.credentials.json", "label": "Logged-in session",
                      "est_bytes": 1_024, "sensitive": true, "default_checked": true },
                    { "rel_path": "claude/settings.json", "label": "Settings",
                      "est_bytes": 4_096, "sensitive": false, "default_checked": true },
                    { "rel_path": "claude/projects/", "label": "Conversation transcripts",
                      "est_bytes": 935_974_962u64, "sensitive": true, "default_checked": false,
                      "count": 14, "count_noun": "projects" }
                ],
                "notes": ["The logged-in session carries an OAuth token."]
            }),
        )
    }

    #[test]
    fn the_manifest_is_read_without_inventing_items() {
        let manifest = claude_code_manifest();
        assert_eq!(manifest.items.len(), 3);
        assert_eq!(manifest.server_scope, "full_profile");
        assert_eq!(manifest.total_estimated_bytes, 935_980_082);
        assert_eq!(manifest.notes.len(), 1);
    }

    /// The ~900 MB item is sensitive and *not* checked by default. A caller
    /// reading `items` as "what to send" is one array element from shipping it.
    #[test]
    fn the_default_selection_leaves_the_expensive_item_out() {
        let manifest = claude_code_manifest();
        let default: Vec<String> = manifest
            .default_selection()
            .into_iter()
            .map(|item| item.relative_path)
            .collect();
        assert_eq!(
            default,
            vec!["claude/.credentials.json", "claude/settings.json"]
        );
        assert!(!default.iter().any(|path| path == "claude/projects/"));

        let login_only: Vec<String> = manifest
            .login_only_selection()
            .into_iter()
            .map(|item| item.relative_path)
            .collect();
        assert_eq!(login_only, vec!["claude/.credentials.json"]);
    }

    #[test]
    fn an_approval_cannot_name_a_path_the_manifest_did_not_offer() {
        let manifest = claude_code_manifest();
        let error = manifest
            .approving("local:s", &["claude/../../etc/passwd".into()], true)
            .unwrap_err();
        assert_eq!(error.tag(), "TeleportRefused");
        assert!(error.to_string().contains("not in this manifest"));
    }

    #[test]
    fn a_sensitive_entry_requires_an_explicit_acknowledgement() {
        let manifest = claude_code_manifest();
        assert!(
            manifest
                .approving("local:s", &["claude/.credentials.json".into()], false)
                .is_err()
        );
        assert!(
            manifest
                .approving("local:s", &["claude/.credentials.json".into()], true)
                .is_ok()
        );
        // A non-sensitive selection needs no acknowledgement.
        assert!(
            manifest
                .approving("local:s", &["claude/settings.json".into()], false)
                .is_ok()
        );
    }

    /// The `nil`-selection rule, in three parts.
    #[test]
    fn no_selection_means_the_servers_default_and_an_empty_one_is_refused() {
        let manifest = claude_code_manifest();

        // 1. No selection carries no `include`, so the *server* chooses.
        let deferred = manifest.approving_server_default("local:s", true).unwrap();
        assert!(deferred.uses_server_default());
        assert!(deferred.includes().is_none());
        assert!(deferred.approved_paths().is_empty());
        // and it is the default set that was consented to, not everything.
        assert_eq!(deferred.approved_bytes(), 1_024 + 4_096);

        // 2. An explicit selection carries exactly those paths.
        let explicit = manifest
            .approving("local:s", &["claude/.credentials.json".into()], true)
            .unwrap();
        assert!(!explicit.uses_server_default());
        assert_eq!(explicit.approved_paths(), vec!["claude/.credentials.json"]);

        // 3. An empty-but-present selection is refused rather than silently
        //    meaning either one.
        assert_eq!(
            manifest.approving("local:s", &[], true).unwrap_err().tag(),
            "TeleportRefused"
        );
    }

    #[test]
    fn approving_everything_is_spelled_out_and_costs_what_it_says() {
        let manifest = claude_code_manifest();
        let paths: Vec<String> = manifest
            .items
            .iter()
            .map(|item| item.relative_path.clone())
            .collect();
        let approval = manifest.approving("local:s", &paths, true).unwrap();
        assert_eq!(approval.approved_bytes(), 935_980_082);
    }
}
