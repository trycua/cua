// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the phone app needs from the core that the desktop apps did not:
//! starting a task, tool rows, the outbox, approval scopes, steering a running
//! agent, dictation, diffs and push. Each submodule is plain data in and plain
//! data out (see the module docs); this file only routes JSON calls to them,
//! so [`crate::dispatch`] answers them like every other method and any shell
//! (the React Native app today; web, SwiftUI or Electron when they want the
//! same screens) gets the same words.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::CoreError;

pub mod approvals;
pub mod bars;
pub mod compose;
pub mod diff;
pub mod directory;
pub mod outbox;
pub mod push;
pub mod start_task;
pub mod tool_rows;
pub mod voice;

/// The methods [`call`] answers (also listed in [`crate::dispatch::METHODS`]).
pub const METHODS: &[&str] = &[
    "startTask.view",
    "startTask.plan",
    "startTask.parseRepo",
    "toolRows.group",
    "toolRows.label",
    "outbox.initial",
    "outbox.reduce",
    "outbox.view",
    "outbox.found",
    "cache.line",
    "approvals.view",
    "approvals.result",
    "sendMode.view",
    "voice.view",
    "voice.insert",
    "push.route",
    "push.registration",
    "push.settings",
    "diff.labels",
    "diff.changes",
    "diff.file",
    "diff.preview",
    "directory.view",
    "placement.offered",
];

fn arg<T: DeserializeOwned>(a: &Value, name: &str) -> Result<T, CoreError> {
    let v = a.get(name).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| CoreError::Invalid(format!("argument {name}: {e}")))
}

fn out<T: Serialize>(v: T) -> Result<Value, CoreError> {
    serde_json::to_value(v).map_err(|e| CoreError::Invalid(e.to_string()))
}

/// Calls a phone method; `None` when `method` is not one of [`METHODS`].
pub fn call(method: &str, a: &Value) -> Option<Result<Value, CoreError>> {
    Some(match method {
        "startTask.view" => arg(a, "input").and_then(|i| out(start_task::view(&i))),
        "startTask.plan" => (|| {
            let model: Option<String> = arg(a, "model")?;
            out(start_task::plan(
                &arg(a, "input")?,
                &arg::<String>(a, "prompt")?,
                &arg::<String>(a, "clientId")?,
                model.as_deref(),
            ))
        })(),
        "startTask.parseRepo" => arg::<String>(a, "text").and_then(|t| {
            out(start_task::parse_repo(&t).map(|(name, url)| start_task::RepoRef { url, name }))
        }),
        "toolRows.group" => arg::<Vec<tool_rows::ToolCallInput>>(a, "calls")
            .and_then(|c| out(tool_rows::group_view(&c))),
        "toolRows.label" => arg::<Vec<String>>(a, "cats").and_then(|c| {
            let cats: Vec<&str> = c.iter().map(String::as_str).collect();
            out(tool_rows::group_label(&cats))
        }),
        "outbox.initial" => out(outbox::State::default()),
        "outbox.reduce" => (|| {
            let state: Option<outbox::State> = arg(a, "state")?;
            out(outbox::reduce(
                &state.unwrap_or_default(),
                &arg(a, "action")?,
            ))
        })(),
        "outbox.view" => (|| {
            let state: Option<outbox::State> = arg(a, "state")?;
            let space: Option<String> = arg(a, "space")?;
            out(outbox::view(&state.unwrap_or_default(), space.as_deref()))
        })(),
        "outbox.found" => (|| {
            let entries: Vec<outbox::EntryLite> = arg(a, "entries")?;
            out(outbox::found_in_thread(
                &arg(a, "message")?,
                &entries,
                &arg::<String>(a, "me")?,
            ))
        })(),
        "cache.line" => (|| {
            out(outbox::cache_line(
                arg(a, "savedMs")?,
                arg::<Option<u64>>(a, "nowMs")?.unwrap_or(0),
                arg::<Option<bool>>(a, "offline")?.unwrap_or(false),
            ))
        })(),
        "approvals.view" => arg(a, "input").and_then(|i| out(approvals::view(&i))),
        "approvals.result" => (|| {
            let scope: Option<String> = arg(a, "scope")?;
            out(approvals::result_line(
                arg(a, "approve")?,
                scope.as_deref(),
                &arg::<Option<String>>(a, "title")?.unwrap_or_default(),
            ))
        })(),
        "sendMode.view" => arg(a, "input").and_then(|i| out(compose::send_mode(&i))),
        "voice.view" => arg(a, "input").and_then(|i| out(voice::view(&i))),
        "voice.insert" => (|| {
            out(voice::insert_view(
                &arg::<String>(a, "text")?,
                arg::<Option<usize>>(a, "start")?.unwrap_or(0),
                arg::<Option<usize>>(a, "end")?.unwrap_or(0),
                &arg::<String>(a, "transcript")?,
            ))
        })(),
        "push.route" => {
            let null = Value::Null;
            out(push::route(a.get("data").unwrap_or(&null)))
        }
        "push.registration" => arg(a, "input").and_then(|i| out(push::registration(&i))),
        "push.settings" => arg(a, "input").and_then(|i| out(push::settings_view(&i))),
        "diff.labels" => out(diff::labels()),
        "diff.changes" => arg(a, "input").and_then(|i| out(diff::changes_view(&i))),
        "diff.file" => arg(a, "input").and_then(|i| out(diff::file_diff(&i))),
        "diff.preview" => arg(a, "input").and_then(|i| out(diff::preview_view(&i))),
        "directory.view" => arg(a, "input").and_then(|i| out(directory::view(&i))),
        "placement.offered" => (|| {
            let members: Vec<directory::Member> = arg(a, "members")?;
            out(directory::offered(&members, &arg(a, "need")?))
        })(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(method: &str, args: Value) -> Value {
        call(method, &args)
            .expect("a phone method")
            .expect("answers")
    }

    #[test]
    fn every_listed_method_is_answered_and_the_dispatcher_lists_them() {
        for m in METHODS {
            let r = call(m, &json!({}));
            assert!(r.is_some(), "{m} is listed but not routed");
            assert!(
                crate::dispatch::METHODS.contains(m),
                "{m} is missing from dispatch::METHODS"
            );
        }
        assert!(call("nope.nope", &json!({})).is_none());
        // And through the dispatcher itself.
        let v = crate::dispatch::call_value(
            "sendMode.view",
            json!({"input": {"running": true, "hasText": true, "canSteer": true}}),
        )
        .unwrap();
        assert_eq!(v["primary"]["label"], "Queue");
    }

    #[test]
    fn a_task_goes_from_the_screen_to_the_wire_through_json() {
        let input = json!({"machines": [{"id": "m1", "name": "Studio Mac", "online": true, "canCreate": true, "os": ["linux"]}], "choice": {"repo": "acme/app"}});
        let v = run("startTask.view", json!({"input": input}));
        assert_eq!(v["title"], "What do you want to do?");
        assert_eq!(v["where"], json!({"kind": "auto"}));
        let p = run(
            "startTask.plan",
            json!({"input": input, "prompt": "fix it", "clientId": "task-0001"}),
        );
        assert_eq!(p["ok"], true);
        assert_eq!(p["request"]["message"], "@claude fix it");
        assert_eq!(p["request"]["repo"]["url"], "https://github.com/acme/app");
        assert_eq!(p["request"]["client_id"], "task-0001");
        assert_eq!(
            run(
                "startTask.parseRepo",
                json!({"text": "https://github.com/acme/app.git"})
            )["name"],
            "acme/app"
        );
        assert_eq!(
            run("startTask.parseRepo", json!({"text": "nope"})),
            Value::Null
        );
    }

    #[test]
    fn the_outbox_reduces_through_json() {
        let r = run(
            "outbox.reduce",
            json!({"action": {"type": "send", "nowMs": 5, "id": "a", "space": "s", "text": "hi"}}),
        );
        assert_eq!(r["effects"][0], json!({"type": "deliver", "id": "a"}));
        let state = r["state"].clone();
        let v = run("outbox.view", json!({"state": state, "space": "s"}));
        assert_eq!(v["rows"][0]["label"], "Sending\u{2026}");
        assert_eq!(run("outbox.initial", json!({}))["schema"], 1);
    }

    #[test]
    fn tool_rows_group_through_json() {
        let g = run(
            "toolRows.group",
            json!({"calls": [{"cat": "ran", "line": "Ran ls"}, {"cat": "ran", "line": "Ran pwd", "state": "running"}]}),
        );
        assert_eq!(g["phrase"], "Ran 2 commands");
        assert_eq!(g["running"], true);
        assert_eq!(
            run(
                "toolRows.label",
                json!({"cats": ["read", "read", "edited"]})
            ),
            "Read 2 files, edited 1 file"
        );
        assert_eq!(run("toolRows.label", json!({"cats": []})), Value::Null);
    }

    /// House wording: "computers", never "devices" or "machines", and no middle dot, in anything a person reads.
    #[test]
    fn nothing_a_person_reads_breaks_the_house_wording() {
        fn words(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::String(s) => out.push(s.clone()),
                Value::Array(a) => a.iter().for_each(|x| words(x, out)),
                Value::Object(o) => o.values().for_each(|x| words(x, out)),
                _ => {}
            }
        }
        let start = json!({"machines": [{"id": "m", "name": "Mac", "online": false}, {"id": "n", "name": "PC", "online": true, "canCreate": false}], "spaces": [{"id": "relay:s", "name": "api", "online": false}], "choice": {"repo": "x y"}});
        let mut all = vec![
            run("startTask.view", json!({"input": start})),
            run(
                "startTask.plan",
                json!({"input": start, "prompt": "", "clientId": "task-0002"}),
            ),
            run(
                "approvals.view",
                json!({"input": {"pending": [{"seq": 1, "kind": "push", "scopeChoices": ["once", "space"]}, {"seq": 2, "kind": "login", "scopeChoices": ["space"]}], "current": 2}}),
            ),
            run(
                "approvals.result",
                json!({"approve": true, "scope": "space", "title": "Push"}),
            ),
            run(
                "sendMode.view",
                json!({"input": {"running": true, "hasText": true, "canSteer": true, "queued": 3}}),
            ),
            run("sendMode.view", json!({"input": {}})),
            run("push.settings", json!({"input": {"permission": "denied"}})),
            run(
                "push.settings",
                json!({"input": {"permission": "granted", "registered": false}}),
            ),
            run(
                "diff.changes",
                json!({"input": {"repo": "a/b", "branch": "x", "base": "main", "files": [{"path": "a/b.png", "binary": true, "mime": "image/png"}], "truncated": true}}),
            ),
            run(
                "diff.file",
                json!({"input": {"path": "a", "patch": "", "truncated": true}}),
            ),
            run(
                "diff.preview",
                json!({"input": {"path": "a.zip", "binary": true}}),
            ),
            run("diff.labels", json!({})),
            run(
                "cache.line",
                json!({"savedMs": 1000, "nowMs": 9_000_000, "offline": true}),
            ),
            run("cache.line", json!({"nowMs": 5, "offline": true})),
        ];
        for denied in [json!("denied"), json!("restricted")] {
            all.push(run("voice.view", json!({"input": {"microphone": denied, "speech": "granted", "recognizerAvailable": true}})));
        }
        for e in ["no-speech", "network", "audio", "other"] {
            all.push(run("voice.view", json!({"input": {"microphone": "granted", "speech": "granted", "error": e, "listening": true}})));
        }
        let mut s = outbox::State {
            online: false,
            messages: vec![outbox::Message {
                id: "a".into(),
                space: "s".into(),
                text: "t".into(),
                state: outbox::MessageState::Failed,
                ..Default::default()
            }],
            ..Default::default()
        };
        all.push(serde_json::to_value(outbox::view(&s, None)).unwrap());
        s.online = true;
        all.push(serde_json::to_value(outbox::view(&s, None)).unwrap());
        let mut texts = vec![];
        all.iter().for_each(|v| words(v, &mut texts));
        assert!(texts.len() > 40);
        for t in &texts {
            assert!(
                !t.contains(['\u{b7}', '\u{2022}', '\u{2219}', '\u{22c5}']),
                "middle dot in {t:?}"
            );
            if t == "machine" || t.starts_with("machine:") {
                continue; // an identifier (the kind of a row), never shown
            }
            let lower = t.to_lowercase();
            for banned in ["device", "machine"] {
                assert!(!lower.contains(banned), "{banned} in {t:?}");
            }
        }
    }
}
