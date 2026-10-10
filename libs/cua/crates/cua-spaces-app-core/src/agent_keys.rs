// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Agents: the provider keys agents in your Spaces get.
//!
//! The cua daemon keeps the keys in the Keychain (`cua_spaces::agents::keys`)
//! and answers only what this section shows: the provider, the variable a
//! run gets the key as, its last four characters and when it was added.
//! A key is typed once into a secure field and never shown again.
//!
//! Anthropic and OpenAI always have a row ("Not set" until saved). Any
//! other key is saved under the variable its agent reads ([`name_problem`]
//! refuses names a shell or loader reads, as the daemon does).

use crate::model::SpaceOs;
use serde::{Deserialize, Serialize};

/// A provider with its own row.
struct Provider {
    id: &'static str,
    label: &'static str,
    env: &'static str,
    /// Who uses it, in plain words.
    used_by: &'static str,
    /// Where to get a key.
    lede: &'static str,
    placeholder: &'static str,
}

const PROVIDERS: &[Provider] = &[
    Provider {
        id: "anthropic",
        label: "Anthropic",
        env: "ANTHROPIC_API_KEY",
        used_by: "For Claude Code and other agents that use Claude.",
        lede: "Create a key at console.anthropic.com, then paste it here.",
        placeholder: "sk-ant-…",
    },
    Provider {
        id: "openai",
        label: "OpenAI",
        env: "OPENAI_API_KEY",
        used_by: "For Codex and other agents that use OpenAI models.",
        lede: "Create a key at platform.openai.com, then paste it here.",
        placeholder: "sk-…",
    },
];

/// A key the daemon reported (`agent_keys.list`'s `keys`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentKeyInput {
    /// `anthropic`, `openai` or `other`.
    pub provider: String,
    /// The variable a run gets it as.
    pub env: String,
    /// Its last four characters (empty for a short key).
    pub last4: String,
    /// When it was added (Unix ms).
    pub added_ms: u64,
}

/// What the section shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentKeysInput {
    /// The keys saved, once read.
    pub keys: Vec<AgentKeyInput>,
    /// Why this machine can't keep keys (the daemon's words), when it can't.
    pub unavailable: Option<String>,
    /// The keys could not be read (no daemon, a refused Keychain).
    pub error: Option<String>,
}

/// One row: a provider, or an Other key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentKeyRow {
    /// The variable (the row's id).
    pub env: String,
    /// `anthropic`, `openai` or `other`.
    pub provider: String,
    /// "Anthropic", "OpenAI", or the variable.
    pub title: String,
    /// Who gets it.
    pub detail: String,
    /// A key is saved.
    pub set: bool,
    /// "Not set", "•••• 0000", or "Saved" for a short key.
    pub status: String,
    /// When it was added (the page writes "Added <date>").
    pub added_ms: Option<u64>,
    /// "Add key" or "Replace".
    pub action_label: String,
    /// "Remove", when a key is saved.
    pub remove_label: Option<String>,
}

/// The section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentKeysView {
    pub title: String,
    /// Where the keys live and who gets them.
    pub intro: String,
    pub rows: Vec<AgentKeyRow>,
    /// The button for an Other key.
    pub add_other_label: String,
    /// What an Other key is.
    pub other_help: String,
    /// Why keys can't be saved or read, in place of the buttons.
    pub notice: Option<String>,
    /// The buttons work.
    pub can_edit: bool,
    /// "Added" (before the date the page formats).
    pub added_label: String,
}

/// What the add or replace sheet is for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentKeyFormInput {
    /// `anthropic`, `openai` or `other`.
    pub provider: String,
    /// The row it opened from (replacing an Other key), when any.
    pub env: Option<String>,
    /// The variable typed for a new Other key.
    pub name: String,
    /// Something was typed in the key field (the page never sends the key).
    pub has_value: bool,
}

/// The add or replace sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentKeyFormView {
    pub title: String,
    pub lede: String,
    /// The variable field, for a new Other key.
    pub name_label: Option<String>,
    pub name_placeholder: Option<String>,
    /// Why the typed variable can't be used.
    pub name_error: Option<String>,
    pub value_label: String,
    pub value_placeholder: String,
    /// Under the key field.
    pub value_help: String,
    pub save_label: String,
    pub cancel_label: String,
    pub can_save: bool,
    /// What `agent_keys.set` gets besides the key.
    pub provider: String,
    pub env: Option<String>,
}

/// Asked before a key is removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentKeyConfirm {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub cancel_label: String,
}

fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

fn status(k: &AgentKeyInput) -> String {
    if k.last4.is_empty() {
        "Saved".into()
    } else {
        format!("\u{2022}\u{2022}\u{2022}\u{2022} {}", k.last4)
    }
}

fn row(
    title: &str,
    env: &str,
    provider: &str,
    detail: String,
    key: Option<&AgentKeyInput>,
) -> AgentKeyRow {
    AgentKeyRow {
        env: env.into(),
        provider: provider.into(),
        title: title.into(),
        detail,
        set: key.is_some(),
        status: key.map(status).unwrap_or_else(|| "Not set".into()),
        added_ms: key.map(|k| k.added_ms),
        action_label: if key.is_some() { "Replace" } else { "Add key" }.into(),
        remove_label: key.map(|_| "Remove".into()),
    }
}

/// Where saved keys live, in the words of the system the app runs on. The
/// daemon keeps them in the OS credential vault (`cua_spaces::agents::keys`):
/// the Keychain on a Mac, Windows Credential Manager on Windows. Linux has no
/// vault cua uses (its store is a file, which never holds agent keys), so
/// those words name none; the daemon's own notice says why keys can't be
/// saved there.
fn vault(os: SpaceOs) -> Option<&'static str> {
    match os {
        SpaceOs::Macos => Some("the Keychain"),
        SpaceOs::Windows => Some("Windows Credential Manager"),
        SpaceOs::Linux | SpaceOs::Unknown => None,
    }
}

/// The section's first lines: why a key is needed and where it stays.
fn intro(os: SpaceOs) -> String {
    let this = os.this_machine_lower();
    let place = vault(os).map(|v| format!(" in {v}")).unwrap_or_default();
    format!(
        "Agents in your Spaces need a provider key to run. Keys stay on {this}{place}, and each \
         one is only given to the agents that use it."
    )
}

/// Under the key field of the sheet.
fn value_help(os: SpaceOs) -> String {
    let this = os.this_machine_lower();
    let place = vault(os).map(|v| format!("in {v} ")).unwrap_or_default();
    format!("It's saved {place}on {this} and won't be shown again.")
}

/// The section for `input` on a Mac: Anthropic, OpenAI, then each Other key by name.
pub fn view(input: &AgentKeysInput) -> AgentKeysView {
    view_on(input, SpaceOs::Macos)
}

/// [`view`] in a shell on `os`: where the keys stay is that system's
/// ([`vault`]). macOS gives the same view as [`view`].
pub fn view_on(input: &AgentKeysInput, os: SpaceOs) -> AgentKeysView {
    let find = |env: &str| input.keys.iter().find(|k| k.env == env);
    let mut rows: Vec<AgentKeyRow> = PROVIDERS
        .iter()
        .map(|p| row(p.label, p.env, p.id, p.used_by.into(), find(p.env)))
        .collect();
    let mut others: Vec<&AgentKeyInput> = input
        .keys
        .iter()
        .filter(|k| provider_of(&k.env) == "other")
        .collect();
    others.sort_by(|a, b| a.env.cmp(&b.env));
    rows.extend(others.into_iter().map(|k| {
        row(
            &k.env,
            &k.env,
            "other",
            format!("For agents that read {}.", k.env),
            Some(k),
        )
    }));
    let notice = input
        .error
        .as_ref()
        .map(|e| format!("Couldn't read your agent keys: {e}"))
        .or_else(|| {
            input
                .unavailable
                .as_ref()
                .map(|why| format!("Keys can't be saved here: {why}."))
        });
    AgentKeysView {
        title: "Agent keys".into(),
        intro: intro(os),
        rows,
        add_other_label: "Add another key".into(),
        other_help: "For another provider, save its key under the variable its agent reads, \
                     like GEMINI_API_KEY."
            .into(),
        can_edit: notice.is_none(),
        notice,
        added_label: "Added".into(),
    }
}

/// The provider a variable belongs to: its row's, else `other`.
pub fn provider_of(env: &str) -> &'static str {
    PROVIDERS
        .iter()
        .find(|p| p.env == env)
        .map(|p| p.id)
        .unwrap_or("other")
}

/// Names that would change how a run's shell, loader or tools behave.
const REFUSED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PWD",
    "OLDPWD",
    "TMPDIR",
    "TMP",
    "TEMP",
    "IFS",
    "ENV",
    "BASH_ENV",
    "CDPATH",
    "TERM",
    "LANG",
    "LANGUAGE",
    "DISPLAY",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "PS1",
    "PS2",
    "PS4",
    "PROMPT_COMMAND",
    "SSH_AUTH_SOCK",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PYTHONPATH",
    "PYTHONHOME",
    "PYTHONSTARTUP",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "RUBYLIB",
    "JAVA_TOOL_OPTIONS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];
const REFUSED_PREFIXES: &[&str] = &[
    "DYLD_",
    "LD_",
    "CUA_",
    "BASH_FUNC_",
    "LC_",
    "XDG_",
    "GIT_",
    "NPM_CONFIG_",
    "MALLOC",
];

/// Why `name` can't hold an Other key (`None`: it can). The daemon refuses
/// the same names (`cua_spaces::agents::keys::validate_name`).
pub fn name_problem(name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() {
        return Some("Enter the variable name, like MISTRAL_API_KEY.".into());
    }
    let ident = n.len() <= 128
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !n.as_bytes()[0].is_ascii_digit();
    if !ident {
        return Some("Use letters, digits and _, and don't start with a digit.".into());
    }
    let upper = n.to_ascii_uppercase();
    if REFUSED.contains(&upper.as_str()) || REFUSED_PREFIXES.iter().any(|p| upper.starts_with(p)) {
        return Some(format!(
            "{n} changes how programs run, so it can't hold a key."
        ));
    }
    None
}

/// The add or replace sheet for `form` on a Mac.
pub fn form(input: &AgentKeysInput, form: &AgentKeyFormInput) -> AgentKeyFormView {
    form_on(input, form, SpaceOs::Macos)
}

/// [`form`] in a shell on `os`: where the key is saved is that system's
/// ([`vault`]). macOS gives the same sheet as [`form`].
pub fn form_on(input: &AgentKeysInput, form: &AgentKeyFormInput, os: SpaceOs) -> AgentKeyFormView {
    let saved = |env: &str| input.keys.iter().any(|k| k.env == env);
    let base = |title: String, lede: &str, placeholder: &str| AgentKeyFormView {
        title,
        lede: lede.into(),
        name_label: None,
        name_placeholder: None,
        name_error: None,
        value_label: "API key".into(),
        value_placeholder: placeholder.into(),
        value_help: value_help(os),
        save_label: "Save".into(),
        cancel_label: "Cancel".into(),
        can_save: form.has_value,
        provider: form.provider.clone(),
        env: None,
    };
    if let Some(p) = provider(&form.provider) {
        let verb = if saved(p.env) {
            "Replace the"
        } else {
            "Add an"
        };
        let mut v = base(format!("{verb} {} key", p.label), p.lede, p.placeholder);
        v.env = Some(p.env.into());
        return v;
    }
    // An Other key: replacing one keeps its name; a new one asks for it.
    if let Some(env) = form.env.as_deref().filter(|e| !e.is_empty()) {
        let mut v = base(
            format!("Replace {env}"),
            "Paste the new key. Agents that read this variable get it from their next run.",
            "Key",
        );
        v.env = Some(env.into());
        return v;
    }
    let typed = form.name.trim();
    let problem = name_problem(typed);
    let mut v = base(
        "Add another key".into(),
        "Enter the variable the agent reads for its key, then paste the key.",
        "Key",
    );
    v.name_label = Some("Variable name".into());
    v.name_placeholder = Some("GEMINI_API_KEY".into());
    // Nothing typed yet is not an error, but can't be saved.
    v.name_error = if typed.is_empty() {
        None
    } else {
        problem.clone()
    };
    v.can_save = form.has_value && problem.is_none();
    v.env = (!typed.is_empty() && problem.is_none()).then(|| typed.to_string());
    if v.env.as_deref().is_some_and(saved) {
        v.title = format!("Replace {typed}");
    }
    v
}

/// The question before removing the key `env` (`None`: none is saved).
pub fn remove_confirm(input: &AgentKeysInput, env: &str) -> Option<AgentKeyConfirm> {
    input.keys.iter().find(|k| k.env == env)?;
    let title = match provider(provider_of(env)) {
        Some(p) => format!("Remove the {} key?", p.label),
        None => format!("Remove {env}?"),
    };
    Some(AgentKeyConfirm {
        title,
        message: format!(
            "Agents that need {env} won't start until you add a key again. Runs already going keep working."
        ),
        confirm_label: "Remove".into(),
        cancel_label: "Cancel".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(env: &str, last4: &str) -> AgentKeyInput {
        AgentKeyInput {
            provider: provider_of(env).into(),
            env: env.into(),
            last4: last4.into(),
            added_ms: 1,
        }
    }

    #[test]
    fn providers_always_have_a_row_and_others_follow() {
        let v = view(&AgentKeysInput::default());
        assert_eq!(v.rows.len(), 2);
        assert!(
            v.rows
                .iter()
                .all(|r| !r.set && r.status == "Not set" && r.action_label == "Add key")
        );
        let v = view(&AgentKeysInput {
            keys: vec![
                key("ZED_KEY", ""),
                key("ANTHROPIC_API_KEY", "0000"),
                key("MISTRAL_API_KEY", "abcd"),
            ],
            ..Default::default()
        });
        let titles: Vec<&str> = v.rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Anthropic", "OpenAI", "MISTRAL_API_KEY", "ZED_KEY"]
        );
        assert_eq!(v.rows[0].status, "\u{2022}\u{2022}\u{2022}\u{2022} 0000");
        assert_eq!(v.rows[0].remove_label.as_deref(), Some("Remove"));
        assert_eq!(v.rows[3].status, "Saved");
        assert!(v.can_edit && v.notice.is_none());
        let v = view(&AgentKeysInput {
            unavailable: Some("no Keychain".into()),
            ..Default::default()
        });
        assert!(!v.can_edit && v.notice.unwrap().contains("no Keychain"));
    }

    /// Where the keys stay is the shell's system's: the Mac's words are the
    /// ones every build had before, to the byte.
    #[test]
    fn the_words_for_where_keys_stay_are_the_systems() {
        let sheet = |os| {
            form_on(
                &AgentKeysInput::default(),
                &AgentKeyFormInput {
                    provider: "anthropic".into(),
                    ..Default::default()
                },
                os,
            )
        };
        let mac = (
            "Agents in your Spaces need a provider key to run. Keys stay on this Mac in the Keychain, and each one is only given to the agents that use it.",
            "It's saved in the Keychain on this Mac and won't be shown again.",
        );
        assert_eq!(view(&AgentKeysInput::default()).intro, mac.0);
        assert_eq!(
            form(
                &AgentKeysInput::default(),
                &AgentKeyFormInput {
                    provider: "anthropic".into(),
                    ..Default::default()
                }
            )
            .value_help,
            mac.1
        );
        assert_eq!(
            view_on(&AgentKeysInput::default(), SpaceOs::Macos).intro,
            mac.0
        );
        assert_eq!(sheet(SpaceOs::Macos).value_help, mac.1);

        // Windows: its credential vault.
        assert_eq!(
            view_on(&AgentKeysInput::default(), SpaceOs::Windows).intro,
            "Agents in your Spaces need a provider key to run. Keys stay on this PC in Windows Credential Manager, and each one is only given to the agents that use it."
        );
        assert_eq!(
            sheet(SpaceOs::Windows).value_help,
            "It's saved in Windows Credential Manager on this PC and won't be shown again."
        );

        // Linux: cua has no vault there (the file store never holds agent keys), so no store is named.
        assert_eq!(
            view_on(&AgentKeysInput::default(), SpaceOs::Linux).intro,
            "Agents in your Spaces need a provider key to run. Keys stay on this computer, and each one is only given to the agents that use it."
        );
        assert_eq!(
            sheet(SpaceOs::Linux).value_help,
            "It's saved on this computer and won't be shown again."
        );

        // Nothing of another system's words is left, in any sheet or section.
        let keys = AgentKeysInput {
            keys: vec![key("ANTHROPIC_API_KEY", "0000"), key("MISTRAL_API_KEY", "")],
            ..Default::default()
        };
        for (os, banned) in [
            (SpaceOs::Windows, ["Keychain", "Mac"]),
            (SpaceOs::Linux, ["Keychain", "Mac"]),
        ] {
            let v = view_on(&keys, os);
            let forms = ["anthropic", "openai", "other"].map(|p| {
                form_on(
                    &keys,
                    &AgentKeyFormInput {
                        provider: p.into(),
                        has_value: true,
                        ..Default::default()
                    },
                    os,
                )
            });
            let mut words = vec![
                v.title,
                v.intro,
                v.add_other_label,
                v.other_help,
                v.added_label,
            ];
            words.extend(
                v.rows
                    .into_iter()
                    .flat_map(|r| [r.title, r.detail, r.status, r.action_label]),
            );
            for f in forms {
                words.extend([f.title, f.lede, f.value_label, f.value_help, f.save_label]);
            }
            for w in words {
                for b in banned {
                    assert!(!w.contains(b), "{os:?}: {w:?} mentions {b}");
                }
            }
        }
        // The rest of the section is the same on every system.
        let mac = view(&keys);
        for os in [SpaceOs::Windows, SpaceOs::Linux] {
            let v = view_on(&keys, os);
            assert_eq!(
                (&v.rows, &v.title, &v.notice, v.can_edit),
                (&mac.rows, &mac.title, &mac.notice, mac.can_edit)
            );
        }
    }

    #[test]
    fn the_sheet_checks_other_names() {
        let input = AgentKeysInput {
            keys: vec![key("ANTHROPIC_API_KEY", "0000")],
            ..Default::default()
        };
        let f = form(
            &input,
            &AgentKeyFormInput {
                provider: "anthropic".into(),
                ..Default::default()
            },
        );
        assert_eq!(f.title, "Replace the Anthropic key");
        assert!(!f.can_save);
        let f = form(
            &input,
            &AgentKeyFormInput {
                provider: "openai".into(),
                has_value: true,
                ..Default::default()
            },
        );
        assert_eq!((f.title.as_str(), f.can_save), ("Add an OpenAI key", true));
        let other = |name: &str| {
            form(
                &input,
                &AgentKeyFormInput {
                    provider: "other".into(),
                    name: name.into(),
                    has_value: true,
                    ..Default::default()
                },
            )
        };
        assert!(other("").name_error.is_none() && !other("").can_save);
        assert!(other("LD_PRELOAD").name_error.is_some() && !other("LD_PRELOAD").can_save);
        assert!(other("path").name_error.is_some());
        assert!(other("1KEY").name_error.is_some());
        let ok = other("GEMINI_API_KEY");
        assert!(ok.can_save && ok.env.as_deref() == Some("GEMINI_API_KEY"));
        assert!(remove_confirm(&input, "OPENAI_API_KEY").is_none());
        assert_eq!(
            remove_confirm(&input, "ANTHROPIC_API_KEY").unwrap().title,
            "Remove the Anthropic key?"
        );
    }
}
