// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Filling a saved login into a Space's browser through its cua-driver.
//!
//! The Keyvault broker decides *whether* a sign-in may happen and *which*
//! saved login it uses; this module is the "how": a fixed sequence of the
//! Space's own cua-driver browser tools (all input goes through cua-driver).
//!
//! 1. The tab: the caller's (`session`, `target_id`, `tab_id` from its own
//!    `get_browser_state` on the same Space), or a driver-owned isolated
//!    Chromium the Keyvault opens on the sign-in page (`browser_prepare`,
//!    `list_windows`, `get_browser_state` bind, `browser_navigate`).
//! 2. A snapshot (`get_browser_state`): the tab's URL must be on the saved
//!    login's exact origin, or nothing is typed. This is what keeps a
//!    phishing page on a look-alike origin from receiving the password.
//! 3. The fields: the first password input, the username-like input before
//!    it, and the submit button after it.
//! 4. `browser_type` into each, then `browser_click` the submit button.
//!
//! The password is only ever an argument of one `browser_type` call. No
//! result, error or log line carries it: driver messages are scrubbed of it
//! before they become an error.

use std::time::Duration;

use cua_keyvault::broker::{BrowserRef, LoginFill, LoginFilled, origin_of};
use cua_keyvault::{Error as KvError, Result as KvResult};
use serde_json::{Map, Value, json};

/// One cua-driver call's result, reduced to what the fill reads.
#[derive(Clone, Debug, Default)]
pub struct DriverReply {
    /// The tool reported a failure (a refusal is one).
    pub is_error: bool,
    /// `structuredContent`.
    pub structured: Value,
    /// The text parts, joined.
    pub text: String,
}

/// The Space's cua-driver, as the fill uses it.
#[async_trait::async_trait]
pub trait Driver: Send + Sync {
    /// Calls one driver tool.
    async fn call(&self, tool: &str, args: Value) -> KvResult<DriverReply>;
    /// Waits between polls (a test driver returns at once).
    async fn pause(&self, d: Duration) {
        tokio::time::sleep(d).await;
    }
    /// Whether this Space is reached through `cua-relay` (`/m/<machine-id>`),
    /// so a `browser_type` of the password crosses a hop this build cannot
    /// yet seal (S1; unlike teleport's bundle upload, a driver call is not a
    /// byte blob this layer can wrap). `false` (a direct or Fleet-gateway
    /// connection) by default, so a test double need not implement it.
    async fn is_relay_routed(&self) -> bool {
        false
    }
}

#[async_trait::async_trait]
impl Driver for cua_spaces::Space {
    async fn call(&self, tool: &str, args: Value) -> KvResult<DriverReply> {
        let map: Map<String, Value> = match args {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        let r = self
            .call_tool(Some("driver"), tool, map, Some(Duration::from_secs(90)))
            .await
            .map_err(|e| KvError::Backend(format!("cua-driver {tool}: {e}")))?;
        let text = r
            .content
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(DriverReply {
            is_error: r.is_error,
            structured: r.structured.unwrap_or(Value::Null),
            text,
        })
    }

    async fn is_relay_routed(&self) -> bool {
        self.spacesd().is_ok_and(|c| {
            matches!(
                c.endpoint().kind(),
                cua_spacesd_client::EndpointKind::Relay { .. }
            )
        })
    }
}

/// The driver-owned browser profile the Keyvault signs in with when the
/// caller names no tab. Named, so a later `browser_prepare` with the same
/// profile finds the session cookies there.
pub const KEYVAULT_PROFILE: &str = "cua-keyvault";

/// The inputs a login form's snapshot resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginRefs {
    /// The username-like input, when the form has one.
    pub username: Option<Field>,
    /// The password input.
    pub password: Field,
    /// The submit control, when found.
    pub submit: Option<String>,
}

/// One input ref and its type attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The `p<snapshot>:<index>` ref.
    pub r#ref: String,
    /// `type=` from the label (`text` when absent).
    pub kind: String,
}

fn attr<'a>(label: &'a str, key: &str) -> Option<&'a str> {
    label
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// Picks the password input, the username input before it and the submit
/// control after it from a `dom_refs_v1` snapshot's `refs`. Pure.
pub fn pick_login_refs(refs: &[Value]) -> Option<LoginRefs> {
    let rows: Vec<(String, String, String)> = refs
        .iter()
        .filter_map(|r| {
            Some((
                r.get("ref")?.as_str()?.to_string(),
                r.get("node")?.as_str()?.to_ascii_lowercase(),
                r.get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ))
        })
        .collect();
    let pw = rows
        .iter()
        .position(|(_, node, label)| node == "input" && attr(label, "type") == Some("password"))?;
    const USERNAME_TYPES: [&str; 4] = ["text", "email", "tel", "username"];
    let username = rows[..pw]
        .iter()
        .rev()
        .find(|(_, node, label)| {
            node == "input" && USERNAME_TYPES.contains(&attr(label, "type").unwrap_or("text"))
        })
        .map(|(r, _, label)| Field {
            r#ref: r.clone(),
            kind: attr(label, "type").unwrap_or("text").to_string(),
        });
    let after = &rows[pw + 1..];
    let submit = after
        .iter()
        .find(|(_, node, label)| {
            (node == "button" || node == "input") && attr(label, "type") == Some("submit")
        })
        .or_else(|| {
            after
                .iter()
                .find(|(_, node, label)| node == "button" && attr(label, "type") != Some("button"))
        })
        .map(|(r, _, _)| r.clone());
    Some(LoginRefs {
        username,
        password: Field {
            r#ref: rows[pw].0.clone(),
            kind: "password".into(),
        },
        submit,
    })
}

/// A driver refusal as a Keyvault error, with `secret` scrubbed out.
fn refused(tool: &str, reply: &DriverReply, secret: &str) -> KvError {
    let code = reply
        .structured
        .pointer("/refusal/code")
        .and_then(Value::as_str)
        .unwrap_or("error");
    let mut msg = reply.text.clone();
    if !secret.is_empty() {
        msg = msg.replace(secret, "***");
    }
    KvError::Backend(format!("cua-driver {tool} refused ({code}): {msg}"))
}

fn str_at<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str)
}

async fn call_ok(d: &dyn Driver, tool: &str, args: Value, secret: &str) -> KvResult<Value> {
    let r = d.call(tool, args).await.map_err(|e| match e {
        KvError::Backend(m) if !secret.is_empty() => KvError::Backend(m.replace(secret, "***")),
        other => other,
    })?;
    if r.is_error || r.structured.get("status").and_then(Value::as_str) == Some("refused") {
        return Err(refused(tool, &r, secret));
    }
    Ok(r.structured)
}

/// Opens a driver-owned isolated Chromium on `url` and binds its tab.
async fn open_own_tab(d: &dyn Driver, session: &str, url: &str) -> KvResult<(String, String)> {
    let prepared = call_ok(
        d,
        "browser_prepare",
        json!({"session": session, "allow_launch": true,
               "profile": {"mode": "isolated_named", "name": KEYVAULT_PROFILE}}),
        "",
    )
    .await?;
    let pid = prepared
        .get("prepared_pid")
        .and_then(Value::as_i64)
        .ok_or_else(|| KvError::Backend("cua-driver browser_prepare returned no pid".into()))?;
    // The window appears shortly after the launch: bounded poll.
    let mut window = None;
    for _ in 0..60 {
        let w = call_ok(d, "list_windows", json!({"session": session}), "").await?;
        window = w
            .get("windows")
            .and_then(Value::as_array)
            .and_then(|ws| {
                ws.iter().find(|x| {
                    x.get("pid").and_then(Value::as_i64) == Some(pid)
                        && x.get("window_id").and_then(Value::as_i64).is_some()
                })
            })
            .and_then(|x| x.get("window_id").and_then(Value::as_i64));
        if window.is_some() {
            break;
        }
        d.pause(Duration::from_millis(250)).await;
    }
    let window_id = window.ok_or_else(|| {
        KvError::Backend("the browser the Keyvault opened never showed a window".into())
    })?;
    let bound = call_ok(
        d,
        "get_browser_state",
        json!({"session": session, "pid": pid, "window_id": window_id}),
        "",
    )
    .await?;
    let target_id = str_at(&bound, "/target_id")
        .ok_or_else(|| KvError::Backend("cua-driver bound no browser target".into()))?
        .to_string();
    let tabs = bound
        .get("tabs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tab = tabs
        .iter()
        .find(|t| t.get("active").and_then(Value::as_bool) == Some(true))
        .or_else(|| tabs.first())
        .and_then(|t| t.get("tab_id").and_then(Value::as_str))
        .ok_or_else(|| KvError::Backend("the Keyvault's browser has no tab".into()))?
        .to_string();
    call_ok(
        d,
        "browser_navigate",
        json!({"session": session, "target_id": target_id, "tab_id": tab, "url": url}),
        "",
    )
    .await?;
    Ok((target_id, tab))
}

/// Signs in: binds or opens the tab, checks its origin, types the saved
/// login and submits. See the module docs.
pub async fn fill_with_driver(d: &dyn Driver, fill: &LoginFill) -> KvResult<LoginFilled> {
    if cua_teleport::RELAY_SEALING_ENFORCED
        && !fill.relay_plaintext_ack
        && d.is_relay_routed().await
    {
        return Err(KvError::Forbidden(format!(
            "{}; pass relay_plaintext_ack to sign in anyway",
            cua_teleport::RELAY_UNSEALED_WARNING
        )));
    }
    let secret = fill.password.as_str();
    let session = fill
        .browser
        .session
        .clone()
        .unwrap_or_else(|| format!("cua-keyvault-{:08x}", rand::random::<u32>()));
    let (target_id, tab_id) = match (&fill.browser.target_id, &fill.browser.tab_id) {
        (Some(t), Some(tab)) => (t.clone(), tab.clone()),
        (None, None) => open_own_tab(d, &session, &fill.url).await?,
        _ => {
            return Err(KvError::Invalid(
                "name both the browser target_id and tab_id, or neither".into(),
            ));
        }
    };
    let ids = json!({"session": session, "target_id": target_id, "tab_id": tab_id});
    let with = |extra: Value| {
        let mut v = ids.clone();
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };
    // The page must be on the saved login's exact origin (the anti-phishing
    // check). A page still loading its first URL gets a few tries.
    let mut snapshot = Value::Null;
    let mut page_origin = None;
    for _ in 0..20 {
        snapshot = call_ok(d, "get_browser_state", ids.clone(), "").await?;
        page_origin = str_at(&snapshot, "/url").and_then(origin_of);
        let has_password = snapshot
            .get("refs")
            .and_then(Value::as_array)
            .is_some_and(|r| pick_login_refs(r).is_some());
        if page_origin.as_deref() == Some(fill.origin.as_str()) && has_password {
            break;
        }
        d.pause(Duration::from_millis(250)).await;
    }
    if page_origin.as_deref() != Some(fill.origin.as_str()) {
        return Err(KvError::Forbidden(format!(
            "the tab is on {}, not {}; the Keyvault types a saved password only on its own site",
            page_origin.unwrap_or_else(|| "no web page".into()),
            fill.origin
        )));
    }
    let refs = snapshot
        .get("refs")
        .and_then(Value::as_array)
        .and_then(|r| pick_login_refs(r))
        .ok_or_else(|| {
            KvError::NotFound(format!(
                "no password field on {}; open the site's sign-in page first",
                fill.origin
            ))
        })?;
    if let Some(u) = &refs.username
        && !fill.username.is_empty()
    {
        // Selection-based replace does not apply to type=email inputs.
        let replace = u.kind != "email";
        call_ok(
            d,
            "browser_type",
            with(json!({"ref": u.r#ref, "text": fill.username, "replace": replace})),
            secret,
        )
        .await?;
    }
    call_ok(
        d,
        "browser_type",
        with(json!({"ref": refs.password.r#ref, "text": secret, "replace": true})),
        secret,
    )
    .await?;
    let submitted = match &refs.submit {
        Some(r) => {
            call_ok(
                d,
                "browser_click",
                with(json!({"ref": r, "delivery_mode": "foreground"})),
                secret,
            )
            .await?;
            true
        }
        None => false,
    };
    // Where the tab went (best effort: navigation may still be settling).
    let mut page_url = String::new();
    for _ in 0..8 {
        d.pause(Duration::from_millis(250)).await;
        if let Ok(s) = call_ok(d, "get_browser_state", ids.clone(), secret).await
            && let Some(u) = str_at(&s, "/url")
        {
            page_url = u.to_string();
            if !submitted
                || s.get("refs")
                    .and_then(Value::as_array)
                    .is_none_or(|r| pick_login_refs(r).is_none())
            {
                break;
            }
        }
    }
    Ok(LoginFilled {
        submitted,
        page_url,
        browser: BrowserRef {
            session: Some(session),
            target_id: Some(target_id),
            tab_id: Some(tab_id),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn refs() -> Vec<Value> {
        serde_json::from_value(json!([
            {"frame": "main", "label": null, "node": "label", "ref": "p1:0"},
            {"frame": "main", "label": "name=q id=q type=search", "node": "input", "ref": "p1:9"},
            {"frame": "main", "label": "name=username id=username type=email", "node": "input", "ref": "p1:1"},
            {"frame": "main", "label": null, "node": "label", "ref": "p1:2"},
            {"frame": "main", "label": "name=password id=password type=password", "node": "input", "ref": "p1:3"},
            {"frame": "main", "label": "type=button", "node": "button", "ref": "p1:5"},
            {"frame": "main", "label": "type=submit", "node": "button", "ref": "p1:4"}
        ]))
        .unwrap()
    }

    #[test]
    fn picks_the_username_before_the_password_and_the_submit_after() {
        let r = pick_login_refs(&refs()).unwrap();
        assert_eq!(r.password.r#ref, "p1:3");
        assert_eq!(
            r.username,
            Some(Field {
                r#ref: "p1:1".into(),
                kind: "email".into()
            })
        );
        assert_eq!(r.submit.as_deref(), Some("p1:4"));
        assert!(pick_login_refs(&refs()[..2]).is_none(), "no password field");
    }

    /// A scripted cua-driver: answers each tool from a table, records calls.
    struct Scripted {
        page_url: Mutex<String>,
        calls: Mutex<Vec<(String, Value)>>,
        refuse_type: bool,
    }

    #[async_trait::async_trait]
    impl Driver for Scripted {
        async fn call(&self, tool: &str, args: Value) -> KvResult<DriverReply> {
            self.calls
                .lock()
                .unwrap()
                .push((tool.to_string(), args.clone()));
            let structured = match tool {
                "browser_prepare" => json!({"status": "ok", "prepared_pid": 42}),
                "list_windows" => json!({"windows": [{"pid": 42, "window_id": 7}]}),
                "get_browser_state" if args.get("pid").is_some() => json!({
                    "status": "ok", "target_id": "bt-1",
                    "tabs": [{"tab_id": "tab-1", "active": true, "url": "about:blank"}]}),
                "get_browser_state" => json!({
                    "status": "ok", "url": self.page_url.lock().unwrap().clone(),
                    "refs": refs()}),
                "browser_navigate" => {
                    *self.page_url.lock().unwrap() = args["url"].as_str().unwrap().to_string();
                    json!({"status": "ok"})
                }
                "browser_type" if self.refuse_type && args["ref"] == "p1:3" => {
                    return Ok(DriverReply {
                        is_error: true,
                        structured: json!({"status": "refused",
                            "refusal": {"code": "browser_ref_stale"}}),
                        text: format!("could not type {}", args["text"].as_str().unwrap()),
                    });
                }
                _ => json!({"status": "ok"}),
            };
            Ok(DriverReply {
                is_error: false,
                structured,
                text: String::new(),
            })
        }
        async fn pause(&self, _d: Duration) {}
    }

    fn fill(url: &str, browser: BrowserRef) -> LoginFill {
        LoginFill {
            url: url.into(),
            origin: origin_of(url).unwrap(),
            username: "ada@example.test".into(),
            password: cua_keyvault::Zeroizing::new("s3cret-Pa55".into()),
            browser,
            relay_plaintext_ack: false,
        }
    }

    #[tokio::test]
    async fn opens_its_own_tab_types_both_fields_and_submits() {
        let d = Scripted {
            page_url: Mutex::new(String::new()),
            calls: Mutex::new(vec![]),
            refuse_type: false,
        };
        let out = fill_with_driver(
            &d,
            &fill("http://login.example.test:8000/", BrowserRef::default()),
        )
        .await
        .unwrap();
        assert!(out.submitted);
        assert_eq!(out.browser.tab_id.as_deref(), Some("tab-1"));
        let calls = d.calls.lock().unwrap().clone();
        let tools: Vec<&str> = calls.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            &tools[..5],
            [
                "browser_prepare",
                "list_windows",
                "get_browser_state",
                "browser_navigate",
                "get_browser_state"
            ]
        );
        let typed: Vec<&Value> = calls
            .iter()
            .filter(|(t, _)| t == "browser_type")
            .map(|(_, a)| a)
            .collect();
        assert_eq!(typed[0]["ref"], "p1:1");
        assert_eq!(typed[0]["text"], "ada@example.test");
        assert_eq!(typed[0]["replace"], false, "email inputs are not replaced");
        assert_eq!(typed[1]["ref"], "p1:3");
        assert_eq!(typed[1]["text"], "s3cret-Pa55");
        let click = calls.iter().find(|(t, _)| t == "browser_click").unwrap();
        assert_eq!(click.1["ref"], "p1:4");
        // The prepared profile is the Keyvault's own, never the user's.
        assert_eq!(calls[0].1["profile"]["name"], KEYVAULT_PROFILE);
    }

    #[tokio::test]
    async fn refuses_a_tab_on_another_origin_before_typing() {
        let d = Scripted {
            page_url: Mutex::new("https://login.example.test.evil.test/".into()),
            calls: Mutex::new(vec![]),
            refuse_type: false,
        };
        let err = fill_with_driver(
            &d,
            &fill(
                "http://login.example.test:8000/",
                BrowserRef {
                    session: Some("s".into()),
                    target_id: Some("bt-9".into()),
                    tab_id: Some("tab-9".into()),
                },
            ),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, KvError::Forbidden(_)), "{err:?}");
        assert!(err.to_string().contains("evil.test"), "{err}");
        assert!(
            !d.calls
                .lock()
                .unwrap()
                .iter()
                .any(|(t, _)| t == "browser_type"),
            "nothing typed"
        );
    }

    #[tokio::test]
    async fn a_driver_refusal_never_carries_the_password() {
        let d = Scripted {
            page_url: Mutex::new("http://login.example.test:8000/".into()),
            calls: Mutex::new(vec![]),
            refuse_type: true,
        };
        let err = fill_with_driver(
            &d,
            &fill(
                "http://login.example.test:8000/",
                BrowserRef {
                    session: Some("s".into()),
                    target_id: Some("bt-1".into()),
                    tab_id: Some("tab-1".into()),
                },
            ),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("browser_ref_stale"), "{err}");
        assert!(!err.contains("s3cret-Pa55"), "{err}");
    }
}
