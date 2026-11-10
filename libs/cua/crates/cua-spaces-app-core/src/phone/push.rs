// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Push notifications on a phone, the client half: where a tap goes, what to
//! tell the relay about this phone, and what Settings says.
//!
//! The relay sends the push (the server half is the relay's); this module
//! reads only its structured `data`, never the title or the body, so what a
//! tap opens cannot be steered by words an agent wrote.
//!
//! * [`route`]: a notification's `data` becomes the screen it opens: the
//!   approval it is about, else the thread (scrolled to the entry), else Home.
//!   Anything malformed opens Home, never a guess.
//! * [`registration`]: whether to tell the relay about this phone's push
//!   token, and exactly what to send (a `PUT` or a `DELETE`), so a token is
//!   registered once and withdrawn when notifications go off or the person
//!   signs out.
//! * [`settings_view`]: the Notifications group in Settings: two switches and
//!   one honest line about where things stand.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The payload version this build reads.
pub const PAYLOAD_VERSION: u64 = 1;

/// Where a tap goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// `approval`, `thread` or `home`.
    pub target: String,
    pub space_id: Option<String>,
    /// The thread entry to scroll to.
    pub entry: Option<u64>,
    /// The approval entry, when the push is about one.
    pub approval: Option<u64>,
    /// The in-app path (`/space/<id>?entry=7`, or `/`).
    pub link: String,
}

fn home() -> Route {
    Route {
        target: "home".into(),
        space_id: None,
        entry: None,
        approval: None,
        link: "/".into(),
    }
}

/// `encodeURIComponent` for the bytes of `s`.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn valid_space(s: &str) -> bool {
    (1..=200).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
}

fn number(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// The screen a notification's `data` opens. Reads the relay's push shape
/// (`v`, `kind`, `space`, `entry`, `approval`) and the app's own local one
/// (`spaceId`, `seq`, `kind`).
pub fn route(data: &Value) -> Route {
    let version_ok = data
        .get("v")
        .is_none_or(|v| number(v) == Some(PAYLOAD_VERSION));
    if !version_ok {
        return home();
    }
    let space = data
        .get("space")
        .or_else(|| data.get("spaceId"))
        .and_then(Value::as_str)
        .filter(|s| valid_space(s));
    let Some(space) = space else {
        return home();
    };
    let entry = data
        .get("entry")
        .or_else(|| data.get("seq"))
        .and_then(number);
    let kind = data.get("kind").and_then(Value::as_str).unwrap_or("");
    let approval = if matches!(kind, "approval" | "question") {
        data.get("approval").and_then(number).or(entry)
    } else {
        None
    };
    let focus = approval.or(entry);
    let id = encode(space);
    Route {
        target: if approval.is_some() {
            "approval"
        } else {
            "thread"
        }
        .into(),
        space_id: Some(space.to_string()),
        entry: focus,
        approval,
        link: match focus {
            Some(n) => format!("/space/{id}?entry={n}"),
            None => format!("/space/{id}"),
        },
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RegistrationInput {
    /// The system permission: `granted`, `denied`, `undetermined`.
    pub permission: String,
    /// The APNs device token, hex.
    pub token: Option<String>,
    /// This phone's relay device id (`dev_...`).
    pub device_id: String,
    /// `sandbox` (a development build) or `production`.
    pub env: String,
    pub app_id: String,
    pub locale: String,
    pub needs_you: bool,
    pub done: bool,
    /// The fingerprint of the registration the relay already has, if any.
    pub registered: Option<String>,
}

impl std::fmt::Debug for RegistrationInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationInput")
            .field("permission", &self.permission)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("device_id", &self.device_id)
            .field("env", &self.env)
            .field("app_id", &self.app_id)
            .field("locale", &self.locale)
            .field("needs_you", &self.needs_you)
            .field("done", &self.done)
            .field("registered", &self.registered)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// `register`, `unregister` or `none`.
    pub action: String,
    pub method: Option<String>,
    pub path: Option<String>,
    pub body: Option<Value>,
    /// Stored after a successful call; compared next time.
    pub fingerprint: Option<String>,
    pub reason: Option<String>,
}

fn none(reason: &str) -> Registration {
    Registration {
        action: "none".into(),
        method: None,
        path: None,
        body: None,
        fingerprint: None,
        reason: Some(reason.into()),
    }
}

/// What to do about the relay's registration for this phone.
pub fn registration(i: &RegistrationInput) -> Registration {
    let path = format!("/v1/push/registrations/{}", encode(&i.device_id));
    let token = i.token.as_deref().map(str::trim).unwrap_or("");
    let withdraw = |reason: &str| {
        if i.registered.is_some() && !i.device_id.is_empty() {
            Registration {
                action: "unregister".into(),
                method: Some("DELETE".into()),
                path: Some(path.clone()),
                body: None,
                fingerprint: None,
                reason: Some(reason.into()),
            }
        } else {
            none(reason)
        }
    };
    if i.device_id.is_empty() {
        return none("this phone is not enrolled yet");
    }
    if i.permission != "granted" {
        return withdraw("notifications are not allowed");
    }
    if !i.needs_you && !i.done {
        return withdraw("every alert is off");
    }
    if token.len() < 32 || token.len() > 400 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return none("no usable push token yet");
    }
    let mut categories = vec![];
    if i.needs_you {
        categories.push("needs_you");
    }
    if i.done {
        categories.push("done");
    }
    let env = if i.env == "production" {
        "production"
    } else {
        "sandbox"
    };
    let app_id = if i.app_id.is_empty() {
        "ai.cua.spaces.mobile"
    } else {
        i.app_id.as_str()
    };
    let locale = if i.locale.is_empty() {
        "en"
    } else {
        i.locale.as_str()
    };
    let fingerprint = format!("{token}|{env}|{app_id}|{}|{locale}", categories.join(","));
    if i.registered.as_deref() == Some(fingerprint.as_str()) {
        return none("already registered");
    }
    Registration {
        action: "register".into(),
        method: Some("PUT".into()),
        path: Some(path),
        body: Some(json!({
            "platform": "ios",
            "provider": "apns",
            "token": token.to_ascii_lowercase(),
            "app_id": app_id,
            "env": env,
            "categories": categories,
            "locale": locale,
        })),
        fingerprint: Some(fingerprint),
        reason: None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsInput {
    pub permission: String,
    pub needs_you: bool,
    pub done: bool,
    /// The relay has this phone's token.
    pub registered: bool,
    /// The account's relay can send push at all.
    pub push_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRow {
    /// `needsYou` or `done`.
    pub id: String,
    pub label: String,
    pub detail: String,
    pub on: bool,
    pub accessibility_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub text: String,
    /// `ok`, `warn`, `muted`.
    pub tone: String,
    /// `allow`, `open-settings`, or none.
    pub action: Option<String>,
    pub action_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub title: String,
    pub rows: Vec<SwitchRow>,
    pub status: Status,
    pub footer: String,
}

pub fn settings_view(i: &SettingsInput) -> SettingsView {
    let status = match (i.permission.as_str(), i.registered, i.push_available) {
        ("denied", _, _) => Status {
            text: "Notifications are off for Cua Spaces in iOS Settings.".into(),
            tone: "warn".into(),
            action: Some("open-settings".into()),
            action_label: Some("Open Settings".into()),
        },
        ("granted", true, _) => Status {
            text: "Push is on. Your agents can reach you when Cua Spaces is closed.".into(),
            tone: "ok".into(),
            action: None,
            action_label: None,
        },
        ("granted", false, true) => Status {
            text: "Setting up push for this phone\u{2026}".into(),
            tone: "muted".into(),
            action: None,
            action_label: None,
        },
        ("granted", false, false) => Status {
            text: "Alerts appear while Cua Spaces is open or was just open. Push to a closed app is not set up for this account yet.".into(),
            tone: "muted".into(),
            action: None,
            action_label: None,
        },
        _ => Status {
            text: "Allow notifications so your agents can reach you.".into(),
            tone: "muted".into(),
            action: Some("allow".into()),
            action_label: Some("Allow notifications".into()),
        },
    };
    SettingsView {
        title: "Notifications".into(),
        rows: vec![
            SwitchRow {
                id: "needsYou".into(),
                label: "Needs you".into(),
                detail: "When an agent asks for an approval or an answer.".into(),
                on: i.needs_you,
                accessibility_label: "Notify when an agent needs you".into(),
            },
            SwitchRow {
                id: "done".into(),
                label: "Done".into(),
                detail: "When an agent finishes.".into(),
                on: i.done,
                accessibility_label: "Notify when an agent is done".into(),
            },
        ],
        status,
        footer: "Allow and Decline on an alert ask you to unlock your phone first.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_about_an_approval_opens_the_approval() {
        let r = route(
            &json!({"v": 1, "kind": "approval", "space": "relay:abc", "entry": 12, "approval": 12}),
        );
        assert_eq!(
            (r.target.as_str(), r.entry, r.approval),
            ("approval", Some(12), Some(12))
        );
        assert_eq!(r.space_id.as_deref(), Some("relay:abc"));
        assert_eq!(r.link, "/space/relay%3Aabc?entry=12");
        // The local notification shape routes the same way.
        let local = route(&json!({"spaceId": "relay:abc", "seq": 12, "kind": "approval"}));
        assert_eq!(local, r);
    }

    #[test]
    fn a_done_push_opens_the_thread_at_the_entry() {
        let r = route(&json!({"v": 1, "kind": "done", "space": "relay:abc", "entry": "30"}));
        assert_eq!(r.target, "thread");
        assert_eq!((r.entry, r.approval), (Some(30), None));
        assert_eq!(r.link, "/space/relay%3Aabc?entry=30");
        let bare = route(&json!({"v": 1, "kind": "message", "space": "relay:abc"}));
        assert_eq!(bare.link, "/space/relay%3Aabc");
    }

    #[test]
    fn anything_odd_opens_home() {
        for d in [
            json!({}),
            json!({"v": 2, "space": "relay:abc"}),
            json!({"v": 1, "space": "../../etc"}),
            json!({"v": 1, "space": "relay:ab c"}),
            json!({"v": 1, "space": ""}),
            json!({"v": 1, "space": 5}),
            json!("text"),
        ] {
            assert_eq!(route(&d), home(), "{d}");
        }
    }

    fn reg() -> RegistrationInput {
        RegistrationInput {
            permission: "granted".into(),
            token: Some("AB".repeat(32)),
            device_id: "dev_0123456789abcdef01234567".into(),
            env: "sandbox".into(),
            app_id: "ai.cua.spaces.mobile".into(),
            locale: "en".into(),
            needs_you: true,
            done: false,
            registered: None,
        }
    }

    #[test]
    fn a_token_is_registered_once_with_what_the_relay_needs() {
        let r = registration(&reg());
        assert_eq!(r.action, "register");
        assert_eq!(r.method.as_deref(), Some("PUT"));
        assert_eq!(
            r.path.as_deref(),
            Some("/v1/push/registrations/dev_0123456789abcdef01234567")
        );
        let b = r.body.clone().unwrap();
        assert_eq!(b["provider"], "apns");
        assert_eq!(b["categories"], json!(["needs_you"]));
        assert_eq!(b["token"], "ab".repeat(32));
        assert_eq!(b["env"], "sandbox");
        // The same thing again is nothing to do.
        let mut again = reg();
        again.registered = r.fingerprint.clone();
        assert_eq!(registration(&again).action, "none");
        // A changed switch registers again.
        again.done = true;
        assert_eq!(
            registration(&again).body.unwrap()["categories"],
            json!(["needs_you", "done"])
        );
    }

    #[test]
    fn turning_it_off_or_losing_permission_withdraws_the_registration() {
        let fp = registration(&reg()).fingerprint;
        let mut off = reg();
        off.registered = fp.clone();
        off.needs_you = false;
        let r = registration(&off);
        assert_eq!(
            (r.action.as_str(), r.method.as_deref()),
            ("unregister", Some("DELETE"))
        );
        let mut denied = reg();
        denied.registered = fp;
        denied.permission = "denied".into();
        assert_eq!(registration(&denied).action, "unregister");
        // Never registered: nothing to withdraw.
        let mut never = reg();
        never.permission = "denied".into();
        assert_eq!(registration(&never).action, "none");
    }

    #[test]
    fn a_bad_token_or_no_enrollment_registers_nothing() {
        let mut short = reg();
        short.token = Some("zz".into());
        assert_eq!(registration(&short).action, "none");
        let mut missing = reg();
        missing.token = None;
        assert_eq!(
            registration(&missing).reason.as_deref(),
            Some("no usable push token yet")
        );
        let mut anon = reg();
        anon.device_id.clear();
        assert_eq!(
            registration(&anon).reason.as_deref(),
            Some("this phone is not enrolled yet")
        );
    }

    #[test]
    fn settings_say_honestly_where_push_stands() {
        let s = |permission: &str, registered, push_available| {
            settings_view(&SettingsInput {
                permission: permission.into(),
                needs_you: true,
                done: false,
                registered,
                push_available,
            })
        };
        assert_eq!(
            s("undetermined", false, false).status.action.as_deref(),
            Some("allow")
        );
        assert_eq!(
            s("denied", false, true).status.action.as_deref(),
            Some("open-settings")
        );
        assert_eq!(s("granted", true, true).status.tone, "ok");
        assert!(
            s("granted", false, false)
                .status
                .text
                .contains("not set up for this account yet")
        );
        assert!(
            s("granted", false, true)
                .status
                .text
                .starts_with("Setting up push")
        );
        let v = s("granted", true, true);
        assert_eq!(v.rows[0].label, "Needs you");
        assert!(v.rows[0].on && !v.rows[1].on);
    }
}
