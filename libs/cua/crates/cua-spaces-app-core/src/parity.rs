// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Scripted flows every shell replays, and their golden transcripts.
//!
//! A flow (`parity/<name>.json`) is host-neutral: user actions and fixture
//! data. Replaying it records a transcript of small frames (strings,
//! booleans, integers) after each step. Every host replays it through its
//! own binding and must produce `parity/golden/<name>.json`:
//!
//! - Rust, typed ([`TypedHost`]): `tests/parity.rs`;
//! - the Tauri app, through its `app_core_call` command ([`run`] with a
//!   JSON host): `apps/cua-spaces/src-tauri/tests/app_core_parity.rs`;
//! - the webview, through `src/model/*.ts` on the wasm core:
//!   `apps/cua-spaces/src/model/parity.test.ts`;
//! - Swift, through the UniFFI records:
//!   `apps/cua-spaces-macos/Tests/CuaSpacesAppTests/ParityTests.swift`.
//!
//! `UPDATE_PARITY=1 cargo test -p cua-spaces-app-core --test parity`
//! rewrites the goldens from the typed Rust run.

use serde_json::{Value, json};

fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// The flows: `(name, flow JSON, golden JSON)`.
pub const FLOWS: &[(&str, &str, &str)] = &[
    (
        "create-space",
        include_str!("../parity/create-space.json"),
        include_str!("../parity/golden/create-space.json"),
    ),
    (
        "create-resources",
        include_str!("../parity/create-resources.json"),
        include_str!("../parity/golden/create-resources.json"),
    ),
    (
        "create-gpu",
        include_str!("../parity/create-gpu.json"),
        include_str!("../parity/golden/create-gpu.json"),
    ),
    (
        "create-cancel",
        include_str!("../parity/create-cancel.json"),
        include_str!("../parity/golden/create-cancel.json"),
    ),
    (
        "teleport-review",
        include_str!("../parity/teleport-review.json"),
        include_str!("../parity/golden/teleport-review.json"),
    ),
    (
        "teleport-sign-ins",
        include_str!("../parity/teleport-sign-ins.json"),
        include_str!("../parity/golden/teleport-sign-ins.json"),
    ),
    (
        "keyvault-approve-deny",
        include_str!("../parity/keyvault-approve-deny.json"),
        include_str!("../parity/golden/keyvault-approve-deny.json"),
    ),
    (
        "keyvault-unlock",
        include_str!("../parity/keyvault-unlock.json"),
        include_str!("../parity/golden/keyvault-unlock.json"),
    ),
    (
        "main-window",
        include_str!("../parity/main-window.json"),
        include_str!("../parity/golden/main-window.json"),
    ),
    (
        "notch",
        include_str!("../parity/notch.json"),
        include_str!("../parity/golden/notch.json"),
    ),
    (
        "notch-drag-trigger",
        include_str!("../parity/notch-drag-trigger.json"),
        include_str!("../parity/golden/notch-drag-trigger.json"),
    ),
    (
        "provisioning",
        include_str!("../parity/provisioning.json"),
        include_str!("../parity/golden/provisioning.json"),
    ),
    (
        "stream-section",
        include_str!("../parity/stream-section.json"),
        include_str!("../parity/golden/stream-section.json"),
    ),
    (
        "space-facts",
        include_str!("../parity/space-facts.json"),
        include_str!("../parity/golden/space-facts.json"),
    ),
    (
        "picker-grid",
        include_str!("../parity/picker-grid.json"),
        include_str!("../parity/golden/picker-grid.json"),
    ),
    (
        "delete-space",
        include_str!("../parity/delete-space.json"),
        include_str!("../parity/golden/delete-space.json"),
    ),
    (
        "space-power",
        include_str!("../parity/space-power.json"),
        include_str!("../parity/golden/space-power.json"),
    ),
    (
        "create-progress",
        include_str!("../parity/create-progress.json"),
        include_str!("../parity/golden/create-progress.json"),
    ),
    (
        "devices",
        include_str!("../parity/devices.json"),
        include_str!("../parity/golden/devices.json"),
    ),
    (
        "driver-card",
        include_str!("../parity/driver-card.json"),
        include_str!("../parity/golden/driver-card.json"),
    ),
    (
        "share-sheet",
        include_str!("../parity/share-sheet.json"),
        include_str!("../parity/golden/share-sheet.json"),
    ),
    (
        "agents-page",
        include_str!("../parity/agents-page.json"),
        include_str!("../parity/golden/agents-page.json"),
    ),
    (
        "drive-page",
        include_str!("../parity/drive-page.json"),
        include_str!("../parity/golden/drive-page.json"),
    ),
    (
        "menu-count",
        include_str!("../parity/menu-count.json"),
        include_str!("../parity/golden/menu-count.json"),
    ),
    (
        "drive-onboarding",
        include_str!("../parity/drive-onboarding.json"),
        include_str!("../parity/golden/drive-onboarding.json"),
    ),
    (
        "drive-storage",
        include_str!("../parity/drive-storage.json"),
        include_str!("../parity/golden/drive-storage.json"),
    ),
    (
        "notifications",
        include_str!("../parity/notifications.json"),
        include_str!("../parity/golden/notifications.json"),
    ),
    (
        "about",
        include_str!("../parity/about.json"),
        include_str!("../parity/golden/about.json"),
    ),
    (
        "your-cloud",
        include_str!("../parity/your-cloud.json"),
        include_str!("../parity/golden/your-cloud.json"),
    ),
    (
        "telemetry-funnel",
        include_str!("../parity/telemetry-funnel.json"),
        include_str!("../parity/golden/telemetry-funnel.json"),
    ),
    (
        "launch-at-login",
        include_str!("../parity/launch-at-login.json"),
        include_str!("../parity/golden/launch-at-login.json"),
    ),
    (
        "experiments",
        include_str!("../parity/experiments.json"),
        include_str!("../parity/golden/experiments.json"),
    ),
    (
        "placement-picker",
        include_str!("../parity/placement-picker.json"),
        include_str!("../parity/golden/placement-picker.json"),
    ),
];

/// The core calls a flow makes, over JSON. A host implements it with its
/// own binding; [`TypedHost`] is the Rust one, [`JsonHost`] wraps any
/// `call(method, args)`.
pub trait Host {
    /// Calls a core method (see [`crate::dispatch::METHODS`]).
    fn call(&self, method: &str, args: Value) -> Result<Value, String>;
}

/// The Rust host: the dispatcher itself, whose arms are the typed API.
pub struct TypedHost;

impl Host for TypedHost {
    fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        crate::dispatch::call_value(method, args).map_err(|e| e.to_string())
    }
}

/// A host over any JSON `call` (the Tauri command, a test double).
pub struct JsonHost<F: Fn(&str, &str) -> Result<String, String>>(pub F);

impl<F: Fn(&str, &str) -> Result<String, String>> Host for JsonHost<F> {
    fn call(&self, method: &str, args: Value) -> Result<Value, String> {
        let out = (self.0)(method, &args.to_string())?;
        serde_json::from_str(&out).map_err(|e| e.to_string())
    }
}

fn facts(v: &Value) -> Vec<Value> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
                    let copy = &f["copy"];
                    let copy = if copy.is_object() {
                        format!(
                            " [copy {} {} {} / {} {} {}ms]",
                            s(&copy["text"]),
                            s(&copy["symbol"]),
                            s(&copy["help"]),
                            s(&copy["doneSymbol"]),
                            s(&copy["doneHelp"]),
                            copy["confirmMs"]
                        )
                    } else {
                        String::new()
                    };
                    let help = f["help"]
                        .as_str()
                        .filter(|h| *h != s(&f["value"]))
                        .map(|h| format!(" ?{}", h.replace('\n', " | ")))
                        .unwrap_or_default();
                    let warning = if f["warning"].is_object() {
                        format!(
                            " [warn {} {}]",
                            s(&f["warning"]["symbol"]),
                            s(&f["warning"]["help"])
                        )
                    } else {
                        String::new()
                    };
                    Value::String(format!(
                        "{}: {}{help}{warning}{copy}",
                        s(&f["label"]),
                        s(&f["value"])
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Projections: the frame each host records. Hosts that hold typed values
/// (Swift) write the same projection by hand.
pub mod frame {
    use super::*;

    /// After each wizard action.
    pub fn wizard(view: &Value) -> Value {
        let enabled = |key: &str| -> Vec<Value> {
            view[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|t| t["enabled"] == true)
                        .map(|t| t["id"].clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        let image_field = {
            let f = &view["imageField"];
            json!({
                "text": f["text"],
                "open": f["open"],
                "custom": f["custom"],
                "error": f["error"],
                // The rows as shown (none while closed).
                "rows": f["groups"].as_array().filter(|_| f["open"] == true).map(|a| a.iter().flat_map(|g| {
                    let group = str_of(&g["label"]);
                    g["rows"].as_array().cloned().unwrap_or_default().into_iter().map(move |r| Value::String(format!(
                        "{group}: {} ({}){}{}",
                        str_of(&r["ref"]), str_of(&r["label"]),
                        if r["selected"] == true { " selected" } else { "" },
                        if r["highlighted"] == true { " highlighted" } else { "" })))
                }).collect::<Vec<_>>()).unwrap_or_default(),
            })
        };
        let address = {
            let a = &view["address"];
            json!({
                "canSubmit": a["canSubmit"],
                "submitLabel": a["submitLabel"],
                "error": a["error"],
                "submit": if a["submit"].is_null() { Value::Null } else {
                    Value::String(format!("{} token={} name={}", str_of(&a["submit"]["url"]),
                        str_of(&a["submit"]["token"]), str_of(&a["submit"]["name"])))
                },
            })
        };
        json!({
            "step": view["step"],
            "canContinue": view["canContinue"],
            "primaryLabel": view["primaryLabel"],
            "image": view["image"]["ref"],
            "placement": view["plan"]["placement"],
            "runtime": view["runtime"],
            "runtimes": view["runtimes"].as_array().map(|a| a.iter().map(|r| r["value"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "kinds": enabled("kindTiles"),
            "placements": enabled("placements"),
            "placementError": view["placementError"],
            "nameInvalid": view["nameInvalid"],
            "cpusText": view["cpusText"],
            "memoryText": view["memoryText"],
            "ranges": format!("cpus {}-{}, memory {}-{}", view["minCpus"], view["maxCpus"], view["minMemoryGb"], view["maxMemoryGb"]),
            // The disk slider: `64 GB (20-500)`, when it shows.
            "disk": if view["diskEditable"] == true {
                Value::String(format!("{} ({}-{}){} ~{} ?{}", str_of(&view["diskText"]), view["minDiskGb"], view["maxDiskGb"],
                    view["diskResetLabel"].as_str().map(|r| format!(" [{r}]")).unwrap_or_default(),
                    str_of(&view["diskNote"]), str_of(&view["diskHelp"])))
            } else { Value::Null },
            // `Label: value [symbol ?help]`.
            "resourceFacts": view["resourceFacts"].as_array().map(|a| a.iter().map(|f| {
                let mut out = format!("{}: {}", str_of(&f["label"]), str_of(&f["value"]));
                if let Some(s) = f["symbol"].as_str() {
                    out.push_str(&format!(" [{s} ?{}]", str_of(&f["help"])));
                }
                Value::String(out)
            }).collect::<Vec<_>>()).unwrap_or_default(),
            "resourcesError": view["resourcesError"],
            // `Label [x] -> url` (checked), `Label [ ] !reason` (disabled),
            // or null (no GPU for this runtime).
            "gpu": if view["gpu"].is_null() { Value::Null } else {
                let g = &view["gpu"];
                Value::String(format!("{} [{}]{}{}",
                    str_of(&g["label"]),
                    if g["on"] == true { "x" } else { " " },
                    g["reason"].as_str().map(|r| format!(" !{r}")).unwrap_or_default(),
                    g["learnMoreUrl"].as_str().map(|u| format!(" {} -> {u}", str_of(&g["learnMoreLabel"]))).unwrap_or_default()))
            },
            // `About $0.18/hour`, `Free (runs on this Mac)`, or null.
            "price": view["price"],
            "labels": format!("{} | {} | {}", str_of(&view["labels"]["cancel"]), str_of(&view["labels"]["back"]), str_of(&view["labels"]["advanced"])),
            "fields": view["fields"].as_array().map(|a| a.iter().map(field).collect::<Vec<_>>()).unwrap_or_default(),
            "imageField": image_field,
            "address": address,
        })
    }

    fn str_of(v: &Value) -> &str {
        v.as_str().unwrap_or("")
    }

    /// `id: Label [placeholder] !error (advanced)`.
    fn field(f: &Value) -> Value {
        let mut out = format!("{}: {}", str_of(&f["id"]), str_of(&f["label"]));
        if let Some(p) = f["placeholder"].as_str() {
            out.push_str(&format!(" [{p}]"));
        }
        if let Some(e) = f["error"].as_str() {
            out.push_str(&format!(" !{e}"));
        }
        if f["advanced"] == true {
            out.push_str(" (advanced)");
        }
        Value::String(out)
    }

    /// The plan's SDK call, the Summary and the notices around the call.
    pub fn plan(args: &Value, view: &Value, creating: &Value, failed: &Value) -> Value {
        json!({
            "createSpace": args,
            "summary": facts(&view["summary"]),
            "openDesktop": view["plan"]["openDesktop"],
            "creating": creating,
            "failed": failed,
        })
    }

    /// The Space list.
    pub fn roster(state: &Value) -> Value {
        json!({
            "mode": state["mode"],
            "ids": state["spaces"].as_array().map(|a| a.iter().map(|x| x["id"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "names": state["spaces"].as_array().map(|a| a.iter().map(|x| x["name"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "selectedId": state["selectedId"],
            "focusIndex": state["focusIndex"],
            "notice": state["notice"]["text"],
        })
    }

    /// The sidebar.
    pub fn sidebar(v: &Value) -> Value {
        json!({
            "thisMachine": v["thisMachine"]["name"],
            "sections": v["sections"].as_array().map(|a| a.iter().map(|sec| {
                Value::String(format!("{}: {}", sec["title"].as_str().unwrap_or(""),
                    sec["rows"].as_array().map(|r| r.iter().map(|row| format!("{}{}{}",
                        row["name"].as_str().unwrap_or(""),
                        if row["dim"] == true { " (dim)" } else { "" },
                        if row["selected"] == true { " (selected)" } else { "" })).collect::<Vec<_>>().join(", ")).unwrap_or_default()))
            }).collect::<Vec<_>>()).unwrap_or_default(),
            "selectedId": v["selectedId"],
            "emptyText": v["emptyText"],
        })
    }

    /// The selected Space's detail.
    pub fn detail(v: &Value) -> Value {
        json!({
            "title": v["title"],
            "facts": facts(&v["facts"]),
            "canStream": v["canStream"],
            "showSections": v["showSections"],
            "deleteLabel": v["deleteLabel"],
            "previewText": v["previewText"],
        })
    }

    fn opt_str(v: &Value) -> String {
        v.as_str().map(str::to_string).unwrap_or_else(|| "-".into())
    }

    /// A row's power button: ` <power Suspend>`, ` <power Resuming… on
    /// busy>` (`on`: a press turns it on); empty without one.
    pub fn power(b: &Value) -> String {
        if !b.is_object() {
            return String::new();
        }
        format!(
            " <{} {}{}{}>",
            str_of(&b["symbol"]),
            str_of(&b["help"]),
            if b["turnOn"] == true { " on" } else { "" },
            if b["busy"] == true { " busy" } else { "" },
        )
    }

    fn strings(v: &Value) -> Vec<Value> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|x| Value::String(str_of(x).to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A main-window sidebar row: `<os icon> name [@ place] | status |
    /// detail (dim) (selected)`.
    fn sidebar_row(r: &Value) -> String {
        format!(
            "<{}> {}{} | {} | {}{}{}",
            str_of(&r["osIcon"]),
            str_of(&r["name"]),
            r["place"]
                .as_str()
                .map(|p| format!(" @ {p}"))
                .unwrap_or_default(),
            str_of(&r["statusText"]),
            str_of(&r["detail"]),
            if r["dim"] == true { " (dim)" } else { "" },
            if r["selected"] == true {
                " (selected)"
            } else {
                ""
            }
        )
    }

    /// The main window's sidebar: This machine, then each section's rows.
    pub fn window_sidebar(v: &Value) -> Value {
        json!({
            "thisMachine": if v["thisMachine"].is_null() { Value::Null } else { Value::String(sidebar_row(&v["thisMachine"])) },
            "sections": v["sections"].as_array().map(|a| a.iter().flat_map(|sec| {
                let title = str_of(&sec["title"]).to_string();
                std::iter::once(Value::String(format!("# {title}"))).chain(
                    sec["rows"].as_array().cloned().unwrap_or_default().into_iter()
                        .map(|r| Value::String(sidebar_row(&r))))
            }).collect::<Vec<_>>()).unwrap_or_default(),
            "selectedId": v["selectedId"],
            "emptyText": v["emptyText"],
        })
    }

    /// A Space's detail with its toolbar, Delete question and sections.
    pub fn window_detail(v: &Value) -> Value {
        json!({
            "title": v["title"],
            "facts": facts(&v["facts"]),
            "isHost": v["isHost"],
            "canStream": v["canStream"],
            "previewText": v["previewText"],
            "actions": v["actions"].as_array().map(|a| a.iter().map(|x| Value::String(format!(
                "{}: {} [{}] ?{}{}{}{}",
                str_of(&x["id"]), str_of(&x["label"]), opt_str(&x["symbol"]), str_of(&x["help"]),
                if x["enabled"] == true { "" } else { " (disabled)" },
                if x["destructive"] == true { " (destructive)" } else { "" },
                if x["primary"] == true { " (primary)" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
            "confirm": confirm(&v["confirm"]),
            "sections": strings(&v["sections"]),
        })
    }

    /// The Delete question: `title | message | confirm [(disabled: why)]
    /// [/ remove] / cancel`.
    fn confirm(c: &Value) -> String {
        format!(
            "{} | {} | {}{}{} / {}",
            str_of(&c["title"]),
            str_of(&c["message"]),
            str_of(&c["confirmLabel"]),
            if c["confirmEnabled"] == false {
                format!(" (disabled: {})", str_of(&c["disabledReason"]))
            } else {
                String::new()
            },
            c["removeLabel"]
                .as_str()
                .map(|r| format!(" / {r}"))
                .unwrap_or_default(),
            str_of(&c["cancelLabel"])
        )
    }

    /// `key: value` per field, in the order given.
    pub fn copy(v: &Value, keys: &[&str]) -> Value {
        Value::Array(
            keys.iter()
                .map(|k| Value::String(format!("{k}: {}", str_of(&v[*k]))))
                .collect(),
        )
    }

    /// The Space sections' words, in [`crate::spaces::sidebar::DetailCopy`] order.
    pub const DETAIL_COPY_KEYS: &[&str] = &[
        "streamLoading",
        "streamEmpty",
        "streamFailed",
        "streamNoMatch",
        "agentsLoading",
        "agentsEmpty",
        "agentsFailed",
        "agentsNoMatch",
        "dropCaption",
        "sendFile",
        "teleportApp",
        "teleportSymbol",
        "teleportSymbolActive",
    ];

    /// The first run's words, in [`crate::onboarding::OnboardingCopy`] order.
    pub const ONBOARDING_COPY_KEYS: &[&str] = &[
        "back",
        "skip",
        "continueLabel",
        "tryAgain",
        "checking",
        "installScript",
        "install",
        "installing",
        "addToPath",
        "shadowed",
        "signIn",
        "signInWaiting",
        "agentsLooking",
        "agentsNone",
        "agentsSkills",
        "agentsMcp",
        "agentsSetUp",
        "agentsSettingUp",
        "agentsDoneTitle",
        "agentsDoneLede",
        "permissionsTitle",
        "openSettings",
    ];

    /// The Keyvault's words, in [`crate::keyvault::view::KvLabels`] order.
    pub const KV_LABEL_KEYS: &[&str] = &[
        "deny",
        "review",
        "cancel",
        "setUp",
        "unlock",
        "revokeAll",
        "confirmNote",
        "protectionTitle",
    ];

    /// The This machine page.
    pub fn host_panel(v: &Value) -> Value {
        json!({
            "title": v["title"],
            "summary": v["summary"],
            "configured": v["configured"],
            "facts": facts(&v["facts"]),
            "clients": if v["clientsTitle"].is_null() { Value::Null } else {
                Value::String(format!("{}: {}", str_of(&v["clientsTitle"]),
                    match v["clientsEmpty"].as_str() {
                        Some(e) => e.to_string(),
                        None => v["clients"].as_array().map(|a| a.iter().map(|c| str_of(c).to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                    }))
            },
            "permissions": std::iter::once(v["permissionsTitle"].clone()).filter(|t| !t.is_null())
                .chain(v["permissions"].as_array().cloned().unwrap_or_default().into_iter().map(|p| Value::String(format!(
                    "{}: {} ?{} <{}>", str_of(&p["id"]), str_of(&p["title"]), str_of(&p["help"]), opt_str(&p["settingsUrl"])))))
                .collect::<Vec<_>>(),
            "openSettings": v["openSettingsLabel"],
            "actions": v["actions"].as_array().map(|a| a.iter().map(|x| Value::String(format!("{}: {}{}",
                str_of(&x["id"]), str_of(&x["label"]), if x["destructive"] == true { " (destructive)" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
        })
    }

    /// The host setup form.
    pub fn host_form(v: &Value) -> Value {
        let r = &v["request"];
        json!({
            "title": v["title"],
            "lede": v["lede"],
            "fields": v["fields"].as_array().map(|a| a.iter().map(|f| Value::String(if f["toggle"] == true {
                format!("{}: {} = {}{}", str_of(&f["id"]), str_of(&f["label"]), if f["on"] == true { "on" } else { "off" },
                    if f["advanced"] == true { " (advanced)" } else { "" })
            } else {
                format!("{}: {} [{}] = {}{}{}", str_of(&f["id"]), str_of(&f["label"]), opt_str(&f["placeholder"]), str_of(&f["value"]),
                    if f["invalid"] == true { " !invalid" } else { "" },
                    if f["advanced"] == true { " (advanced)" } else { "" })
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "advanced": format!("{}{}", str_of(&v["advancedLabel"]), if v["advancedOpen"] == true { " (open)" } else { "" }),
            "buttons": format!("{} / {}", str_of(&v["backLabel"]), str_of(&v["submitLabel"])),
            "canSubmit": v["canSubmit"],
            "busy": v["busy"],
            "error": v["error"],
            "request": if r.is_null() { Value::Null } else { Value::String(format!(
                "mode={} relay={} direct={} name={} allow={}", str_of(&r["mode"]), opt_str(&r["relayUrl"]), opt_str(&r["direct"]),
                opt_str(&r["name"]), r["allow"].as_array().map(|a| a.iter().map(|x| str_of(x).to_string()).collect::<Vec<_>>().join(",")).unwrap_or_else(|| "-".into()))) },
        })
    }

    /// The window chrome.
    pub fn chrome(v: &Value) -> Value {
        json!([
            format!("title: {}", str_of(&v["title"])),
            format!(
                "newSpace: {} {}",
                str_of(&v["newSpaceLabel"]),
                str_of(&v["newSpaceShortcut"])
            ),
            format!("search: {}", str_of(&v["searchPlaceholder"])),
            format!("keyvault: {}", str_of(&v["keyvaultTitle"])),
            format!("account: {}", str_of(&v["account"])),
            format!("signIn: {}", opt_str(&v["signInLabel"])),
            format!(
                "settings: {} {}",
                str_of(&v["settingsLabel"]),
                str_of(&v["settingsShortcut"])
            ),
            format!(
                "empty: {} / {}",
                str_of(&v["emptyTitle"]),
                str_of(&v["emptyAction"])
            ),
            format!("volume: {}", opt_str(&v["volumeLabel"])),
        ])
    }

    /// The menu bar item's menu.
    pub fn menu(v: &Value) -> Value {
        Value::Array(
            v.as_array()
                .map(|a| {
                    a.iter()
                        .map(|m| {
                            Value::String(if m["id"] == "separator" {
                                "---".into()
                            } else {
                                format!(
                                    "{}: {}{}{}",
                                    str_of(&m["id"]),
                                    str_of(&m["label"]),
                                    m["shortcut"]
                                        .as_str()
                                        .map(|s| format!(" {s}"))
                                        .unwrap_or_default(),
                                    if m["enabled"] == true {
                                        ""
                                    } else {
                                        " (disabled)"
                                    }
                                )
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    /// The Settings page, one string per section header and row.
    pub fn settings(v: &Value) -> Value {
        let mut out = vec![Value::String(format!("= {}", str_of(&v["title"])))];
        for sec in v["sections"].as_array().into_iter().flatten() {
            out.push(Value::String(format!(
                "# {} ({}){}{}",
                str_of(&sec["title"]),
                str_of(&sec["id"]),
                sec["button"]
                    .as_str()
                    .map(|b| format!(" [{b}]"))
                    .unwrap_or_default(),
                if sec["button"].is_null() || sec["buttonEnabled"] == true {
                    ""
                } else {
                    " (disabled)"
                }
            )));
            for r in sec["rows"].as_array().into_iter().flatten() {
                out.push(Value::String(setting_row(r)));
            }
        }
        Value::Array(out)
    }

    /// One Settings row: `- id kind: label = value ~placeholder [button] {options} (disabled) ?help <link>`.
    pub fn setting_row(r: &Value) -> String {
        let options = r["options"]
            .as_array()
            .filter(|o| !o.is_empty())
            .map(|o| {
                format!(
                    " {{{}}}",
                    o.iter()
                        .map(|x| format!(
                            "{}{}",
                            if x["active"] == true { "*" } else { "" },
                            str_of(&x["label"])
                        ))
                        .collect::<Vec<_>>()
                        .join("|")
                )
            })
            .unwrap_or_default();
        format!(
            "- {} {}: {}{}{}{}{}{}{}{}",
            str_of(&r["id"]),
            str_of(&r["kind"]),
            str_of(&r["label"]),
            r["value"]
                .as_str()
                .map(|x| format!(" = {x}"))
                .unwrap_or_default(),
            r["placeholder"]
                .as_str()
                .map(|x| format!(" ~{x}"))
                .unwrap_or_default(),
            r["button"]
                .as_str()
                .map(|x| format!(" [{x}]"))
                .unwrap_or_default(),
            options,
            if r["enabled"] == true {
                ""
            } else {
                " (disabled)"
            },
            r["help"]
                .as_str()
                .map(|x| format!(" ?{x}"))
                .unwrap_or_default(),
            r["linkUrl"]
                .as_str()
                .map(|x| format!(" <{} {x}>", str_of(&r["linkLabel"])))
                .unwrap_or_default()
        )
    }

    /// Coding agents' Settings rows.
    pub fn agent_rows(v: &Value) -> Value {
        Value::Array(
            v.as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| {
                            Value::String(format!(
                                "{}: {}{}{} ({}) {}/{}",
                                str_of(&r["agent"]),
                                str_of(&r["name"]),
                                if r["installed"] == true {
                                    " installed"
                                } else {
                                    ""
                                },
                                if r["configured"] == true {
                                    " configured"
                                } else {
                                    ""
                                },
                                str_of(&r["detail"]),
                                r["skillsInstalled"],
                                r["skillsTotal"]
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    /// What a setup did for one agent.
    pub fn agent_summary(v: &Value) -> Value {
        Value::String(format!(
            "{} | {} | {}",
            str_of(&v["line"]),
            str_of(&v["text"]),
            v["failed"]
                .as_array()
                .map(|a| a
                    .iter()
                    .map(|x| str_of(x).to_string())
                    .collect::<Vec<_>>()
                    .join("; "))
                .unwrap_or_default()
        ))
    }

    /// When the presentation previews are sampled (ms into the loop).
    pub const PREVIEW_TIMES: [u32; 9] = [0, 850, 1350, 1550, 1700, 2400, 2750, 3150, 3900];

    /// A presentation preview's scene: what the miniature shows.
    pub fn preview_scene(p: &Value) -> String {
        let int = |v: &Value| v.as_f64().unwrap_or(0.0).round() as i64;
        let n = &p["notch"];
        let m = &p["menu"];
        if !n.is_null() {
            let tiles = n["view"]["tiles"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|t| str_of(&t["name"]))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            format!(
                "notch: tiles {tiles}; tab {} {}; {}x{} loop {}",
                str_of(&n["view"]["tab"]["count"]),
                str_of(&n["view"]["tab"]["word"]),
                int(&p["width"]),
                int(&p["height"]),
                int(&p["loopMs"])
            )
        } else {
            let rows = m["rows"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| {
                            let i = &r["item"];
                            let label = if i["id"] == "separator" {
                                "-".to_string()
                            } else {
                                str_of(&i["label"]).to_string()
                            };
                            match i["shortcut"].as_str() {
                                Some(k) => format!("{label} {k}"),
                                None => label,
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .unwrap_or_default();
            format!(
                "menu: {rows}; {}x{} loop {}",
                int(&p["width"]),
                int(&p["height"]),
                int(&p["loopMs"])
            )
        }
    }

    /// The background computer-use card's scene.
    pub fn driver_scene(p: &Value) -> String {
        let int = |v: &Value| v.as_f64().unwrap_or(0.0).round() as i64;
        let r = |v: &Value| {
            format!(
                "{},{} {}x{}",
                int(&v["x"]),
                int(&v["y"]),
                int(&v["width"]),
                int(&v["height"])
            )
        };
        let count = |k: &str| p[k].as_array().map_or(0, Vec::len);
        format!(
            "back {}; front {}; boxes {}; lines {} (selects {}); agent {} {} points, {} rays; {}x{} loop {}",
            r(&p["back"]["frame"]),
            r(&p["front"]["frame"]),
            count("checkboxes"),
            count("lines"),
            p["selectedLine"],
            str_of(&p["agentFill"]),
            count("agentPointer"),
            count("agentRays"),
            int(&p["width"]),
            int(&p["height"]),
            int(&p["loopMs"])
        )
    }

    /// One frame of the background computer-use card, in tenths of a point
    /// and thousandths.
    pub fn driver_frame(f: &Value) -> String {
        let n = |v: &Value, k: f64| (v.as_f64().unwrap_or(0.0) * k).round() as i64;
        let checked = f["checked"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| n(c, 1000.0).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        format!(
            "user {},{} pressed {} selection {}; agent {},{} pressed {} ripple {}; checked {}",
            n(&f["pointer"]["x"], 10.0),
            n(&f["pointer"]["y"], 10.0),
            f["pressed"],
            n(&f["selection"], 1000.0),
            n(&f["agent"]["x"], 10.0),
            n(&f["agent"]["y"], 10.0),
            f["agentPressed"],
            n(&f["ripple"], 1000.0),
            checked
        )
    }

    /// One preview frame, in tenths of a point and thousandths.
    pub fn preview_frame(f: &Value) -> String {
        let n = |v: &Value, k: f64| (v.as_f64().unwrap_or(0.0) * k).round() as i64;
        format!(
            "pointer {},{} open {} content {} tab {} hover {} pressed {} active {} highlighted {}",
            n(&f["pointer"]["x"], 10.0),
            n(&f["pointer"]["y"], 10.0),
            n(&f["open"], 1000.0),
            n(&f["content"], 1000.0),
            n(&f["tab"], 1000.0),
            n(&f["hover"], 1000.0),
            f["pressed"],
            f["active"],
            f["highlighted"]
                .as_u64()
                .map_or("-".to_string(), |i| i.to_string()),
        )
    }

    /// A first-run page (Done adds `launchAtLogin`).
    pub fn onboarding(v: &Value) -> Value {
        let mut out = onboarding_page(v);
        let cb = &v["launchAtLogin"];
        if cb.is_object() {
            out["launchAtLogin"] = Value::String(checkbox(cb));
        }
        out
    }

    /// A checkbox: `[x] label ~note`.
    pub fn checkbox(c: &Value) -> String {
        format!(
            "[{}] {} ~{}",
            if c["checked"] == true { "x" } else { " " },
            str_of(&c["label"]),
            str_of(&c["note"])
        )
    }

    fn onboarding_page(v: &Value) -> Value {
        json!({
            "step": v["step"],
            "title": v["title"],
            "lede": v["lede"],
            "primary": v["primaryLabel"],
            "canSkip": v["canSkip"],
            "canBack": v["canBack"],
            "showMark": v["showMark"],
            "dots": v["dots"].as_array().map(|a| a.iter().map(|d| format!("{}{}", if d["current"] == true { "*" } else { "" }, str_of(&d["label"]))).collect::<Vec<_>>().join(",")).unwrap_or_default(),
            "summary": facts(&v["summary"]),
            "choices": v["choices"].as_array().map(|a| a.iter().map(|c| Value::String(format!("{}: {}{}", str_of(&c["mode"]), str_of(&c["label"]),
                if c["preselected"] == true { " (preselected)" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
            "presentations": v["presentations"].as_array().map(|a| a.iter().map(|c| Value::String(format!("{} [{}]{}", str_of(&c["title"]), str_of(&c["id"]),
                if c["selected"] == true { " (selected)" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
            "prompts": v["prompts"],
            "notice": v["notice"],
            "noticeLink": if v["noticeLinkUrl"].is_null() { Value::Null } else { Value::String(format!("{} {}", str_of(&v["noticeLinkLabel"]), str_of(&v["noticeLinkUrl"]))) },
            // `label [x] (disabled) ~help`.
            "usage": if v["usage"].is_null() { Value::Null } else { Value::String(format!("{} [{}]{}{}", str_of(&v["usage"]["label"]),
                if v["usage"]["on"] == true { "x" } else { " " }, if v["usage"]["enabled"] == true { "" } else { " (disabled)" },
                v["usage"]["help"].as_str().map(|h| format!(" ~{h}")).unwrap_or_default())) },
            "drive": drive_card(&v["drive"]),
            "driveStorage": drive_storage(&v["drive"]),
        })
    }

    /// The Cua Volume page's card: `label [x] (disabled) (busy) ~note !error <settings url>`.
    pub fn drive_card(d: &Value) -> Value {
        if d.is_null() {
            return Value::Null;
        }
        Value::String(format!(
            "{} [{}]{}{}{}{}{} | {}",
            str_of(&d["label"]),
            if d["checked"] == true { "x" } else { " " },
            if d["enabled"] == true {
                ""
            } else {
                " (disabled)"
            },
            if d["busy"] == true { " (busy)" } else { "" },
            d["note"]
                .as_str()
                .map(|x| format!(" ~{x}"))
                .unwrap_or_default(),
            d["error"]
                .as_str()
                .map(|x| format!(" !{x}"))
                .unwrap_or_default(),
            d["settingsUrl"]
                .as_str()
                .map(|u| format!(" <{} {u}>", str_of(&d["settingsLabel"])))
                .unwrap_or_default(),
            str_of(&d["imageLabel"]),
        ))
    }

    /// The Cua Volume page's storage choice: the options, the bucket's rows,
    /// the note and whether Continue can be pressed.
    pub fn drive_storage(d: &Value) -> Value {
        if d.is_null() || d["storageTitle"].is_null() {
            return Value::Null;
        }
        let options = d["storageOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| {
                format!(
                    "{}{}",
                    if o["active"] == true { "*" } else { "" },
                    str_of(&o["label"])
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let mut out = vec![Value::String(format!(
            "{} {{{options}}}",
            str_of(&d["storageTitle"])
        ))];
        for r in d["storageRows"].as_array().into_iter().flatten() {
            out.push(Value::String(setting_row(r)));
        }
        if let Some(n) = d["storageNote"].as_str() {
            out.push(Value::String(format!("~{n}")));
        }
        for k in ["storedIn", "mountedAt"] {
            if let Some(t) = d[k].as_str() {
                out.push(Value::String(format!(
                    "{t} ({})",
                    str_of(
                        &d[if k == "storedIn" {
                            "storedPath"
                        } else {
                            "mountedPath"
                        }]
                    )
                )));
            }
        }
        out.push(Value::String(format!("continue {}", d["canContinue"])));
        Value::Array(out)
    }

    /// The Cua Volume page's miniature: what it shows.
    pub fn drive_scene(p: &Value) -> String {
        let int = |v: &Value| v.as_f64().unwrap_or(0.0).round() as i64;
        let r = |v: &Value| {
            format!(
                "{},{} {}x{}",
                int(&v["x"]),
                int(&v["y"]),
                int(&v["width"]),
                int(&v["height"])
            )
        };
        let count = |k: &str| p[k].as_array().map_or(0, Vec::len);
        format!(
            "space {}; finder {}; sidebar {}; places {}; volume {} {} at {}; files {}->{}; {}x{} loop {}",
            r(&p["space"]["frame"]),
            r(&p["finder"]["frame"]),
            r(&p["sidebar"]),
            count("places"),
            str_of(&p["volumeLabel"]),
            r(&p["volume"]),
            int(&p["volumeLabelX"]),
            count("sourceIcons"),
            count("destIcons"),
            int(&p["width"]),
            int(&p["height"]),
            int(&p["loopMs"])
        )
    }

    /// One frame of the Cua Volume page's miniature, in tenths of a point
    /// and thousandths.
    pub fn drive_frame(f: &Value) -> String {
        let n = |v: &Value, k: f64| (v.as_f64().unwrap_or(0.0) * k).round() as i64;
        let arrived = f["arrived"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| n(c, 1000.0).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let flight = if f["flight"].is_null() {
            "-".to_string()
        } else {
            format!(
                "{},{}",
                n(&f["flight"]["x"], 10.0),
                n(&f["flight"]["y"], 10.0)
            )
        };
        format!(
            "volume {} flight {flight} arrived {arrived}",
            n(&f["volume"], 1000.0)
        )
    }

    /// After each picker event.
    pub fn picker(
        state: &Value,
        sections: &Value,
        review: &Value,
        can_plan: &Value,
        progress: f64,
        sensitive: &Value,
        plan_sensitive: &Value,
    ) -> Value {
        json!({
            "step": state["step"],
            "selectedId": state["selectedId"],
            "entry": state["entry"]["id"],
            "move": state["move"],
            "canPlan": can_plan,
            "sections": sections.as_array().map(|a| a.iter().map(|sec| Value::String(format!("{}: {}",
                sec["title"].as_str().unwrap_or(""),
                sec["entries"].as_array().map(|e| e.iter().map(|x| x["name"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default()))).collect::<Vec<_>>()).unwrap_or_default(),
            "review": if review.is_null() { Value::Null } else { json!({
                "title": review["title"],
                "items": review["items"].as_array().map(|a| a.iter().map(|c| Value::String(format!("{}:{}{}",
                    c["kind"].as_str().unwrap_or(""), c["label"].as_str().unwrap_or(""),
                    if c["sensitive"] == true { " (secret)" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
                "needsAcknowledgement": review["needsAcknowledgement"],
                "needsRelayPlaintextAcknowledgement": review["needsRelayPlaintextAcknowledgement"],
                "canConfirm": review["canConfirm"],
                "leavesText": review["leavesText"],
                "warnings": review["warnings"],
            }) },
            "permille": (progress * 1000.0).round() as i64,
            "error": state["error"],
            "report": state["report"]["appId"],
            "sensitive": sensitive.as_array().map(|a| a.iter().map(|o| Value::String(format!("{}:{}:{}:{}",
                o["group"].as_str().unwrap_or(""), o["label"].as_str().unwrap_or(""),
                o["detail"].as_str().unwrap_or(""), if o["checked"] == true { "on" } else { "off" }))).collect::<Vec<_>>()).unwrap_or_default(),
            "planSensitive": plan_sensitive,
        })
    }

    /// After each notch event.
    pub fn notch(state: &Value, view: &Value, effects: &Value) -> Value {
        json!({
            "open": state["open"],
            "phase": view["phase"],
            "prompt": view["prompt"],
            "tiles": view["tiles"].as_array().map(|a| a.iter().map(|t| Value::String(format!("{} [{}]{}{}",
                t["name"].as_str().unwrap_or(""),
                t["symbol"].as_str().unwrap_or(""),
                if t["dropTarget"] == true { "" } else { " (no drop)" },
                if t["targeted"] == true { " (targeted)" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
            "dropMode": view["dropMode"],
            "label": view["label"],
            "tab": if view["showTab"] == true { json!([view["tab"]["count"], view["tab"]["word"]]) } else { Value::Null },
            "header": if view["header"].is_null() { Value::Null } else { json!({
                "query": view["header"]["query"],
                "matches": view["header"]["matchCount"],
                "buttons": view["header"]["buttons"].as_array().map(|a| a.iter().map(|b| Value::String(format!("{} [{}]",
                    b["label"].as_str().unwrap_or(""), b["symbol"].as_str().unwrap_or("")))).collect::<Vec<_>>()).unwrap_or_default(),
            }) },
            "empty": view["empty"],
            "hidden": view["hidden"],
            "activity": if view["activity"].is_null() { Value::Null } else { Value::String(format!("{}: {}{}{}",
                view["activity"]["kind"].as_str().unwrap_or(""),
                view["activity"]["label"].as_str().unwrap_or(""),
                view["activity"]["symbol"].as_str().map(|s| format!(" [{s}]")).unwrap_or_default(),
                view["activity"]["permille"].as_u64().map(|p| format!(" {p}/1000")).unwrap_or_default())) },
            "cue": view["hoverCue"],
            "permission": view["permission"]["text"],
            "effects": effects.as_array().map(|a| a.iter().map(effect).collect::<Vec<_>>()).unwrap_or_default(),
        })
    }

    /// A notch layout: every rectangle as `[x, y, width, height]`.
    pub fn notch_layout(l: &Value) -> Value {
        let r = |k: &str| {
            let v = &l[k];
            json!([v["x"], v["y"], v["width"], v["height"]])
        };
        json!({
            "hasNotch": l["hasNotch"],
            "notchStyle": l["notchStyle"],
            "notch": r("notch"),
            "closed": r("closedFrame"),
            "open": r("openFrame"),
            "prompt": r("promptFrame"),
            "tab": r("tabFrame"),
            "stage": r("stageFrame"),
        })
    }

    fn effect(e: &Value) -> Value {
        let kind = e["kind"].as_str().unwrap_or("");
        Value::String(match kind {
            "start-dwell" | "start-close" => format!("{kind}:{}", e["ms"]),
            "drag" => {
                let inner = &e["effect"];
                match inner["kind"].as_str().unwrap_or("") {
                    "capture" => format!("capture:{}", inner["windowId"]),
                    "commit" => format!("commit:{}", inner["spaceId"].as_str().unwrap_or("")),
                    other => other.to_string(),
                }
            }
            other => other.to_string(),
        })
    }

    /// The Keyvault sidebar.
    pub fn kv_sidebar(v: &Value) -> Value {
        json!({
            "categories": v["categories"].as_array().map(|a| a.iter().map(|c| Value::String(match c["count"].as_u64() {
                Some(n) => format!("{}:{n}", c["title"].as_str().unwrap_or("")),
                None => c["title"].as_str().unwrap_or("").to_string(),
            })).collect::<Vec<_>>()).unwrap_or_default(),
            "apps": v["apps"].as_array().map(|a| a.iter().map(|x| Value::String(format!("{} ({}){}",
                x["title"].as_str().unwrap_or(""), x["items"], if x["waiting"] == true { " waiting" } else { "" }))).collect::<Vec<_>>()).unwrap_or_default(),
        })
    }

    /// A Keyvault list (the panes other than the vault list).
    pub fn kv_list(v: &Value) -> Value {
        let text = |x: &Value| x.as_str().unwrap_or("").to_string();
        json!({
            "title": v["title"],
            "vault": v["vault"],
            "pending": v["pending"].as_array().map(|a| a.iter().map(|p| Value::String(format!("{} ({}) {} for {}",
                text(&p["caller"]), text(&p["badge"]["text"]), text(&p["summary"]), text(&p["wants"])))).collect::<Vec<_>>()).unwrap_or_default(),
            "access": v["access"].as_array().map(|a| a.iter().map(|x| Value::String(format!("{} / {} [{}]",
                text(&x["text"]), text(&x["detail"]), text(&x["actionLabel"])))).collect::<Vec<_>>()).unwrap_or_default(),
            "recent": v["recent"].as_array().map(|a| a.iter().map(|x| Value::String(format!("{} {} ({}) {}",
                text(&x["decision"]["verb"]), text(&x["decision"]["what"]), text(&x["decision"]["tone"]), text(&x["age"])))).collect::<Vec<_>>()).unwrap_or_default(),
            "empty": v["emptyText"],
        })
    }

    /// The vault list: each app with its sites and files, one line each,
    /// the selection and the batch bar.
    pub fn kv_vault(v: &Value) -> Value {
        let text = |x: &Value| x.as_str().unwrap_or("").to_string();
        let mark = |x: &Value| match x.as_str().unwrap_or("") {
            "on" => "[x]",
            "mixed" => "[-]",
            _ => "[ ]",
        };
        let row = |r: &Value| {
            format!(
                "{} {} {} | {} | {} | {}{}",
                if r["selected"] == true { "[x]" } else { "[ ]" },
                text(&r["kindLabel"]),
                text(&r["title"]),
                text(&r["subtitle"]),
                text(&r["lockSymbol"]),
                text(&r["updated"]),
                if r["identityProvider"] == true {
                    " | always asks"
                } else {
                    ""
                }
            )
        };
        let lines: Vec<Value> = v["apps"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|a| {
                let mut out = vec![Value::String(format!(
                    "{} {} {} | {} | {} | {}",
                    mark(&a["selected"]),
                    text(&a["name"]),
                    text(&a["providerId"]),
                    text(&a["summary"]),
                    text(&a["lock"]),
                    text(&a["updated"])
                ))];
                for s in a["sites"].as_array().into_iter().flatten() {
                    out.push(Value::String(format!(
                        "  {} {} | {} | {} | {}{}",
                        mark(&s["selected"]),
                        text(&s["site"]),
                        text(&s["counts"]),
                        text(&s["lock"]),
                        text(&s["updated"]),
                        if s["open"] == true { " | open" } else { "" }
                    )));
                    for r in s["rows"].as_array().into_iter().flatten() {
                        out.push(Value::String(format!("    {}", row(r))));
                    }
                }
                if !a["files"].is_null() {
                    let f = &a["files"];
                    out.push(Value::String(format!(
                        "  {} files ({}) | {}{}",
                        mark(&f["selected"]),
                        f["count"],
                        text(&f["lock"]),
                        if f["open"] == true { " | open" } else { "" }
                    )));
                    for r in f["rows"].as_array().into_iter().flatten() {
                        out.push(Value::String(format!("    {}", row(r))));
                    }
                }
                out
            })
            .collect();
        let sel = &v["selection"];
        json!({
            "shown": format!("{}/{}", v["shown"], v["total"]),
            "lines": lines,
            "empty": v["emptyText"],
            "selection": format!("{} (unlock {} / lock {} / always ask {})",
                text(&sel["title"]),
                sel["unlockIds"].as_array().map(|a| a.len()).unwrap_or(0),
                sel["lockIds"].as_array().map(|a| a.len()).unwrap_or(0),
                sel["alwaysAsk"]),
            "canUnlock": sel["canUnlock"],
            "canLock": sel["canLock"],
        })
    }

    /// The unlock prompt (`null`: skipped).
    pub fn kv_unlock_prompt(v: &Value) -> Value {
        if v.is_null() {
            return Value::Null;
        }
        json!([
            v["title"],
            v["message"],
            v["subject"],
            format!(
                "{} / {} / {}",
                str_of(&v["deny"]),
                str_of(&v["allow"]),
                str_of(&v["neverAsk"])
            ),
        ])
    }

    /// The delete confirmation.
    pub fn kv_delete_confirm(v: &Value) -> Value {
        json!([
            v["title"],
            v["message"],
            format!("{} / {}", str_of(&v["confirm"]), str_of(&v["cancel"]))
        ])
    }

    /// The approval sheet.
    pub fn approval(v: &Value) -> Value {
        json!({
            "title": v["title"],
            "badge": v["badge"]["text"],
            "rows": v["rows"].as_array().map(|a| a.iter().map(|r| Value::String(format!("{}{} {}",
                if r["selected"] == true { "[x] " } else { "[ ] " },
                r["title"].as_str().unwrap_or(""), r["account"].as_str().unwrap_or("")))).collect::<Vec<_>>()).unwrap_or_default(),
            "canApprove": v["canApprove"],
            "approveLabel": v["approveLabel"],
            "blockedReason": v["blockedReason"],
            "claims": v["claims"],
        })
    }

    /// The page chrome.
    pub fn kv_page(v: &Value) -> Value {
        json!({
            "ready": v["ready"],
            "killSwitchVisible": v["killSwitchVisible"],
            "disabled": v["disabled"],
            "disabledBanner": v["disabledBanner"],
            "logStatus": v["logStatus"],
            "revokeAll": v["revokeAll"],
            "pendingCount": v["pendingCount"],
            "protection": facts(&v["protection"]),
        })
    }

    /// The setup or unlock form, as lines (`null`: none).
    pub fn kv_form(v: &Value) -> Value {
        if v.is_null() {
            return Value::Null;
        }
        json!([
            format!("{} with {}", str_of(&v["mode"]), str_of(&v["method"])),
            format!("help: {}", str_of(&v["help"])),
            format!("passphrase: {}", opt_str(&v["passphraseLabel"])),
            format!("confirm: {}", opt_str(&v["confirmLabel"])),
            format!("submit: {}", str_of(&v["submitLabel"])),
        ])
    }

    /// The Devices page.
    pub fn devices(v: &Value) -> Value {
        let b = &v["banner"];
        let t = &v["thisDevice"];
        let l = &v["labels"];
        let lines = |key: &str| -> Vec<Value> {
            v[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    Value::String(format!(
                        "{} {}{}",
                        a["ts"],
                        str_of(&a["text"]),
                        if a["notable"] == true {
                            " (notable)"
                        } else {
                            ""
                        }
                    ))
                })
                .collect()
        };
        json!({
            "banner": if b.is_null() { Value::Null } else { Value::String(format!(
                "{}: {} [{}]", str_of(&b["tone"]), str_of(&b["text"]), opt_str(&b["actionLabel"]))) },
            "enrolled": v["enrolled"],
            "thisDevice": format!("{}: {} @{} name={} [{}]", str_of(&t["kind"]), str_of(&t["title"]),
                t["at"], opt_str(&t["name"]), opt_str(&t["actionLabel"])),
            "rows": v["rows"].as_array().into_iter().flatten().map(|r| Value::String(format!(
                "{} | {} | seen {} | {}{}{}",
                str_of(&r["title"]),
                str_of(&r["detail"]),
                r["lastSeen"],
                r["actions"].as_array().into_iter().flatten().map(|a| str_of(a).to_string()).collect::<Vec<_>>().join(","),
                if r["current"] == true { " | current" } else { "" },
                if r["revokeConfirm"].is_object() {
                    format!(" | {} / {}", str_of(&r["revokeConfirm"]["title"]), str_of(&r["revokeConfirm"]["message"]))
                } else { String::new() },
            ))).collect::<Vec<_>>(),
            "approvals": v["approvals"].as_array().into_iter().flatten().map(|a| Value::String(format!(
                "{}{}: {} | {} / {}", str_of(&a["deviceId"]), if a["expired"] == true { " (expired)" } else { "" },
                str_of(&a["text"]), str_of(&a["notifyTitle"]), str_of(&a["notifyBody"])))).collect::<Vec<_>>(),
            "recent": lines("recent"),
            "activity": lines("activity"),
            "unconfirmedMachines": v["unconfirmedMachines"].as_array().into_iter().flatten().map(|m| Value::String(format!(
                "{}: {} / {}", str_of(&m["title"]), str_of(&m["confirm"]["title"]), str_of(&m["confirm"]["message"])
            ))).collect::<Vec<_>>(),
            "labels": format!("{} / {} / {} / {} ({}) / {} / {} {} {} {} / {} {} {} / {} {}",
                str_of(&l["title"]), str_of(&l["thisDevice"]), str_of(&l["devices"]), str_of(&l["recent"]),
                str_of(&l["recentEmpty"]), str_of(&l["lastSeen"]), str_of(&l["approve"]), str_of(&l["deny"]),
                str_of(&l["rename"]), str_of(&l["revoke"]), str_of(&l["renameTitle"]), str_of(&l["renameConfirm"]),
                str_of(&l["cancel"]), str_of(&l["newMachines"]), str_of(&l["confirmMachine"])),
        })
    }

    /// `text | trailing | action | secondary | on` per line.
    pub fn lines(v: &Value) -> Vec<Value> {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|l| {
                Value::String(format!(
                    "{} | {} | {} | {} | {}",
                    str_of(&l["text"]),
                    str_of(&l["trailing"]),
                    opt_str(&l["actionLabel"]),
                    opt_str(&l["secondaryLabel"]),
                    match l["on"].as_bool() {
                        Some(true) => "on",
                        Some(false) => "off",
                        None => "-",
                    }
                ))
            })
            .collect()
    }

    /// The Agents page.
    pub fn agents(v: &Value) -> Value {
        let d = &v["detail"];
        let detail = if d.is_null() {
            Value::Null
        } else {
            let f = &d["form"];
            json!({
                "name": d["name"],
                "subtitle": d["subtitle"],
                "tabs": d["tabs"].as_array().into_iter().flatten().map(|t| format!("{}{}",
                    str_of(&t["label"]), if t["selected"] == true { "*" } else { "" })).collect::<Vec<_>>().join(", "),
                "memory": lines(&d["memory"]),
                "memoryEmpty": d["memoryEmpty"],
                "file": if d["file"].is_null() { Value::Null } else { json!({
                    "path": d["file"]["path"], "text": d["file"]["text"],
                    "versions": lines(&d["file"]["versions"]), "close": d["file"]["closeLabel"] }) },
                "routines": lines(&d["routines"]),
                "routinesEmpty": d["routinesEmpty"],
                "form": format!("{} [{}] [{}] {} {} {} {}", str_of(&d["addRoutineLabel"]), str_of(&f["title"]),
                    str_of(&f["prompt"]), str_of(&f["schedule"]), f["minutes"].as_u64().unwrap_or(0),
                    str_of(&f["time"]), str_of(&f["weekday"])),
                "schedules": lines(&d["schedules"]),
                "canAdd": d["canAddRoutine"],
                "access": lines(&d["access"]),
                "accessEmpty": d["accessEmpty"],
                "allowThisMachine": d["allowThisMachineLabel"],
                "audit": lines(&d["audit"]),
                "error": d["error"],
            })
        };
        json!({
            "title": v["title"],
            "rows": v["rows"].as_array().into_iter().flatten().map(|r| Value::String(format!(
                "{} | {} | {} | {}{}", str_of(&r["name"]), str_of(&r["detail"]), str_of(&r["state"]),
                str_of(&r["actionLabel"]), if r["selected"] == true { " (selected)" } else { "" }))).collect::<Vec<_>>(),
            "empty": v["emptyText"],
            "detail": detail,
            "busy": v["busy"],
            "error": v["error"],
            "request": v["requestText"],
        })
    }

    /// The Drive page.
    pub fn drive(v: &Value) -> Value {
        json!({
            "title": v["title"],
            "requests": format!("{}: {}", str_of(&v["requestsTitle"]), lines(&v["requests"]).len()),
            "requestRows": lines(&v["requests"]),
            "grants": format!("{}: {}", str_of(&v["grantsTitle"]), str_of(&v["grantsEmpty"])),
            "grantRows": lines(&v["grants"]),
            "busy": v["busy"],
            "error": v["error"],
            "request": v["requestText"],
            "open": if v["openLabel"].is_null() { Value::Null } else {
                Value::String(format!("{} {}", str_of(&v["openLabel"]), opt_str(&v["mountPath"]))) },
            "mountLine": v["mountLine"],
            "devices": format!("{}: {}", str_of(&v["devicesTitle"]), lines(&v["devices"]).len()),
            "deviceRows": lines(&v["devices"]),
            "syncNote": if v["syncNote"].is_null() { Value::Null } else {
                Value::String(format!("{}{}", if v["syncError"] == true { "!" } else { "" }, str_of(&v["syncNote"]))) },
            "conflicts": v["conflicts"].as_array().into_iter().flatten().map(|c| format!("{} | {} | {} {} | {}",
                str_of(&c["text"]), str_of(&c["trailing"]), opt_str(&c["openLabel"]), opt_str(&c["reveal"]),
                str_of(&c["resolveLabel"]))).collect::<Vec<_>>(),
        })
    }

    /// A notifications poll: what is posted, the marker, and the list.
    pub fn notifications(plan: &Value, view: &Value) -> Value {
        json!({
            "post": plan["post"].as_array().into_iter().flatten().map(|n| format!("{} | {} | {}",
                str_of(&n["id"]), str_of(&n["title"]), str_of(&n["body"]))).collect::<Vec<_>>(),
            "seenMs": plan["seenMs"],
            "title": view["title"],
            "rows": lines(&view["rows"]),
            "empty": view["emptyText"],
            "unread": view["unread"],
            "markAll": view["markAllLabel"],
        })
    }

    /// Settings, About (and the Sparkle channels its choice allows).
    pub fn about(v: &Value, channels: &Value) -> Value {
        let check = |on: &Value, label: &Value, enabled: bool| {
            format!(
                "[{}] {}{}",
                if on == true { "x" } else { " " },
                str_of(label),
                if enabled { "" } else { " (disabled)" }
            )
        };
        let u = &v["updates"];
        let updates = if u.is_object() {
            json!({
                "autoCheck": check(&u["autoCheck"], &u["autoCheckLabel"], true),
                "autoInstall": check(&u["autoInstall"], &u["autoInstallLabel"], u["autoInstallEnabled"] == true),
                "channel": format!("{} {}", str_of(&u["channelLabel"]), u["channels"].as_array().into_iter().flatten()
                    .map(|o| format!("{}{}", str_of(&o["label"]), if o["active"] == true { "*" } else { "" }))
                    .collect::<Vec<_>>().join(" | ")),
                "help": u["channelHelp"],
                "check": format!("{} {}", str_of(&u["checkLabel"]), if u["checkEnabled"] == true { "on" } else { "off" }),
                "lastCheck": u["lastCheck"],
                "sparkleChannels": channels,
            })
        } else {
            Value::Null
        };
        json!({
            "title": v["title"],
            "version": v["versionLine"],
            "links": v["links"].as_array().into_iter().flatten().map(|l| format!("{} {} -> {}",
                str_of(&l["id"]), str_of(&l["label"]), l["url"].as_str().unwrap_or("(bundled notices)"))).collect::<Vec<_>>(),
            "copyright": v["copyright"],
            "updates": updates,
        })
    }

    /// The New Space wizard in "Your cloud": tiles with their lines, what
    /// is greyed out and why, the cloud menu, and the Resources facts.
    pub fn your_cloud(view: &Value) -> Value {
        let tiles = |key: &str, only_off: bool| -> Vec<Value> {
            view[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| !only_off || t["enabled"] != true)
                .map(|t| {
                    Value::String(format!(
                        "{} | {} | {} | {}",
                        str_of(&t["id"]),
                        str_of(&t["title"]),
                        str_of(&t["detail"]),
                        if t["enabled"] == true { "on" } else { "off" }
                    ))
                })
                .collect()
        };
        json!({
            "step": view["step"],
            "canContinue": view["canContinue"],
            "placement": view["plan"]["placement"],
            "cloud": view["cloud"],
            "placements": placement_menu(view),
            "systemsOff": tiles("osTiles", true),
            "kindsOff": tiles("kindTiles", true),
            "placementError": view["placementError"],
            "fields": view["fields"].as_array().into_iter().flatten().map(|f| f["id"].clone()).collect::<Vec<_>>(),
            "price": view["price"],
            "resourceFacts": view["resourceFacts"].as_array().into_iter().flatten()
                .map(|f| Value::String(format!("{}: {}", str_of(&f["label"]), str_of(&f["value"])))).collect::<Vec<_>>(),
            "summary": view["summary"].as_array().into_iter().flatten()
                .map(|f| Value::String(format!("{}: {}", str_of(&f["label"]), str_of(&f["value"])))).collect::<Vec<_>>(),
        })
    }

    /// The "Run on" menu: `id | label | detail | on` (`off` when disabled,
    /// ` *` when chosen), each group after a `---` line.
    pub fn placement_menu(view: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        let mut group = None;
        for o in view["placements"].as_array().into_iter().flatten() {
            if group.is_some() && group != o["group"].as_str() {
                out.push(Value::String("---".into()));
            }
            group = o["group"].as_str();
            out.push(Value::String(format!(
                "{} | {} | {} | {}{}",
                str_of(&o["id"]),
                str_of(&o["label"]),
                str_of(&o["detail"]),
                if o["enabled"] == true { "on" } else { "off" },
                if o["selected"] == true { " *" } else { "" }
            )));
        }
        out
    }

    /// The "Connect a cloud" sheet.
    pub fn cloud_connect(v: &Value) -> Value {
        let field = |f: &Value| -> Value {
            if f.is_null() {
                Value::Null
            } else {
                Value::String(format!(
                    "{} [{}] = {}",
                    str_of(&f["label"]),
                    str_of(&f["placeholder"]),
                    str_of(&f["value"])
                ))
            }
        };
        json!({
            "title": v["title"],
            "rows": v["rows"].as_array().into_iter().flatten().map(|r| Value::String(format!(
                "{} | {} | {}{}{}", str_of(&r["id"]), str_of(&r["title"]), str_of(&r["detail"]),
                if r["found"] == true { " | found" } else { "" },
                if r["selected"] == true { " | selected" } else { "" }))).collect::<Vec<_>>(),
            "field": field(&v["field"]),
            "profile": field(&v["profileField"]),
            "checks": v["checks"].as_array().into_iter().flatten().map(|c| Value::String(format!(
                "{} {}", if c["ok"] == true { "ok" } else { "failed" }, str_of(&c["text"])))).collect::<Vec<_>>(),
            "result": v["result"],
            "touches": v["touches"].as_array().into_iter().flatten().cloned().collect::<Vec<_>>(),
            "buttons": format!("{} {} ?{} / {} {} / {} / {} {}", str_of(&v["testLabel"]),
                if v["canTest"] == true { "on" } else { "off" }, str_of(&v["testHelp"]),
                str_of(&v["connectLabel"]), if v["canConnect"] == true { "on" } else { "off" },
                str_of(&v["cancelLabel"]), str_of(&v["makeDefaultLabel"]),
                if v["makeDefault"] == true { "on" } else { "off" }),
            "error": v["error"],
            "done": v["done"],
            "request": v["request"],
        })
    }

    /// The Share sheet.
    pub fn share(v: &Value) -> Value {
        json!({
            "title": v["title"],
            "rows": v["rows"].as_array().into_iter().flatten().map(|r| Value::String(format!(
                "{} | {}{}", str_of(&r["who"]), str_of(&r["role"]),
                if r["connected"] == true { " | connected" } else { "" }))).collect::<Vec<_>>(),
            "empty": v["emptyText"],
            "field": format!("{} [{}] {} as {}", str_of(&v["whoPlaceholder"]), str_of(&v["who"]),
                opt_str(&v["hint"]), str_of(&v["role"])),
            "roles": v["roles"].as_array().into_iter().flatten().map(|r| format!(
                "{}={}", str_of(&r["id"]), str_of(&r["label"]))).collect::<Vec<_>>().join(", "),
            "buttons": format!("{} {} / {} / {}", str_of(&v["shareLabel"]),
                if v["canShare"] == true { "on" } else { "off" }, str_of(&v["removeLabel"]), str_of(&v["doneLabel"])),
            "disabled": v["disabledReason"],
            "busy": v["busy"],
            "error": v["error"],
            "request": v["request"],
        })
    }

    /// The enroll sheet.
    pub fn enroll(v: &Value) -> Value {
        json!({
            "title": format!("{} / {}", str_of(&v["title"]), str_of(&v["lede"])),
            "options": v["options"].as_array().into_iter().flatten().map(|o| Value::String(format!(
                "{}: {} / {}", str_of(&o["method"]), str_of(&o["title"]), str_of(&o["detail"])))).collect::<Vec<_>>(),
            "code": v["code"],
            "codeHelp": v["codeHelp"],
            "status": v["status"],
            "error": v["error"],
            "busy": v["busy"],
            "done": v["done"],
            "buttons": format!("{} / {}", opt_str(&v["backLabel"]), str_of(&v["closeLabel"])),
        })
    }

    /// The approval sheet.
    pub fn approve(v: &Value) -> Value {
        let r = &v["request"];
        json!({
            "title": v["title"],
            "message": v["message"],
            "code": if v["needsCode"] == true {
                Value::String(format!("{} [{}] = {}", str_of(&v["codeLabel"]), str_of(&v["codePlaceholder"]), str_of(&v["code"])))
            } else { Value::Null },
            "canApprove": v["canApprove"],
            "buttons": format!("{} / {}{}", str_of(&v["approveLabel"]), str_of(&v["denyLabel"]),
                if v["denyRevokes"] == true { " (revokes)" } else { "" }),
            "presence": v["presenceReason"],
            "busy": v["busy"],
            "error": v["error"],
            "request": if r.is_null() { Value::Null } else {
                Value::String(format!("code={} device={}", opt_str(&r["code"]), opt_str(&r["deviceId"])))
            },
        })
    }

    /// A passphrase check, as one line.
    pub fn kv_check(v: &Value) -> Value {
        Value::String(format!(
            "submit={} strength={} hint={}",
            v["canSubmit"] == true,
            opt_str(&v["strength"]),
            opt_str(&v["hint"])
        ))
    }
}

fn c(h: &dyn Host, method: &str, args: Value) -> Result<Value, String> {
    h.call(method, args).map_err(|e| format!("{method}: {e}"))
}

/// Replays `flow` (its JSON) on `host` and returns the transcript.
pub fn run(name: &str, flow_json: &str, h: &dyn Host) -> Result<Value, String> {
    let flow: Value = serde_json::from_str(flow_json).map_err(|e| e.to_string())?;
    match name {
        "create-space" | "create-resources" | "create-gpu" => run_create_space(&flow, h),
        "teleport-review" | "teleport-sign-ins" => run_teleport(&flow, h),
        "keyvault-approve-deny" => run_keyvault(&flow, h),
        "keyvault-unlock" => run_keyvault_unlock(&flow, h),
        "main-window" => run_main_window(&flow, h),
        "notch" => run_notch(&flow, h),
        "notch-drag-trigger" => run_notch_drag_trigger(&flow, h),
        "provisioning" | "create-progress" | "create-cancel" => run_provisioning(&flow, h),
        "stream-section" => run_stream_section(&flow, h),
        "space-facts" => run_space_facts(&flow, h),
        "picker-grid" => run_picker_grid(&flow, h),
        "delete-space" => run_delete_space(&flow, h),
        "space-power" => run_space_power(&flow, h),
        "devices" => run_devices(&flow, h),
        "driver-card" => run_driver_card(&flow, h),
        "share-sheet" => run_share(&flow, h),
        "agents-page" => run_agents_page(&flow, h),
        "drive-page" => run_drive_page(&flow, h),
        "drive-onboarding" => run_drive_onboarding(&flow, h),
        "menu-count" => run_menu_count(&flow, h),
        "drive-storage" => run_drive_storage(&flow, h),
        "notifications" => run_notifications(&flow, h),
        "about" => run_about(&flow, h),
        "your-cloud" => run_your_cloud(&flow, h),
        "telemetry-funnel" => run_telemetry(&flow, h),
        "launch-at-login" => run_launch_at_login(&flow, h),
        "experiments" => run_experiments(&flow, h),
        "placement-picker" => run_placement_picker(&flow, h),
        other => Err(format!("unknown flow {other}")),
    }
}

fn run_create_space(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let env = &f["env"];
    let mut frames = Vec::new();
    let mut state = c(h, "wizard.initial", json!({ "env": env }))?;
    let view = c(h, "wizard.view", json!({ "state": state, "env": env }))?;
    frames.push(json!({ "wizard": "initial", "frame": frame::wizard(&view) }));
    for action in f["wizard"].as_array().into_iter().flatten() {
        state = c(
            h,
            "wizard.reduce",
            json!({ "state": state, "action": action, "env": env }),
        )?;
        let view = c(h, "wizard.view", json!({ "state": state, "env": env }))?;
        frames.push(json!({ "wizard": action["type"], "frame": frame::wizard(&view) }));
    }
    let view = c(h, "wizard.view", json!({ "state": state, "env": env }))?;
    let args = c(h, "wizard.createArgs", json!({ "plan": view["plan"] }))?;
    let creating = c(h, "wizard.creatingText", json!({ "plan": view["plan"] }))?;
    let failed = c(
        h,
        "wizard.createFailedText",
        json!({ "error": f["createError"] }),
    )?;
    frames.push(json!({ "plan": frame::plan(&args, &view, &creating, &failed) }));

    let spaces = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": f["rows"], "now": f["now"] }),
    )?;
    let mut roster = c(h, "roster.initial", json!({ "spaces": [] }))?;
    roster = c(
        h,
        "roster.reduce",
        json!({ "state": roster, "action": { "type": "sync-spaces", "spaces": spaces } }),
    )?;
    frames.push(json!({ "roster": "sync-spaces", "frame": frame::roster(&roster) }));
    for action in f["roster"].as_array().into_iter().flatten() {
        roster = c(
            h,
            "roster.reduce",
            json!({ "state": roster, "action": action }),
        )?;
        frames.push(json!({ "roster": action["type"], "frame": frame::roster(&roster) }));
    }
    let spaces = roster["spaces"].clone();
    let selected = roster["selectedId"].clone();
    let sb = c(
        h,
        "sidebar.build",
        json!({ "spaces": spaces, "query": "", "selectedId": selected }),
    )?;
    frames.push(json!({ "sidebar": "", "frame": frame::sidebar(&sb) }));
    let sb = c(
        h,
        "sidebar.build",
        json!({ "spaces": spaces, "query": f["query"], "selectedId": selected }),
    )?;
    frames.push(json!({ "sidebar": f["query"], "frame": frame::sidebar(&sb) }));
    for space in spaces.as_array().into_iter().flatten() {
        let d = c(h, "sidebar.detail", json!({ "space": space }))?;
        frames.push(json!({ "detail": space["id"], "frame": frame::detail(&d) }));
    }
    Ok(json!({ "flow": f["name"], "frames": frames }))
}

fn picker_frame(h: &dyn Host, state: &Value) -> Result<Value, String> {
    let sections = c(h, "flow.sections", json!({ "state": state }))?;
    let review = c(h, "flow.review", json!({ "state": state }))?;
    let can_plan = c(h, "flow.canPlan", json!({ "state": state }))?;
    let progress = c(h, "flow.progress", json!({ "state": state }))?
        .as_f64()
        .unwrap_or(0.0);
    let sensitive = c(h, "flow.sensitiveOptions", json!({ "state": state }))?;
    let plan_sensitive = c(h, "flow.planSensitive", json!({ "state": state }))?;
    Ok(frame::picker(
        state,
        &sections,
        &review,
        &can_plan,
        progress,
        &sensitive,
        &plan_sensitive,
    ))
}

fn run_teleport(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let mut state = c(h, "flow.initial", json!({ "spaceName": f["spaceName"] }))?;
    frames.push(json!({ "picker": "initial", "frame": picker_frame(h, &state)? }));
    for event in f["picker"].as_array().into_iter().flatten() {
        let mut ev = event.clone();
        match event["type"].as_str() {
            Some("loaded") => ev["entries"] = f["entries"].clone(),
            Some("planned") => ev["plan"] = f["plan"].clone(),
            _ => {}
        }
        state = c(h, "flow.reduce", json!({ "state": state, "event": ev }))?;
        frames.push(json!({ "picker": event["type"], "frame": picker_frame(h, &state)? }));
    }
    let consent = c(h, "flow.consent", json!({ "state": state }))?;
    frames.push(json!({ "consent": consent }));

    let mut notch = c(h, "notch.initial", json!({}))?;
    for event in f["notch"].as_array().into_iter().flatten() {
        let t = c(h, "notch.reduce", json!({ "state": notch, "event": event }))?;
        notch = t["state"].clone();
        let view = c(
            h,
            "notch.view",
            json!({ "state": notch, "spaces": f["spaces"] }),
        )?;
        let label = match event["type"].as_str() {
            Some("drag") => format!("drag {}", event["event"]["type"].as_str().unwrap_or("")),
            other => other.unwrap_or("").to_string(),
        };
        frames.push(json!({ "notch": label, "frame": frame::notch(&notch, &view, &t["effects"]) }));
    }
    Ok(json!({ "flow": f["name"], "frames": frames }))
}

/// The notch panel on its own: geometry per screen, the shared motion, then
/// hover, permission, pasteboard and window-drag events.
fn run_notch(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    for screen in f["screens"].as_array().into_iter().flatten() {
        for prompt in [false, true] {
            let l = c(
                h,
                "notch.layout",
                json!({ "screen": screen["facts"], "prompt": prompt }),
            )?;
            frames.push(json!({
                "layout": format!("{}{}", screen["name"].as_str().unwrap_or(""), if prompt { " +prompt" } else { "" }),
                "frame": frame::notch_layout(&l),
            }));
        }
    }
    frames.push(json!({ "motion": c(h, "notch.motion", json!({}))? }));
    let mut notch = c(h, "notch.initial", json!({}))?;
    for event in f["events"].as_array().into_iter().flatten() {
        let t = c(h, "notch.reduce", json!({ "state": notch, "event": event }))?;
        notch = t["state"].clone();
        let view = c(
            h,
            "notch.view",
            json!({ "state": notch, "spaces": f["spaces"] }),
        )?;
        let label = match event["type"].as_str() {
            Some("drag") => format!("drag {}", event["event"]["type"].as_str().unwrap_or("")),
            Some("drag-permission") => format!("drag-permission {}", event["granted"]),
            Some("drop-targeted") => format!("drop-targeted {}", event["targeted"]),
            Some("search") => format!("search {}", event["query"]),
            Some("visibility") => format!("visibility shown={}", event["shown"]),
            Some("activity") => format!(
                "activity hotspot={} transfer={}",
                event["hotspot"], event["transfer"]
            ),
            other => other.unwrap_or("").to_string(),
        };
        frames.push(json!({ "notch": label, "frame": frame::notch(&notch, &view, &t["effects"]) }));
    }
    Ok(json!({ "flow": "notch", "frames": frames }))
}

/// A dragged window and the notch: the trigger geometry per display, then
/// each drag event's phase, move or resize, the overlay events it sends and
/// the tick it asks for.
fn run_notch_drag_trigger(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let screens: Vec<Value> = f["screens"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| s["facts"].clone())
        .collect();
    let displays = c(h, "dragTrigger.displays", json!({ "screens": screens }))?;
    let portal = c(
        h,
        "dragTrigger.portalDisplays",
        json!({ "screens": screens }),
    )?;
    let r = |v: &Value| json!([v["x"], v["y"], v["width"], v["height"]]);
    let mut frames = Vec::new();
    for (i, s) in f["screens"].as_array().into_iter().flatten().enumerate() {
        let (d, p) = (&displays[i], &portal[i]);
        frames.push(json!({
            "display": s["name"],
            "frame": r(&d["frame"]),
            "notch": r(&d["notch"]),
            "prompt": r(&d["prompt"]),
            "expanded": r(&d["expanded"]),
            "portalExpanded": r(&p["expanded"]),
        }));
    }
    let mut state = c(h, "dragTrigger.initial", json!({}))?;
    for event in f["events"].as_array().into_iter().flatten() {
        let t = c(
            h,
            "dragTrigger.apply",
            json!({ "state": state, "event": event, "displays": displays }),
        )?;
        state = t["state"].clone();
        let overlay: Vec<Value> = t["overlay"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| o["type"].clone())
            .collect();
        frames.push(json!({
            "event": event["type"],
            "phase": state["phase"],
            "kind": state["kind"],
            "display": state["display"],
            "overlay": overlay,
            "tickAt": t["tickAtMs"],
        }));
    }
    Ok(json!({ "flow": "notch-drag-trigger", "frames": frames }))
}

/// One frame of the provisioning flow: the composed list through the
/// roster, the sidebar rows, the closed notch's activity and the tiles, and
/// each Space's detail.
fn provisioning_frame(
    h: &dyn Host,
    registry: &Value,
    creates: &Value,
    roster: &mut Value,
) -> Result<Value, String> {
    let spaces = c(
        h,
        "creates.compose",
        json!({ "spaces": registry, "state": creates }),
    )?;
    *roster = c(
        h,
        "roster.reduce",
        json!({ "state": roster, "action": { "type": "sync-spaces", "spaces": spaces } }),
    )?;
    let spaces = roster["spaces"].clone();
    let sb = c(
        h,
        "sidebar.build",
        json!({ "spaces": spaces, "query": "", "selectedId": roster["selectedId"] }),
    )?;
    let rows: Vec<Value> = sb["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|sec| sec["rows"].as_array().cloned().unwrap_or_default())
        .map(|r| {
            Value::String(format!(
                "{} {} [{}] {} {}{}",
                r["osIcon"].as_str().unwrap_or(""),
                r["name"].as_str().unwrap_or(""),
                r["statusText"].as_str().unwrap_or(""),
                r["progress"]
                    .as_u64()
                    .map(|p| p.to_string())
                    .unwrap_or("-".into()),
                r["trailing"].as_str().unwrap_or("-"),
                frame::power(&r["power"]),
            ))
        })
        .collect();
    let notch = c(h, "notch.initial", json!({}))?;
    let closed = c(h, "notch.view", json!({ "state": notch, "spaces": spaces }))?;
    let tiles: Vec<Value> = c(h, "notch.tiles", json!({ "spaces": spaces }))?
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            Value::String(format!(
                "{} {} {} {}",
                t["symbol"].as_str().unwrap_or(""),
                t["label"].as_str().unwrap_or(""),
                t["progress"]
                    .as_u64()
                    .map(|p| p.to_string())
                    .unwrap_or("-".into()),
                t["progressLabel"].as_str().unwrap_or("-"),
            ))
        })
        .collect();
    let mut details = Vec::new();
    for space in spaces.as_array().into_iter().flatten() {
        let d = c(h, "sidebar.detail", json!({ "space": space }))?;
        let actions: Vec<Value> = d["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| {
                Value::String(format!(
                    "{}{}",
                    a["label"].as_str().unwrap_or(""),
                    if a["enabled"] == true { "" } else { " (off)" }
                ))
            })
            .collect();
        let mut detail = json!({
            "id": space["id"],
            "facts": facts(&d["facts"]),
            "previewText": d["previewText"],
            "progress": d["progress"],
            // "4.2 of 22.1 GB · 85 MB/s · about 4 min" while it downloads.
            "progressText": d["progressText"],
            "canStream": d["canStream"],
            "actions": actions,
            // `text [button -> url]`, when a cloud create ran out of credit.
            "creditNotice": if d["creditNotice"].is_null() { Value::Null } else {
                Value::String(format!("{} [{} -> {}]", d["creditNotice"]["text"].as_str().unwrap_or(""),
                    d["creditNotice"]["button"].as_str().unwrap_or(""), d["creditNotice"]["url"].as_str().unwrap_or("")))
            },
        });
        // Why turning it off or on failed, when it did.
        if let Some(e) = d["powerError"].as_str() {
            detail["powerError"] = Value::String(e.into());
        }
        details.push(detail);
    }
    Ok(json!({
        "ids": spaces.as_array().map(|a| a.iter().map(|x| x["id"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "rows": rows,
        "activity": {
            "kind": closed["activity"]["kind"],
            "label": closed["activity"]["label"],
            "permille": closed["activity"]["permille"],
        },
        "tiles": tiles,
        "details": details,
        "pending": c(h, "creates.isPending", json!({ "id": "pending:1" }))?,
    }))
}

fn run_provisioning(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let registry = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": f["rows"], "now": now }),
    )?;
    let mut roster = c(h, "roster.initial", json!({ "spaces": [] }))?;
    let mut creates = json!({ "pending": [] });
    let mut frames = vec![json!({
        "creates": "initial",
        "frame": provisioning_frame(h, &registry, &creates, &mut roster)?,
    })];
    for action in f["creates"].as_array().into_iter().flatten() {
        creates = c(
            h,
            "creates.reduce",
            json!({ "state": creates, "action": action }),
        )?;
        let label = format!(
            "{} {} {}",
            action["type"].as_str().unwrap_or(""),
            action["id"].as_str().unwrap_or(""),
            action["phase"].as_str().unwrap_or(""),
        );
        frames.push(json!({
            "creates": label.trim_end(),
            "frame": provisioning_frame(h, &registry, &creates, &mut roster)?,
        }));
    }
    // The registry refresh after the create lists the new Space: its
    // pending row gives way to it.
    let refreshed = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": f["refreshed"], "now": now }),
    )?;
    frames.push(json!({
        "creates": "refreshed",
        "frame": provisioning_frame(h, &refreshed, &creates, &mut roster)?,
    }));
    Ok(json!({ "flow": "provisioning", "frames": frames }))
}

/// Deletes in flight: each step is a delete action or a registry refresh
/// (which settles the state); the frame is the provisioning frame (the list,
/// sidebar, notch and details), the deletes in flight, and the banner a
/// failed delete shows.
fn run_delete_space(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let mut registry = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": f["rows"], "now": now }),
    )?;
    let mut roster = c(h, "roster.initial", json!({ "spaces": [] }))?;
    let mut creates = json!({ "pending": [], "deleting": [] });
    let mut frames = Vec::new();
    for step in f["steps"].as_array().into_iter().flatten() {
        if step["action"].is_object() {
            creates = c(
                h,
                "creates.reduce",
                json!({ "state": creates, "action": step["action"] }),
            )?;
        }
        if step["rows"].is_array() {
            registry = c(
                h,
                "spaces.rowsToSpaces",
                json!({ "rows": step["rows"], "now": now }),
            )?;
            creates = c(
                h,
                "creates.settle",
                json!({ "state": creates, "spaces": registry }),
            )?;
        }
        let deleting: Vec<Value> = creates["deleting"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                Value::String(format!(
                    "{}{}",
                    d["id"].as_str().unwrap_or(""),
                    if d["done"] == true { " done" } else { "" }
                ))
            })
            .collect();
        let id = step["action"]["id"].clone();
        let banner = match step["error"].as_str() {
            Some(e) => {
                let name = registry
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| s["id"] == id)
                    .map(|s| s["name"].clone())
                    .unwrap_or(Value::Null);
                c(
                    h,
                    "sidebar.deleteFailedText",
                    json!({ "name": name, "error": e }),
                )?
            }
            None => Value::Null,
        };
        frames.push(json!({
            "step": step["step"],
            "frame": provisioning_frame(h, &registry, &creates, &mut roster)?,
            "deleting": deleting,
            "isDeleting": if id.is_string() {
                c(h, "creates.isDeleting", json!({ "state": creates, "id": id }))?
            } else {
                Value::Null
            },
            "banner": banner,
        }));
    }
    Ok(json!({ "flow": "delete-space", "frames": frames }))
}

/// Power actions in flight: each step is a power action or a registry
/// refresh (which settles the state); the frame is the provisioning frame
/// (rows with their power buttons, details with the Power action and any
/// inline error) and the power actions in flight.
fn run_space_power(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let mut registry = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": f["rows"], "now": now }),
    )?;
    let mut roster = c(h, "roster.initial", json!({ "spaces": [] }))?;
    let mut creates = json!({ "pending": [], "deleting": [], "powering": [] });
    let mut frames = Vec::new();
    for step in f["steps"].as_array().into_iter().flatten() {
        if step["action"].is_object() {
            creates = c(
                h,
                "creates.reduce",
                json!({ "state": creates, "action": step["action"] }),
            )?;
        }
        if step["rows"].is_array() {
            registry = c(
                h,
                "spaces.rowsToSpaces",
                json!({ "rows": step["rows"], "now": now }),
            )?;
            creates = c(
                h,
                "creates.settle",
                json!({ "state": creates, "spaces": registry }),
            )?;
        }
        let powering: Vec<Value> = creates["powering"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                Value::String(format!(
                    "{} {}{}{}",
                    p["id"].as_str().unwrap_or(""),
                    if p["on"] == true { "on" } else { "off" },
                    if p["done"] == true { " done" } else { "" },
                    p["error"]
                        .as_str()
                        .map(|e| format!(" !{e}"))
                        .unwrap_or_default(),
                ))
            })
            .collect();
        frames.push(json!({
            "step": step["step"],
            "frame": provisioning_frame(h, &registry, &creates, &mut roster)?,
            "powering": powering,
        }));
    }
    Ok(json!({ "flow": "space-power", "frames": frames }))
}

fn run_keyvault(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let o = &f["overview"];
    let now = &f["now"];
    let mut frames = Vec::new();
    let sb = c(h, "keyvault.sidebar", json!({ "overview": o, "now": now }))?;
    frames.push(json!({ "sidebar": frame::kv_sidebar(&sb) }));
    let page = c(h, "keyvault.page", json!({ "overview": o, "now": now }))?;
    frames.push(json!({ "page": frame::kv_page(&page) }));
    for sel in f["selections"].as_array().into_iter().flatten() {
        let l = c(
            h,
            "keyvault.list",
            json!({ "overview": o, "selection": sel, "now": now }),
        )?;
        frames.push(json!({ "list": sel, "frame": frame::kv_list(&l) }));
    }
    for a in f["approvals"].as_array().into_iter().flatten() {
        let mut st = c(h, "approval.open", json!({ "requestId": a["request"] }))?;
        let v = c(h, "approval.view", json!({ "overview": o, "state": st }))?;
        frames.push(
            json!({ "approval": a["request"], "action": "open", "frame": frame::approval(&v) }),
        );
        for action in a["actions"].as_array().into_iter().flatten() {
            st = c(
                h,
                "approval.reduce",
                json!({ "overview": o, "state": st, "action": action }),
            )?;
            let v = c(h, "approval.view", json!({ "overview": o, "state": st }))?;
            frames.push(
                json!({ "approval": a["request"], "action": action, "frame": frame::approval(&v) }),
            );
        }
        let cmd = c(
            h,
            "approval.approveCommand",
            json!({ "overview": o, "state": st }),
        )?;
        frames.push(json!({ "approval": a["request"], "approveCommand": cmd }));
    }
    let deny = c(
        h,
        "approval.denyCommand",
        json!({ "state": { "requestId": f["deny"], "selected": [] } }),
    )?;
    frames.push(json!({ "denyCommand": deny }));
    let vault = &f["vault"];
    let mut st = vault["state"].clone();
    let v = c(
        h,
        "keyvault.vaultView",
        json!({ "overview": o, "state": st, "now": now }),
    )?;
    frames.push(json!({ "vault": "open", "frame": frame::kv_vault(&v) }));
    for action in vault["actions"].as_array().into_iter().flatten() {
        st = c(
            h,
            "keyvault.vaultReduce",
            json!({ "overview": o, "state": st, "action": action }),
        )?;
        let v = c(
            h,
            "keyvault.vaultView",
            json!({ "overview": o, "state": st, "now": now }),
        )?;
        frames.push(json!({ "vault": action, "frame": frame::kv_vault(&v) }));
    }
    // One app's list: the sidebar's pick narrows the state.
    let mut one = vault["state"].clone();
    one["app"] = vault["app"].clone();
    let v = c(
        h,
        "keyvault.vaultView",
        json!({ "overview": o, "state": one, "now": now }),
    )?;
    frames.push(json!({ "vaultApp": vault["app"], "frame": frame::kv_vault(&v) }));
    for u in vault["unlock"].as_array().into_iter().flatten() {
        let p = c(
            h,
            "keyvault.unlockPrompt",
            json!({ "overview": o, "count": u["count"], "name": u["name"] }),
        )?;
        frames.push(json!({ "unlockPrompt": u, "frame": frame::kv_unlock_prompt(&p) }));
    }
    let mut skip = o.clone();
    skip["status"]["skip_unlock_prompt"] = Value::Bool(true);
    let p = c(
        h,
        "keyvault.unlockPrompt",
        json!({ "overview": skip, "count": 1, "name": null }),
    )?;
    frames.push(json!({ "unlockPromptSkipped": frame::kv_unlock_prompt(&p) }));
    for d in vault["delete"].as_array().into_iter().flatten() {
        let p = c(
            h,
            "keyvault.deleteConfirm",
            json!({ "count": d["count"], "liveCopies": d["liveCopies"] }),
        )?;
        frames.push(json!({ "deleteConfirm": d, "frame": frame::kv_delete_confirm(&p) }));
    }
    let mut off = o.clone();
    off["status"]["disabled"] = Value::Bool(true);
    let page = c(h, "keyvault.page", json!({ "overview": off, "now": now }))?;
    frames.push(json!({ "page": frame::kv_page(&page) }));
    let d = &f["disabled"];
    let mut st = c(h, "approval.open", json!({ "requestId": d["request"] }))?;
    for action in d["actions"].as_array().into_iter().flatten() {
        st = c(
            h,
            "approval.reduce",
            json!({ "overview": off, "state": st, "action": action }),
        )?;
    }
    let v = c(h, "approval.view", json!({ "overview": off, "state": st }))?;
    let cmd = c(
        h,
        "approval.approveCommand",
        json!({ "overview": off, "state": st }),
    )?;
    frames.push(json!({ "disabledApproval": frame::approval(&v), "approveCommand": cmd }));
    let on = c(h, "keyvault.page", json!({ "overview": o, "now": now }))?;
    let sb = c(h, "keyvault.sidebar", json!({ "overview": o, "now": now }))?;
    let key = c(h, "keyvault.recoveryKeyText", json!({ "key": "ABCD-EFGH" }))?;
    frames.push(json!({
        "labels": frame::copy(&on["labels"], frame::KV_LABEL_KEYS),
        "killSwitchHelp": [on["killSwitchHelp"], page["killSwitchHelp"]],
        "badges": sb["categories"].as_array().map(|a| a.iter().map(|x| Value::String(format!("{}:{}",
            x["title"].as_str().unwrap_or(""), x["badge"].as_u64().map(|n| n.to_string()).unwrap_or_else(|| "-".into())))).collect::<Vec<_>>()).unwrap_or_default(),
        "recoveryKey": key,
    }));
    Ok(json!({ "flow": "keyvault-approve-deny", "frames": frames }))
}

fn run_keyvault_unlock(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let mut frames = Vec::new();
    for case in f["cases"].as_array().into_iter().flatten() {
        let o = &case["overview"];
        let page = c(h, "keyvault.page", json!({ "overview": o, "now": now }))?;
        let form = c(h, "keyvault.credentialForm", json!({ "overview": o }))?;
        if form != page["form"] {
            return Err(format!(
                "{}: the page's form differs from credentialForm",
                case["name"].as_str().unwrap_or("")
            ));
        }
        frames.push(json!({
            "case": case["name"],
            "title": page["unavailableTitle"],
            "canSetup": page["canSetup"],
            "canUnlock": page["canUnlock"],
            "form": frame::kv_form(&form),
            "unlockFact": page["protection"].as_array().into_iter().flatten()
                .find(|f| f["label"] == "Unlock").map(|f| f["value"].clone()).unwrap_or(Value::Null),
        }));
    }
    for chk in f["checks"].as_array().into_iter().flatten() {
        let r = c(
            h,
            "keyvault.passphraseCheck",
            json!({ "mode": chk["mode"], "passphrase": chk["passphrase"], "confirm": chk["confirm"] }),
        )?;
        frames.push(json!({ "check": chk["label"], "result": frame::kv_check(&r) }));
    }
    Ok(json!({ "flow": "keyvault-unlock", "frames": frames }))
}

fn run_main_window(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let os = &f["os"];
    let mut frames = Vec::new();
    let spaces = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": f["rows"], "now": now }),
    )?;
    let mut last_roster = Value::Null;
    for state in f["hostStates"].as_array().into_iter().flatten() {
        let status = if state.is_null() {
            Value::Null
        } else {
            c(h, "host.summaryInput", json!({ "state": state }))?
        };
        let with = c(
            h,
            "host.withThisMachine",
            json!({ "spaces": spaces, "status": status, "now": now, "os": os }),
        )?;
        let mut roster = c(h, "roster.initial", json!({ "spaces": [] }))?;
        roster = c(
            h,
            "roster.reduce",
            json!({ "state": roster, "action": { "type": "sync-spaces", "spaces": with } }),
        )?;
        let sb = c(
            h,
            "sidebar.build",
            json!({ "spaces": roster["spaces"], "query": "", "selectedId": "" }),
        )?;
        let panel = c(h, "host.panel", json!({ "state": state }))?;
        frames.push(json!({
            "host": if state.is_null() { Value::Null } else { state["configured"].clone() },
            "sidebar": frame::window_sidebar(&sb),
            "panel": frame::host_panel(&panel),
        }));
        last_roster = roster;
    }
    let list = last_roster["spaces"].clone();
    let sb = c(
        h,
        "sidebar.build",
        json!({ "spaces": list, "query": f["query"], "selectedId": "this-mac" }),
    )?;
    frames.push(json!({ "query": f["query"], "sidebar": frame::window_sidebar(&sb) }));
    for space in list.as_array().into_iter().flatten() {
        let d = c(h, "sidebar.detail", json!({ "space": space }))?;
        frames.push(json!({ "detail": space["id"], "frame": frame::window_detail(&d) }));
    }
    let copy = c(h, "sidebar.detailCopy", json!({}))?;
    let failed = c(
        h,
        "sidebar.deleteFailedText",
        json!({ "name": f["deleteFailed"]["name"], "error": f["deleteFailed"]["error"] }),
    )?;
    frames.push(json!({ "detailCopy": frame::copy(&copy, frame::DETAIL_COPY_KEYS), "deleteFailed": failed }));

    let identity = &f["formIdentity"];
    let mut form = c(h, "host.formInitial", json!({}))?;
    let view = c(
        h,
        "host.formView",
        json!({ "state": form, "identity": identity }),
    )?;
    frames.push(json!({ "hostForm": "initial", "frame": frame::host_form(&view) }));
    for action in f["hostForm"].as_array().into_iter().flatten() {
        form = c(
            h,
            "host.formReduce",
            json!({ "state": form, "action": action }),
        )?;
        let view = c(
            h,
            "host.formView",
            json!({ "state": form, "identity": identity }),
        )?;
        frames.push(json!({ "hostForm": action["type"], "frame": frame::host_form(&view) }));
    }

    for input in f["chrome"].as_array().into_iter().flatten() {
        let v = c(h, "window.chrome", json!({ "input": input }))?;
        frames.push(json!({ "chrome": frame::chrome(&v) }));
    }
    for n in f["menuBar"].as_array().into_iter().flatten() {
        let v = c(h, "window.menuBar", json!({ "spaces": n }))?;
        frames.push(json!({ "menuBar": n, "frame": frame::menu(&v) }));
    }

    let ag = &f["agents"];
    let rows = c(
        h,
        "agents.settingsRows",
        json!({ "statuses": ag["statuses"], "total": ag["total"] }),
    )?;
    let mut summaries = Vec::new();
    for r in rows
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["installed"] == true)
    {
        let s = c(
            h,
            "agents.setupSummary",
            json!({ "outcomes": ag["outcomes"], "agent": r["agent"], "name": r["name"] }),
        )?;
        summaries.push(frame::agent_summary(&s));
    }
    frames.push(json!({ "agents": frame::agent_rows(&rows), "summaries": summaries }));
    for (i, input) in f["settings"].as_array().into_iter().flatten().enumerate() {
        let mut input = input.clone();
        if i >= 2 {
            input["agents"] = rows.clone();
        }
        let v = c(h, "settings.page", json!({ "input": input }))?;
        frames.push(json!({ "settings": i, "frame": frame::settings(&v) }));
    }

    let ob = &f["onboarding"];
    let mut st = c(
        h,
        "onboarding.initial",
        json!({ "installerMode": ob["installerMode"], "identity": null }),
    )?;
    let v = c(h, "onboarding.view", json!({ "state": st }))?;
    frames.push(json!({ "onboarding": "initial", "frame": frame::onboarding(&v) }));
    for action in ob["actions"].as_array().into_iter().flatten() {
        st = c(
            h,
            "onboarding.reduce",
            json!({ "state": st, "action": action }),
        )?;
        let v = c(h, "onboarding.view", json!({ "state": st }))?;
        frames.push(json!({ "onboarding": action["type"], "frame": frame::onboarding(&v) }));
    }
    let copy = c(h, "onboarding.copy", json!({}))?;
    let t = &ob["texts"];
    let mut texts = Vec::new();
    for id in t["identities"].as_array().into_iter().flatten() {
        texts.push(c(h, "onboarding.signedInText", json!({ "identity": id }))?);
    }
    for code in t["codes"].as_array().into_iter().flatten() {
        texts.push(c(
            h,
            "onboarding.signInCodeText",
            json!({ "userCode": code }),
        )?);
    }
    for v in t["versions"].as_array().into_iter().flatten() {
        texts.push(c(
            h,
            "onboarding.replacesText",
            json!({ "installedVersion": v }),
        )?);
    }
    texts.push(c(
        h,
        "onboarding.installedAtText",
        json!({ "target": t["target"] }),
    )?);
    frames.push(json!({ "onboardingCopy": frame::copy(&copy, frame::ONBOARDING_COPY_KEYS), "texts": texts }));
    let mut previews = Vec::new();
    for menu_bar in [false, true] {
        let p = c(h, "onboarding.preview", json!({ "menuBar": menu_bar }))?;
        let mut lines = vec![frame::preview_scene(&p)];
        for t in frame::PREVIEW_TIMES {
            let f = c(
                h,
                "onboarding.previewFrame",
                json!({ "menuBar": menu_bar, "tMs": t }),
            )?;
            lines.push(format!("{t}: {}", frame::preview_frame(&f)));
        }
        let still = c(h, "onboarding.previewStill", json!({ "menuBar": menu_bar }))?;
        lines.push(format!("still: {}", frame::preview_frame(&still)));
        previews.push(lines);
    }
    frames.push(json!({ "presentationPreviews": previews }));

    let d = &f["drops"];
    let mut sending = Vec::new();
    for paths in d["sending"].as_array().into_iter().flatten() {
        sending.push(c(h, "transfer.dropSendingText", json!({ "paths": paths }))?);
    }
    let mut sent = Vec::new();
    for files in d["sent"].as_array().into_iter().flatten() {
        sent.push(c(h, "transfer.dropSentText", json!({ "files": files }))?);
    }
    frames.push(json!({ "dropSending": sending, "dropSent": sent }));
    Ok(json!({ "flow": "main-window", "frames": frames }))
}

/// The AI agents page's background computer-use card: its line, the
/// miniature's scene, its frames at the flow's times and the Reduce Motion
/// still, then the setup summary once the card's step ran.
fn run_driver_card(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let copy = c(h, "onboarding.copy", json!({}))?;
    frames
        .push(json!({ "driverCopy": frame::copy(&copy, &["agentsDriver", "agentsDriverImage"]) }));
    let p = c(h, "onboarding.driverPreview", json!({}))?;
    let mut lines = vec![frame::driver_scene(&p)];
    for t in f["times"].as_array().into_iter().flatten() {
        let v = c(h, "onboarding.driverPreviewFrame", json!({ "tMs": t }))?;
        lines.push(format!("{}: {}", t, frame::driver_frame(&v)));
    }
    let still = c(h, "onboarding.driverPreviewStill", json!({}))?;
    lines.push(format!("still: {}", frame::driver_frame(&still)));
    frames.push(json!({ "driverPreview": lines }));
    let mut summaries = Vec::new();
    for s in f["summaries"].as_array().into_iter().flatten() {
        let v = c(
            h,
            "agents.setupSummary",
            json!({ "outcomes": s["outcomes"], "agent": s["agent"], "name": s["name"] }),
        )?;
        summaries.push(frame::agent_summary(&v));
    }
    frames.push(json!({ "summaries": summaries }));
    Ok(json!({ "flow": "driver-card", "frames": frames }))
}

/// A Space's Stream section, one frame per case: each row as one line
/// (`kind id | label | resolution | icon | actions`) and its tooltip.
fn run_stream_section(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut frames = Vec::new();
    for case in f["cases"].as_array().into_iter().flatten() {
        let v = c(
            h,
            "sidebar.streamSection",
            json!({ "input": case["input"] }),
        )?;
        let mut rows = Vec::new();
        let mut help = Vec::new();
        for r in v["rows"].as_array().into_iter().flatten() {
            let icon = &r["icon"];
            let icon = match icon["kind"].as_str() {
                Some("os") => format!("os {}", s(&icon["id"])),
                _ => format!(
                    "app {} {} {}",
                    s(&icon["appName"]),
                    s(&icon["appId"]),
                    icon["pid"]
                ),
            };
            let actions: Vec<String> = r["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    format!(
                        "{} {}: {}{}",
                        s(&a["id"]),
                        s(&a["symbol"]),
                        s(&a["help"]),
                        if a["active"].as_bool() == Some(true) {
                            " (on)"
                        } else {
                            ""
                        }
                    )
                })
                .collect();
            rows.push(format!(
                "{} {} | {} | {} | {} | {}",
                s(&r["kind"]),
                s(&r["id"]),
                s(&r["label"]),
                r["resolution"].as_str().unwrap_or("-"),
                icon,
                actions.join(", ")
            ));
            help.push(s(&r["help"]));
        }
        frames.push(json!({
            "case": case["name"],
            "rows": rows,
            "help": help,
            "statusText": v["statusText"],
        }));
    }
    // Picture in picture: the panels the shell reports, each row's button,
    // and what a click does.
    if let Some(pip) = f.get("pip") {
        let mut open = json!([]);
        for step in pip["steps"].as_array().into_iter().flatten() {
            if let Some(row) = step.get("click") {
                let cmd = c(h, "stream.pipClick", json!({ "open": open, "row": row }))?;
                frames.push(json!({ "click": row, "command": cmd["type"], "row": cmd["row"] }));
                continue;
            }
            open = c(
                h,
                "stream.pipReduce",
                json!({ "open": open, "event": step["event"] }),
            )?;
            let mut input = pip["input"].clone();
            input["open"] = open.clone();
            let v = c(h, "sidebar.streamSection", json!({ "input": input }))?;
            let buttons: Vec<String> = v["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| {
                    let a = &r["actions"][0];
                    format!("{} {} {}", s(&r["id"]), s(&a["symbol"]), a["active"])
                })
                .collect();
            frames
                .push(json!({ "event": step["event"]["type"], "open": open, "buttons": buttons }));
        }
    }
    Ok(json!({ "flow": "stream-section", "frames": frames }))
}

/// A Space detail's facts, one frame per case: a registry row, and the
/// memory and storage use when the case has any.
fn run_space_facts(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    for case in f["cases"].as_array().into_iter().flatten() {
        let spaces = c(
            h,
            "spaces.rowsToSpaces",
            json!({ "rows": [case["row"]], "now": f["now"] }),
        )?;
        let d = c(
            h,
            "sidebar.detail",
            json!({ "space": spaces[0], "usage": case["usage"], "hostArch": f["hostArch"] }),
        )?;
        frames.push(json!({ "case": case["name"], "facts": facts(&d["facts"]) }));
    }
    Ok(json!({ "flow": "space-facts", "frames": frames }))
}

/// A picker grid as lines: `# Section`, then one line per tile
/// (`id | title | icon | thumbnail | help (dim) (selected)`).
fn grid_frame(g: &Value) -> Value {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut lines = Vec::new();
    for sec in g["sections"].as_array().into_iter().flatten() {
        if !s(&sec["title"]).is_empty() {
            lines.push(format!("# {}", s(&sec["title"])));
        }
        for t in sec["tiles"].as_array().into_iter().flatten() {
            let icon = match t["icon"]["kind"].as_str() {
                Some("host") => format!("host {}", s(&t["icon"]["path"])),
                Some("guest") => format!(
                    "guest {} {} {}",
                    s(&t["icon"]["appName"]),
                    s(&t["icon"]["appId"]),
                    t["icon"]["pid"]
                ),
                _ => "-".into(),
            };
            let thumb = match t["thumbnail"]["kind"].as_str() {
                Some("host-window") => format!("window {}", t["thumbnail"]["windowId"]),
                Some("guest-window") => format!(
                    "guest {}@{}",
                    s(&t["thumbnail"]["windowId"]),
                    t["thumbnail"]["epoch"]
                ),
                _ => "-".into(),
            };
            lines.push(format!(
                "{} | {} | {icon} | {thumb} | {}{}{}",
                s(&t["id"]),
                s(&t["title"]),
                s(&t["help"]),
                if t["disabled"] == true { " (dim)" } else { "" },
                if t["selected"] == true {
                    " (selected)"
                } else {
                    ""
                },
            ));
        }
    }
    json!({ "tiles": lines, "emptyText": g["emptyText"] })
}

/// The teleport picker's grid: tabs, the Apps tab with arrow keys and a
/// search, then the two window tabs.
fn run_picker_grid(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let tabs = c(h, "grid.tabs", json!({ "spaceName": f["spaceName"] }))?;
    frames.push(json!({ "tabs": tabs }));
    let mut state = c(h, "flow.initial", json!({ "spaceName": f["spaceName"] }))?;
    state = c(
        h,
        "flow.reduce",
        json!({ "state": state, "event": { "type": "loaded", "entries": f["entries"] } }),
    )?;
    let windows = &f["windows"];
    let grid = c(
        h,
        "grid.apps",
        json!({ "state": state, "windows": windows }),
    )?;
    frames.push(json!({ "apps": "loaded", "grid": grid_frame(&grid) }));
    for delta in f["steps"].as_array().into_iter().flatten() {
        let grid = c(
            h,
            "grid.apps",
            json!({ "state": state, "windows": windows }),
        )?;
        let next = c(
            h,
            "grid.step",
            json!({ "grid": grid, "selected": state["selectedId"], "delta": delta }),
        )?;
        state = c(
            h,
            "flow.reduce",
            json!({ "state": state, "event": { "type": "select", "id": next } }),
        )?;
        frames.push(json!({ "step": delta, "selected": state["selectedId"] }));
    }
    state = c(
        h,
        "flow.reduce",
        json!({ "state": state, "event": { "type": "query", "query": f["query"] } }),
    )?;
    let grid = c(
        h,
        "grid.apps",
        json!({ "state": state, "windows": windows }),
    )?;
    frames.push(json!({
        "apps": format!("query {}", f["query"].as_str().unwrap_or("")),
        "grid": grid_frame(&grid),
    }));
    let g = c(
        h,
        "grid.windows",
        json!({ "windows": windows, "query": "", "selected": "12" }),
    )?;
    frames.push(json!({ "windows": "all", "grid": grid_frame(&g) }));
    let g = c(
        h,
        "grid.windows",
        json!({ "windows": windows, "query": "nothing like it", "selected": null }),
    )?;
    frames.push(json!({ "windows": "no match", "grid": grid_frame(&g) }));
    let g = c(
        h,
        "grid.remote",
        json!({ "windows": f["remote"], "query": "", "selected": null }),
    )?;
    frames.push(json!({ "remote": "all", "grid": grid_frame(&g) }));
    let g = c(
        h,
        "grid.remote",
        json!({ "windows": f["remote"], "query": f["remoteQuery"], "selected": "w-1" }),
    )?;
    frames.push(json!({
        "remote": format!("query {}", f["remoteQuery"].as_str().unwrap_or("")),
        "grid": grid_frame(&g),
    }));
    // The primary button of each tab, with and without a selection.
    let apps = c(
        h,
        "grid.apps",
        json!({ "state": state, "windows": windows }),
    )?;
    let windows_selected = c(
        h,
        "grid.windows",
        json!({ "windows": windows, "query": "", "selected": "12" }),
    )?;
    let windows_none = c(
        h,
        "grid.windows",
        json!({ "windows": windows, "query": "", "selected": null }),
    )?;
    for (tab, grid) in [
        ("apps", &apps),
        ("windows", &windows_selected),
        ("windows", &windows_none),
        ("space", &g),
    ] {
        let p = c(
            h,
            "grid.primary",
            json!({ "tab": tab, "spaceName": f["spaceName"], "grid": grid }),
        )?;
        frames.push(json!({ "primary": tab, "label": p["label"], "enabled": p["enabled"] }));
    }
    Ok(json!({ "flow": "picker-grid", "frames": frames }))
}

fn run_devices(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let mut frames = Vec::new();
    let mut last = Value::Null;
    let mut last_devices = Value::Array(Vec::new());
    for step in f["pages"].as_array().into_iter().flatten() {
        last = c(
            h,
            "devices.view",
            json!({ "input": step["input"], "now": now }),
        )?;
        last_devices = step["input"]["devices"].clone();
        frames.push(json!({ "page": step["step"], "view": frame::devices(&last) }));
    }
    for run in f["enroll"].as_array().into_iter().flatten() {
        let mut state = c(h, "devices.enrollInitial", json!({}))?;
        let view = c(h, "devices.enrollView", json!({ "state": state }))?;
        frames
            .push(json!({ "enroll": run["name"], "action": "open", "view": frame::enroll(&view) }));
        for action in run["actions"].as_array().into_iter().flatten() {
            state = c(
                h,
                "devices.enrollReduce",
                json!({ "state": state, "action": action }),
            )?;
            let view = c(h, "devices.enrollView", json!({ "state": state }))?;
            frames.push(json!({
                "enroll": run["name"],
                "action": action["type"],
                "phase": state["phase"],
                "view": frame::enroll(&view),
            }));
        }
    }
    // The approval sheet for each prompt on the last page.
    for (i, run) in f["approve"].as_array().into_iter().flatten().enumerate() {
        let prompt = last["approvals"][i].clone();
        if prompt.is_null() {
            return Err(format!("no approval prompt {i} on the last page"));
        }
        let mut state = c(h, "devices.approveOpen", json!({ "prompt": prompt }))?;
        let view = c(
            h,
            "devices.approveView",
            json!({ "state": state, "devices": last_devices }),
        )?;
        frames.push(json!({ "approve": prompt["deviceId"], "action": "open", "view": frame::approve(&view) }));
        for action in run.as_array().into_iter().flatten() {
            state = c(
                h,
                "devices.approveReduce",
                json!({ "state": state, "action": action, "devices": last_devices }),
            )?;
            let view = c(
                h,
                "devices.approveView",
                json!({ "state": state, "devices": last_devices }),
            )?;
            frames.push(json!({
                "approve": prompt["deviceId"],
                "action": action["type"],
                "view": frame::approve(&view),
            }));
        }
    }
    Ok(json!({ "flow": "devices", "frames": frames }))
}

fn run_share(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let mut state = c(h, "share.initial", json!({}))?;
    let mut input = f["input"].clone();
    for step in f["steps"].as_array().into_iter().flatten() {
        if let Some(patch) = step["input"].as_object() {
            for (k, v) in patch {
                input[k] = v.clone();
            }
        }
        if !step["action"].is_null() {
            state = c(
                h,
                "share.reduce",
                json!({ "input": input, "state": state, "action": step["action"] }),
            )?;
        }
        let view = c(h, "share.view", json!({ "input": input, "state": state }))?;
        frames.push(json!({ "step": step["step"], "view": frame::share(&view) }));
    }
    Ok(json!({ "flow": "share-sheet", "frames": frames }))
}

fn patch(input: &mut Value, step: &Value) {
    if let Some(p) = step["input"].as_object() {
        for (k, v) in p {
            input[k] = v.clone();
        }
    }
}

fn run_agents_page(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let now = f["nowMs"].clone();
    let mut state = c(h, "agents.pageInitial", json!({}))?;
    let mut input = f["input"].clone();
    for step in f["steps"].as_array().into_iter().flatten() {
        patch(&mut input, step);
        if !step["action"].is_null() {
            state = c(
                h,
                "agents.pageReduce",
                json!({ "input": input, "state": state, "action": step["action"] }),
            )?;
        }
        let view = c(
            h,
            "agents.pageView",
            json!({ "input": input, "state": state, "nowMs": now }),
        )?;
        frames.push(json!({ "step": step["step"], "view": frame::agents(&view) }));
    }
    Ok(json!({ "flow": "agents-page", "frames": frames }))
}

fn run_drive_page(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let mut state = c(h, "drive.initial", json!({}))?;
    let mut input = f["input"].clone();
    for step in f["steps"].as_array().into_iter().flatten() {
        patch(&mut input, step);
        if !step["action"].is_null() {
            state = c(
                h,
                "drive.reduce",
                json!({ "state": state, "action": step["action"] }),
            )?;
        }
        let view = c(h, "drive.view", json!({ "input": input, "state": state }))?;
        frames.push(json!({ "step": step["step"], "view": frame::drive(&view) }));
    }
    Ok(json!({ "flow": "drive-page", "frames": frames }))
}

/// The first run's Cua Volume page: the miniature's scene, its frames at the
/// flow's times and the Reduce Motion still, then the page after each of
/// the flow's actions (from the first page).
fn run_drive_onboarding(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let p = c(h, "onboarding.drivePreview", json!({}))?;
    let mut lines = vec![frame::drive_scene(&p)];
    for t in f["times"].as_array().into_iter().flatten() {
        let v = c(h, "onboarding.drivePreviewFrame", json!({ "tMs": t }))?;
        lines.push(format!("{}: {}", t, frame::drive_frame(&v)));
    }
    let still = c(h, "onboarding.drivePreviewStill", json!({}))?;
    lines.push(format!("still: {}", frame::drive_frame(&still)));
    frames.push(json!({ "drivePreview": lines }));
    for run in f["runs"].as_array().into_iter().flatten() {
        let mut st = c(
            h,
            "onboarding.initial",
            json!({ "installerMode": null, "identity": null }),
        )?;
        for step in run["steps"].as_array().into_iter().flatten() {
            st = c(
                h,
                "onboarding.reduce",
                json!({ "state": st, "action": step["action"] }),
            )?;
            if step["step"].is_null() {
                continue;
            }
            let v = c(h, "onboarding.view", json!({ "state": st }))?;
            let storage_request = if st["storage"]["request"].is_null() {
                Value::Null
            } else {
                c(
                    h,
                    "storage.requestText",
                    json!({ "request": st["storage"]["request"] }),
                )?
            };
            frames.push(json!({
                "run": run["name"],
                "step": step["step"],
                "request": st["driveRequest"],
                "storageRequest": storage_request,
                "frame": frame::onboarding(&v),
            }));
        }
    }
    Ok(json!({ "flow": "drive-onboarding", "frames": frames }))
}

/// The usage events a run from first launch to a first ready Space means:
/// each frame is one reducer step and the signals the core derives from it
/// (`telemetry.*`), which every shell sends the same way.
fn run_telemetry(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames =
        vec![json!({ "at": "launch", "signals": c(h, "telemetry.launched", json!({}))? })];
    for run in ["onboarding", "skipping", "no-usage-data"] {
        let mut st = c(
            h,
            "onboarding.initial",
            json!({ "installerMode": null, "identity": null }),
        )?;
        for a in f[run].as_array().into_iter().flatten() {
            let signals = c(
                h,
                "telemetry.onboarding",
                json!({ "state": st, "action": a }),
            )?;
            st = c(h, "onboarding.reduce", json!({ "state": st, "action": a }))?;
            frames.push(
                json!({ "at": run, "action": a["type"], "page": st["step"], "signals": signals }),
            );
        }
        frames.push(json!({
            "at": run,
            "action": "finish",
            "signals": c(h, "telemetry.onboardingFinished", json!({ "state": st }))?,
        }));
    }
    let mut st = Value::Null;
    for step in f["creates"].as_array().into_iter().flatten() {
        let signals = c(
            h,
            "telemetry.creates",
            json!({ "state": st, "action": step["action"], "now": step["now"] }),
        )?;
        st = c(
            h,
            "creates.reduce",
            json!({ "state": st, "action": step["action"] }),
        )?;
        frames
            .push(json!({ "at": "creates", "action": step["action"]["type"], "signals": signals }));
    }
    for (section, method) in [
        ("storage", "telemetry.storage"),
        ("share", "telemetry.share"),
    ] {
        let input = &f[section]["input"];
        for case in f[section]["cases"].as_array().into_iter().flatten() {
            let signals = c(
                h,
                method,
                json!({ "input": input, "state": case["state"], "action": case["action"] }),
            )?;
            frames.push(json!({ "at": section, "step": case["step"], "signals": signals }));
        }
    }
    for case in f["enroll"].as_array().into_iter().flatten() {
        let signals = c(
            h,
            "telemetry.enroll",
            json!({ "state": case["state"], "action": case["action"] }),
        )?;
        frames.push(json!({ "at": "enroll", "step": case["step"], "signals": signals }));
    }
    Ok(json!({ "flow": "telemetry-funnel", "frames": frames }))
}

/// Settings' Storage section: the section after each step (a data patch, an
/// action, or a row's button, choice or edit turned into one by the core),
/// and the command it asks the shell to run.
fn run_drive_storage(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let mut state = c(h, "storage.initial", json!({}))?;
    let mut input = f["input"].clone();
    for step in f["steps"].as_array().into_iter().flatten() {
        patch(&mut input, step);
        let mut actions = Vec::new();
        if !step["action"].is_null() {
            actions.push(step["action"].clone());
        }
        if let Some(id) = step["press"].as_str() {
            actions.push(c(h, "storage.press", json!({ "input": input, "id": id }))?);
        }
        if let Some([id, option]) = step["choose"].as_array().map(Vec::as_slice) {
            actions.push(c(
                h,
                "storage.choose",
                json!({ "id": id, "option": option }),
            )?);
        }
        if let Some([id, value]) = step["edit"].as_array().map(Vec::as_slice) {
            actions.push(c(h, "storage.edit", json!({ "id": id, "value": value }))?);
        }
        for action in actions.into_iter().filter(|a| !a.is_null()) {
            state = c(
                h,
                "storage.reduce",
                json!({ "state": state, "action": action }),
            )?;
        }
        let section = c(
            h,
            "storage.section",
            json!({ "input": input, "state": state }),
        )?;
        let request = if state["request"].is_null() {
            Value::Null
        } else {
            c(
                h,
                "storage.requestText",
                json!({ "request": state["request"] }),
            )?
        };
        frames.push(json!({
            "step": step["step"],
            "section": frame::settings(&json!({ "title": "Settings", "sections": [section] })),
            "request": request,
        }));
    }
    Ok(json!({ "flow": "drive-storage", "frames": frames }))
}

/// The menu bar item and the notch count the same Spaces: for each case
/// (this machine not hosting, hosting, a provider's Spaces, remote Spaces,
/// then Cua Volume's sync states), the notch tab's count, the menu's lines
/// and whether the two agree.
fn run_menu_count(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let now = &f["now"];
    let mut frames = Vec::new();
    for case in f["cases"].as_array().into_iter().flatten() {
        let spaces = c(
            h,
            "spaces.rowsToSpaces",
            json!({ "rows": case["rows"], "now": now }),
        )?;
        let host = &case["hostState"];
        let status = if host.is_null() {
            Value::Null
        } else {
            c(h, "host.summaryInput", json!({ "state": host }))?
        };
        let roster = c(
            h,
            "host.withThisMachine",
            json!({ "spaces": spaces, "status": status, "now": now, "os": "macos" }),
        )?;
        let notch_state = c(h, "notch.initial", json!({}))?;
        let notch = c(
            h,
            "notch.view",
            json!({ "state": notch_state, "spaces": roster }),
        )?;
        let count = c(h, "spaces.openableCount", json!({ "spaces": roster }))?;
        let menu = c(
            h,
            "window.menu",
            json!({ "input": { "spaces": roster, "keyvault": null, "sync": case["sync"], "nowMs": case["nowMs"], "backend": case["backend"] } }),
        )?;
        let lines: Vec<Value> = menu
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["id"] != "separator")
            .map(|m| {
                Value::String(format!(
                    "{}: {}{}",
                    str_of(&m["id"]),
                    str_of(&m["label"]),
                    if m["enabled"] == true {
                        ""
                    } else {
                        " (disabled)"
                    }
                ))
            })
            .collect();
        let tab = str_of(&notch["tab"]["count"]).to_string();
        let first = menu[0]["label"].as_str().unwrap_or("");
        let menu_count = if first.starts_with("No Spaces") {
            "0".to_string()
        } else {
            first.split(' ').next().unwrap_or("").to_string()
        };
        frames.push(json!({
            "case": case["name"],
            "count": count,
            "notch": tab,
            "menu": lines,
            "agree": menu_count == tab && count.as_u64() == tab.parse::<u64>().ok(),
        }));
    }
    Ok(json!({ "flow": "menu-count", "frames": frames }))
}

fn run_notifications(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let now = f["nowMs"].clone();
    let mut seen = json!(0);
    let mut feed = json!([]);
    for step in f["steps"].as_array().into_iter().flatten() {
        if !step["seenMs"].is_null() {
            seen = step["seenMs"].clone();
        }
        if !step["feed"].is_null() {
            feed = step["feed"].clone();
        }
        let plan = c(
            h,
            "notifications.plan",
            json!({ "feed": feed, "seenMs": seen }),
        )?;
        seen = plan["seenMs"].clone();
        let view = c(
            h,
            "notifications.view",
            json!({ "feed": feed, "nowMs": now }),
        )?;
        frames.push(json!({ "step": step["step"], "view": frame::notifications(&plan, &view) }));
    }
    Ok(json!({ "flow": "notifications", "frames": frames }))
}

fn run_about(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let mut input = f["input"].clone();
    for step in f["panes"].as_array().into_iter().flatten() {
        if let (Some(input), Some(set)) = (input.as_object_mut(), step["set"].as_object()) {
            for (k, v) in set {
                input.insert(k.clone(), v.clone());
            }
        }
        let view = c(h, "about.view", json!({ "input": input }))?;
        let channels = c(
            h,
            "about.allowedChannels",
            json!({ "channel": input["channel"] }),
        )?;
        frames.push(json!({ "step": step["step"], "about": frame::about(&view, &channels) }));
    }
    for step in f["launches"].as_array().into_iter().flatten() {
        let plan = c(h, "about.afterLaunch", json!({ "input": step["input"] }))?;
        frames.push(
            json!({ "step": step["step"], "launch": format!("refresh {} save {}",
            plan["refresh"], plan["save"].as_str().unwrap_or("-")) }),
        );
    }
    for step in f["daemons"].as_array().into_iter().flatten() {
        let restart = c(h, "about.restartDaemon", json!({ "check": step["check"] }))?;
        frames.push(json!({ "step": step["step"], "restartDaemon": restart }));
    }
    for step in f["reports"].as_array().into_iter().flatten() {
        let notice = c(
            h,
            "about.refreshNotice",
            json!({ "report": step["report"] }),
        )?;
        frames.push(json!({ "step": step["step"], "notice": notice }));
    }
    Ok(json!({ "flow": "about", "frames": frames }))
}

/// Launch at login: the Done page's checkbox through its toggles, Settings,
/// General's rows for each login-item input, and the launch plans.
fn run_launch_at_login(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut frames = Vec::new();
    let mut st = c(
        h,
        "onboarding.initial",
        json!({ "installerMode": null, "identity": null }),
    )?;
    for action in f["onboarding"].as_array().into_iter().flatten() {
        st = c(
            h,
            "onboarding.reduce",
            json!({ "state": st, "action": action }),
        )?;
        let v = c(h, "onboarding.view", json!({ "state": st }))?;
        if v["step"] == "done" {
            frames.push(json!({
                "onboarding": action["type"],
                "launchAtLogin": st["launchAtLogin"],
                "frame": frame::onboarding(&v),
            }));
        }
    }
    for step in f["settings"].as_array().into_iter().flatten() {
        let v = c(
            h,
            "settings.page",
            json!({ "input": { "signIn": { "kind": "idle" }, "canSignOut": false, "menuBar": false,
                "defaultLocation": "local", "agentsBusy": false, "agentsPending": [],
                "loginItem": step["loginItem"] } }),
        )?;
        let general: Vec<Value> = v["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["id"] == "general")
            .flat_map(|s| s["rows"].as_array().cloned().unwrap_or_default())
            .map(|r| Value::String(frame::setting_row(&r)))
            .collect();
        frames.push(json!({ "step": step["step"], "general": general }));
    }
    for step in f["launches"].as_array().into_iter().flatten() {
        let plan = c(
            h,
            "loginItem.launchPlan",
            json!({ "choice": step["choice"], "onboarded": step["onboarded"],
                "serves": step["serves"], "status": step["status"] }),
        )?;
        frames.push(
            json!({ "step": step["step"], "launch": format!("register {} record {}",
            plan["register"], plan["record"]) }),
        );
    }
    Ok(json!({ "flow": "launch-at-login", "frames": frames }))
}

/// Replays every flow on `host`, comparing against the goldens. Returns
/// `(name, ok, transcript)` per flow.
pub fn check_all(h: &dyn Host) -> Vec<(String, Result<(), String>, Value)> {
    FLOWS
        .iter()
        .map(|(name, flow, golden)| {
            let got = run(name, flow, h);
            match got {
                Err(e) => ((*name).to_string(), Err(e), Value::Null),
                Ok(t) => {
                    let want: Value = serde_json::from_str(golden).unwrap_or(Value::Null);
                    let ok = if t == want {
                        Ok(())
                    } else {
                        Err(first_difference(&want, &t))
                    };
                    ((*name).to_string(), ok, t)
                }
            }
        })
        .collect()
}

/// The first frame that differs, for a readable failure.
pub fn first_difference(want: &Value, got: &Value) -> String {
    let empty = vec![];
    let w = want["frames"].as_array().unwrap_or(&empty);
    let g = got["frames"].as_array().unwrap_or(&empty);
    for (i, (a, b)) in w.iter().zip(g.iter()).enumerate() {
        if a != b {
            return format!("frame {i}: want {a} got {b}");
        }
    }
    format!("frame count: want {} got {}", w.len(), g.len())
}

/// "Your cloud": the wizard and the "Connect a cloud" sheet. A step may
/// connect clouds (`clouds`, by name from the flow's `clouds`) and change
/// the default location before its action.
fn run_your_cloud(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut env = f["env"].clone();
    let sheet = &f["sheet"];
    let mut frames = Vec::new();
    let mut wizard = c(h, "wizard.initial", json!({ "env": env }))?;
    let mut state = c(h, "cloudConnect.initial", json!({}))?;
    for step in f["steps"].as_array().into_iter().flatten() {
        if let Some(names) = step["clouds"].as_array() {
            env["clouds"] = Value::Array(
                names
                    .iter()
                    .map(|n| f["clouds"][str_of(n)].clone())
                    .collect(),
            );
        }
        if !step["defaultLocation"].is_null() {
            env["defaultLocation"] = step["defaultLocation"].clone();
        }
        if step.get("sheet").is_some() {
            if !step["sheet"].is_null() {
                state = c(
                    h,
                    "cloudConnect.reduce",
                    json!({ "input": sheet, "state": state, "action": step["sheet"] }),
                )?;
            }
            let view = c(
                h,
                "cloudConnect.view",
                json!({ "input": sheet, "state": state }),
            )?;
            frames.push(json!({ "step": step["step"], "sheet": frame::cloud_connect(&view) }));
            continue;
        }
        if !step["wizard"].is_null() {
            wizard = c(
                h,
                "wizard.reduce",
                json!({ "state": wizard, "action": step["wizard"], "env": env }),
            )?;
        }
        let view = c(h, "wizard.view", json!({ "state": wizard, "env": env }))?;
        let args = c(h, "wizard.createArgs", json!({ "plan": view["plan"] }))?;
        frames.push(json!({
            "step": step["step"],
            "wizard": frame::your_cloud(&view),
            "createSpace": format!("on={} image={} cpus={} memoryMb={}", str_of(&args["on"]), str_of(&args["image"]),
                args["cpus"], args["memoryMb"]),
        }));
    }
    Ok(json!({ "flow": "your-cloud", "frames": frames }))
}

/// Settings, Experiments: the tab and what every switch decides, after
/// each change (`changes`: a row's choice). Each frame has the tab, the
/// events the change sends, Settings (Storage only with Cua Volume), the
/// first run's pages and its Done page, a Space's buttons (Share only with
/// Sharing) and New Space's Run on menu and links (clouds only with Your
/// cloud).
fn run_experiments(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let storage_state = c(h, "storage.initial", json!({}))?;
    let storage = c(
        h,
        "storage.section",
        json!({ "input": f["storageInput"], "state": storage_state }),
    )?;
    let spaces = c(
        h,
        "spaces.rowsToSpaces",
        json!({ "rows": [f["space"]], "now": f["now"] }),
    )?;
    let space = spaces[0].clone();
    let mut x = json!({});
    let mut frames = Vec::new();
    let mut first = experiments_frame(f, h, &storage, &space, &x)?;
    first["step"] = json!("defaults");
    first["signals"] = json!([]);
    frames.push(first);
    for change in f["changes"].as_array().into_iter().flatten() {
        let before = x.clone();
        x = c(
            h,
            "experiments.choose",
            json!({ "experiments": before, "row": change["row"], "option": change["option"] }),
        )?;
        let signals = c(
            h,
            "telemetry.experimentsChanged",
            json!({ "before": before, "after": x }),
        )?;
        let mut fr = experiments_frame(f, h, &storage, &space, &x)?;
        fr["step"] = change["step"].clone();
        fr["signals"] = signals;
        frames.push(fr);
    }
    Ok(json!({ "flow": "experiments", "frames": frames }))
}

/// What the switches `x` decide (see [`run_experiments`]).
fn experiments_frame(
    f: &Value,
    h: &dyn Host,
    storage: &Value,
    space: &Value,
    x: &Value,
) -> Result<Value, String> {
    let tab = c(h, "experiments.page", json!({ "experiments": x }))?;
    let mut input = f["settings"].clone();
    input["experiments"] = x.clone();
    let page = c(h, "settings.page", json!({ "input": input }))?;
    let settings = c(
        h,
        "settings.withStorage",
        json!({ "page": page, "storage": storage, "experiments": x }),
    )?;
    let mut st = c(
        h,
        "onboarding.initial",
        json!({ "installerMode": null, "identity": null }),
    )?;
    st = c(
        h,
        "onboarding.reduce",
        json!({ "state": st, "action": { "type": "experiments-loaded", "experiments": x } }),
    )?;
    let mut pages = vec![st["step"].clone()];
    for a in f["onboarding"].as_array().into_iter().flatten() {
        st = c(h, "onboarding.reduce", json!({ "state": st, "action": a }))?;
        if pages.last() != Some(&st["step"]) {
            pages.push(st["step"].clone());
        }
    }
    let done = c(h, "onboarding.view", json!({ "state": st }))?;
    let detail = c(
        h,
        "sidebar.detail",
        json!({ "space": space, "experiments": x }),
    )?;
    let chrome = c(h, "window.chrome", json!({ "input": { "experiments": x } }))?;
    let mut menu_input = f["menu"].clone();
    menu_input["experiments"] = x.clone();
    let menu = c(h, "window.menu", json!({ "input": menu_input }))?;
    let mut env = f["env"].clone();
    env["experiments"] = x.clone();
    let wizard = c(h, "wizard.initial", json!({ "env": env }))?;
    let view = c(h, "wizard.view", json!({ "state": wizard, "env": env }))?;
    let fields: Vec<Value> = view["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| f["id"].clone())
        .collect();
    Ok(json!({
        "tab": frame::settings(&tab),
        "settings": frame::settings(&settings),
        "onboarding": {
            "pages": pages,
            "done": frame::onboarding(&done),
        },
        "actions": frame::window_detail(&detail)["actions"],
        "volumePage": chrome["volumeLabel"],
        "menu": frame::menu(&menu),
        "wizard": {
            "placements": frame::placement_menu(&view),
            "fields": fields,
        },
    }))
}

/// New Space's Run on menu: This Mac, your machines (offline and at a limit
/// say why) and, with the Your cloud experiment, your clouds. A step may
/// patch the wizard's env (`env`: the experiments, the hosts) before its
/// action; each frame is the wizard and the create's `on`.
fn run_placement_picker(f: &Value, h: &dyn Host) -> Result<Value, String> {
    let mut env = f["env"].clone();
    let mut frames = Vec::new();
    let mut wizard = c(h, "wizard.initial", json!({ "env": env }))?;
    for step in f["steps"].as_array().into_iter().flatten() {
        if let Some(patch) = step["env"].as_object() {
            for (k, v) in patch {
                env[k.as_str()] = v.clone();
            }
        }
        if !step["wizard"].is_null() {
            wizard = c(
                h,
                "wizard.reduce",
                json!({ "state": wizard, "action": step["wizard"], "env": env }),
            )?;
        }
        let view = c(h, "wizard.view", json!({ "state": wizard, "env": env }))?;
        let args = c(h, "wizard.createArgs", json!({ "plan": view["plan"] }))?;
        frames.push(json!({
            "step": step["step"],
            "wizard": frame::your_cloud(&view),
            "on": args["on"],
        }));
    }
    Ok(json!({ "flow": "placement-picker", "frames": frames }))
}
