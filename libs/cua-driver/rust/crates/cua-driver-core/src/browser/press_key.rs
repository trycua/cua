// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `browser_press_key`: one key or chord to an exactly-bound tab.
//!
//! Native `press_key` and `hotkey` cannot reach a background Chromium window
//! with modifiers held: Chromium reads modifier state from the live keyboard,
//! so a posted chord arrives without them. Here the chord goes through
//! `Input.dispatchKeyEvent` on the tab's own CDP session, with the modifier
//! bitmask on every event, so it reaches the page and the browser's own
//! shortcuts without touching the OS foreground.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::{ProtectedResourceOwnership, Tool, ToolDef};
use crate::tool_args::ArgsExt;

use super::cdp_ws::CdpConnection;
use super::engine::BrowserEngine;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::session_schema as schema_session;
use super::tools::{
    browser_protected_resource_scope, browser_resource_ownership, call_until_dialog, dialog_block,
    dialog_capability_id, require_explicit_session, schema_ref, schema_tab_id, schema_target_id,
};

/// CDP `Input.dispatchKeyEvent` modifier bits.
const ALT: i64 = 1;
const CTRL: i64 = 2;
const META: i64 = 4;
const SHIFT: i64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Meta,
}

impl Modifier {
    fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Ok(Self::Ctrl),
            "shift" => Ok(Self::Shift),
            "alt" | "option" | "opt" => Ok(Self::Alt),
            "meta" | "cmd" | "command" | "win" | "windows" | "super" => Ok(Self::Meta),
            _ => Err(format!(
                "unknown modifier {name:?}: use ctrl, shift, alt, or meta (cmd/win)"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
            Self::Alt => "alt",
            Self::Meta => "meta",
        }
    }

    fn bit(self) -> i64 {
        match self {
            Self::Ctrl => CTRL,
            Self::Shift => SHIFT,
            Self::Alt => ALT,
            Self::Meta => META,
        }
    }

    /// `(key, code, windowsVirtualKeyCode)` of the modifier key itself.
    fn identity(self) -> (&'static str, &'static str, i64) {
        match self {
            Self::Ctrl => ("Control", "ControlLeft", 17),
            Self::Shift => ("Shift", "ShiftLeft", 16),
            Self::Alt => ("Alt", "AltLeft", 18),
            Self::Meta => ("Meta", "MetaLeft", 91),
        }
    }
}

/// The DOM identity of one non-modifier key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyIdentity {
    /// `KeyboardEvent.key` without shift.
    key: String,
    /// `KeyboardEvent.key` with shift held.
    shifted_key: String,
    code: String,
    virtual_key: i64,
    /// Text the key inserts when no command modifier is held.
    text: Option<String>,
    shifted_text: Option<String>,
}

impl KeyIdentity {
    fn named(key: &str, code: &str, virtual_key: i64, text: Option<&str>) -> Self {
        Self {
            key: key.into(),
            shifted_key: key.into(),
            code: code.into(),
            virtual_key,
            text: text.map(str::to_owned),
            shifted_text: text.map(str::to_owned),
        }
    }

    fn printable(key: char, shifted: char, code: &str, virtual_key: i64) -> Self {
        Self {
            key: key.to_string(),
            shifted_key: shifted.to_string(),
            code: code.into(),
            virtual_key,
            text: Some(key.to_string()),
            shifted_text: Some(shifted.to_string()),
        }
    }
}

/// Resolve the `press_key` key vocabulary to its DOM identity: key names,
/// a letter or digit, and the US-layout punctuation keys.
fn key_identity(name: &str) -> Result<KeyIdentity, String> {
    let mut chars = name.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() {
            return Ok(KeyIdentity::printable(
                lower,
                lower.to_ascii_uppercase(),
                &format!("Key{}", lower.to_ascii_uppercase()),
                lower.to_ascii_uppercase() as i64,
            ));
        }
        if ch.is_ascii_digit() {
            const SHIFTED: &[u8; 10] = b")!@#$%^&*(";
            let shifted = SHIFTED[ch as usize - '0' as usize] as char;
            return Ok(KeyIdentity::printable(
                ch,
                shifted,
                &format!("Digit{ch}"),
                ch as i64,
            ));
        }
        let punctuation = match ch {
            '-' => Some(('_', "Minus", 189)),
            '=' => Some(('+', "Equal", 187)),
            '[' => Some(('{', "BracketLeft", 219)),
            ']' => Some(('}', "BracketRight", 221)),
            '\\' => Some(('|', "Backslash", 220)),
            ';' => Some((':', "Semicolon", 186)),
            '\'' => Some(('"', "Quote", 222)),
            ',' => Some(('<', "Comma", 188)),
            '.' => Some(('>', "Period", 190)),
            '/' => Some(('?', "Slash", 191)),
            '`' => Some(('~', "Backquote", 192)),
            _ => None,
        };
        if let Some((shifted, code, virtual_key)) = punctuation {
            return Ok(KeyIdentity::printable(ch, shifted, code, virtual_key));
        }
        if ch == ' ' {
            return Ok(KeyIdentity::named(" ", "Space", 32, Some(" ")));
        }
    }
    let lower = name.trim().to_ascii_lowercase();
    if let Some(n) = lower
        .strip_prefix('f')
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| (1..=12).contains(n))
    {
        let key = format!("F{n}");
        return Ok(KeyIdentity::named(&key, &key, 111 + n, None));
    }
    Ok(match lower.as_str() {
        "return" | "enter" => KeyIdentity::named("Enter", "Enter", 13, Some("\r")),
        "tab" => KeyIdentity::named("Tab", "Tab", 9, None),
        "escape" | "esc" => KeyIdentity::named("Escape", "Escape", 27, None),
        "space" => KeyIdentity::named(" ", "Space", 32, Some(" ")),
        "backspace" => KeyIdentity::named("Backspace", "Backspace", 8, None),
        "delete" | "del" | "forward_delete" => KeyIdentity::named("Delete", "Delete", 46, None),
        "insert" | "ins" => KeyIdentity::named("Insert", "Insert", 45, None),
        "home" => KeyIdentity::named("Home", "Home", 36, None),
        "end" => KeyIdentity::named("End", "End", 35, None),
        "pageup" => KeyIdentity::named("PageUp", "PageUp", 33, None),
        "pagedown" => KeyIdentity::named("PageDown", "PageDown", 34, None),
        "left" => KeyIdentity::named("ArrowLeft", "ArrowLeft", 37, None),
        "up" => KeyIdentity::named("ArrowUp", "ArrowUp", 38, None),
        "right" => KeyIdentity::named("ArrowRight", "ArrowRight", 39, None),
        "down" => KeyIdentity::named("ArrowDown", "ArrowDown", 40, None),
        _ => {
            return Err(format!(
                "unknown key {name:?}: use a letter, digit, punctuation key, or one of return, \
                 tab, escape, space, backspace, delete, insert, home, end, pageup, pagedown, \
                 left, up, right, down, f1-f12"
            ))
        }
    })
}

/// Editing chords a macOS browser only performs when the key event names the
/// editor command: Cocoa, not the renderer, maps Cmd+A and friends to
/// commands, and a CDP key event bypasses Cocoa. Other platforms map them in
/// the renderer, where naming the command as well would run it twice.
fn macos_editing_command(code: &str, modifiers: i64) -> Option<&'static str> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    match (code, modifiers) {
        ("KeyA", META) => Some("selectAll"),
        ("KeyC", META) => Some("copy"),
        ("KeyX", META) => Some("cut"),
        ("KeyV", META) => Some("paste"),
        ("KeyZ", META) => Some("undo"),
        ("KeyZ", m) if m == META | SHIFT => Some("redo"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct KeyPress {
    key: KeyIdentity,
    /// Modifiers in the order they are pressed; released in reverse.
    modifiers: Vec<Modifier>,
}

impl KeyPress {
    fn parse(key: &str, modifiers: &[String]) -> Result<Self, String> {
        let mut parsed: Vec<Modifier> = Vec::new();
        for name in modifiers {
            let modifier = Modifier::parse(name)?;
            if !parsed.contains(&modifier) {
                parsed.push(modifier);
            }
        }
        if Modifier::parse(key).is_ok() {
            return Err(format!(
                "key {key:?} is a modifier: pass it in modifiers with the key it modifies"
            ));
        }
        Ok(Self {
            key: key_identity(key)?,
            modifiers: parsed,
        })
    }

    fn mask(&self) -> i64 {
        self.modifiers.iter().fold(0, |mask, m| mask | m.bit())
    }

    /// The `Input.dispatchKeyEvent` parameters of a whole press, in dispatch
    /// order: each modifier down, the key down and up, then each modifier up.
    #[cfg(test)]
    fn events(&self) -> Vec<Value> {
        let mut events = self.down_events();
        events.extend(self.release_events(self.modifiers.len(), true));
        events
    }

    fn dom_key(&self) -> &str {
        if self.mask() & SHIFT != 0 {
            &self.key.shifted_key
        } else {
            &self.key.key
        }
    }

    /// Each modifier down, then the key down. Every event carries the
    /// modifier state the page observes at that point.
    fn down_events(&self) -> Vec<Value> {
        let mask = self.mask();
        let mut events = Vec::new();
        let mut held = 0;
        for modifier in &self.modifiers {
            held |= modifier.bit();
            let (key, code, virtual_key) = modifier.identity();
            events.push(json!({
                "type": "rawKeyDown",
                "modifiers": held,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": virtual_key,
                "nativeVirtualKeyCode": virtual_key,
            }));
        }
        // A command modifier turns a printable key into a shortcut: no text.
        let text = if mask & (CTRL | ALT | META) == 0 {
            if mask & SHIFT != 0 {
                self.key.shifted_text.as_ref()
            } else {
                self.key.text.as_ref()
            }
        } else {
            None
        };
        let mut down = json!({
            "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
            "modifiers": mask,
            "key": self.dom_key(),
            "code": self.key.code,
            "windowsVirtualKeyCode": self.key.virtual_key,
            "nativeVirtualKeyCode": self.key.virtual_key,
        });
        if let Some(text) = text {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        if let Some(command) = macos_editing_command(&self.key.code, mask) {
            down["commands"] = json!([command]);
        }
        events.push(down);
        events
    }

    /// The releases for a press whose first `held` modifiers went down: the
    /// key up when the key went down too, then those modifiers up in reverse.
    fn release_events(&self, held: usize, key_sent: bool) -> Vec<Value> {
        let mut mask = self.modifiers[..held]
            .iter()
            .fold(0, |mask, m| mask | m.bit());
        let mut events = Vec::new();
        if key_sent {
            events.push(json!({
                "type": "keyUp",
                "modifiers": mask,
                "key": self.dom_key(),
                "code": self.key.code,
                "windowsVirtualKeyCode": self.key.virtual_key,
                "nativeVirtualKeyCode": self.key.virtual_key,
            }));
        }
        for modifier in self.modifiers[..held].iter().rev() {
            mask &= !modifier.bit();
            let (key, code, virtual_key) = modifier.identity();
            events.push(json!({
                "type": "keyUp",
                "modifiers": mask,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": virtual_key,
                "nativeVirtualKeyCode": virtual_key,
            }));
        }
        events
    }
}

/// The reply to a focus-emulation restore can be lost (a blocked renderer, a
/// timed-out call), so it is bounded and only reports whether it was confirmed.
async fn disable_focus_emulation(conn: &CdpConnection, cdp: &str, timeout: Duration) -> bool {
    conn.call_with_timeout(
        Some(cdp),
        "Emulation.setFocusEmulationEnabled",
        json!({ "enabled": false }),
        timeout,
    )
    .await
    .is_ok()
}

const FOCUS_RESTORE_TIMEOUT: Duration = Duration::from_secs(2);
/// A page-owned dialog blocks the renderer, which applies the restore once the
/// dialog closes; do not wait for that reply.
const BLOCKED_FOCUS_RESTORE_TIMEOUT: Duration = Duration::from_millis(50);

/// What happened to the key itself.
enum KeyDelivery {
    /// The key down was acknowledged, or a page dialog opened while it was
    /// in flight and is blocking the reply.
    Pressed,
    /// The key down was sent but neither acknowledged nor followed by a
    /// dialog: it may have reached the page.
    Unknown(anyhow::Error),
    /// A modifier down failed, so the key down was never sent.
    NotPressed(anyhow::Error),
    /// A dialog opened before the key down was sent, so it never was.
    DialogBeforeKey,
}

pub struct BrowserPressKeyTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserPressKeyTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        let def = ToolDef {
            name: "browser_press_key".into(),
            description: "Press one key or chord in an exactly-bound tab through CDP \
                Input.dispatchKeyEvent, without activating the tab or raising the \
                browser window. Use it where native press_key or hotkey cannot reach a \
                background browser (Chromium ignores posted modifier state). key and \
                modifiers take the press_key vocabulary: key names (return, tab, escape, \
                arrows, f1-f12), a letter, digit, or punctuation key; modifiers ctrl, \
                shift, alt, meta (cmd/win). Pass a ref to focus that element first; \
                without one the key goes to the page's current focus. A chord reaches \
                browser shortcuts too (ctrl+w closes the tab, f5 reloads), so refs are \
                invalidated after every press. Heuristic bindings are refused."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target_id": schema_target_id(),
                    "tab_id": schema_tab_id(),
                    "session": schema_session(),
                    "key": {
                        "type": "string",
                        "description": "Key to press: return, tab, escape, space, backspace, \
                            delete, insert, home, end, pageup, pagedown, left, up, right, \
                            down, f1-f12, a letter, a digit, or a punctuation key."
                    },
                    "modifiers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Modifiers held while the key is pressed: ctrl, \
                            shift, alt, meta (cmd/win)."
                    },
                    "ref": schema_ref(),
                },
                "required": ["target_id", "tab_id", "key"],
                "additionalProperties": false
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        };
        Self { def, engine }
    }
}

impl BrowserPressKeyTool {
    /// A page dialog opened while the page was being focused, before any key
    /// was sent.
    async fn dialog_before_keys(
        &self,
        conn: &CdpConnection,
        cdp: &str,
        session: &str,
        target_id: &str,
        tab_id: &str,
        open: &super::cdp_ws::CdpDialogState,
    ) -> ToolResult {
        let restored = disable_focus_emulation(conn, cdp, BLOCKED_FOCUS_RESTORE_TIMEOUT).await;
        self.engine
            .store
            .invalidate_tab_snapshots(session, target_id, tab_id);
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserInputIncomplete,
            "a page dialog opened before any key was sent, so the key was not pressed; resolve \
             the dialog with browser_dialog first",
        )
        .with_detail(json!({
            "pressed": false,
            "released": true,
            "refs_invalidated": true,
            "focus_emulation_restored": restored,
            "retryable": false,
            "dialog": dialog_block(open),
        }))
        .to_tool_result()
    }
}

#[async_trait]
impl Tool for BrowserPressKeyTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        if adapter_id == "browser_bound_input" {
            browser_resource_ownership(&self.engine, args).await
        } else {
            ProtectedResourceOwnership::UserOwned
        }
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, crate::tool::ProtectedResourceScopeError> {
        if adapter_id == "browser_bound_input" {
            browser_protected_resource_scope(&self.engine, args, "browser_press_key").await
        } else {
            Ok(None)
        }
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, key_name) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("key"),
        ) {
            (Ok(t), Ok(tab), Ok(k)) => (t, tab, k),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        let session = match require_explicit_session(&args) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let modifier_names: Vec<String> = match args.get("modifiers") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                match items
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                {
                    Some(names) => names,
                    None => return ToolResult::error("modifiers must be an array of strings"),
                }
            }
            Some(_) => return ToolResult::error("modifiers must be an array of strings"),
        };
        let press = match KeyPress::parse(&key_name, &modifier_names) {
            Ok(press) => press,
            Err(message) => return ToolResult::error(message),
        };
        let ext_ref = args.opt_str("ref");

        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(v) => v,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let conn = &validated.conn;

        // A ref routes the keys to its own frame's session (the tab, or the
        // OOPIF child that contains it) after re-proving the frame identity.
        let (cdp_session, focus_node) = match &ext_ref {
            Some(ext_ref) => {
                let entry = match self
                    .engine
                    .store
                    .resolve_ref(&session, &target_id, &tab_id, ext_ref)
                {
                    Ok(e) => e,
                    Err(refusal) => return refusal.to_tool_result(),
                };
                match self
                    .engine
                    .frame_session_for_mutation(
                        &session,
                        &target_id,
                        &tab_id,
                        &validated,
                        &entry.frame,
                    )
                    .await
                {
                    Ok(s) => (s, Some(entry.backend_node_id)),
                    Err(refusal) => return refusal.to_tool_result(),
                }
            }
            None => (validated.cdp_session.clone(), None),
        };
        let cdp = cdp_session.as_str();

        let cdp_target_id = validated.tab.cdp_target_id.as_str();

        // An open page dialog blocks the renderer: no key would be handled
        // until it closes. Refuse before touching the page.
        if let Some(open) = conn.dialog_state(cdp_target_id) {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!(
                    "a {} dialog is open on the exact tab; resolve it with browser_dialog before \
                     pressing keys",
                    open.kind
                ),
            )
            .with_detail(json!({ "dialog": dialog_block(&open) }))
            .to_tool_result();
        }

        // Focus emulation lets a tab whose window is in the background, or
        // that is not the active tab, process the keys as a focused page.
        // Gaining focus runs the page's focus handlers, which can open a
        // dialog that blocks the reply, so both steps watch for one.
        let enable = conn.call(
            Some(cdp),
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        );
        match call_until_dialog(conn, cdp_target_id, enable).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                // The command may have applied even though its reply was lost.
                let restored = disable_focus_emulation(conn, cdp, FOCUS_RESTORE_TIMEOUT).await;
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserInputTrustUnavailable,
                    format!("the tab could not enter CDP focus emulation for key input: {error}"),
                )
                .with_detail(json!({ "pressed": false, "focus_emulation_restored": restored }))
                .to_tool_result();
            }
            Err(open) => {
                return self
                    .dialog_before_keys(conn, cdp, &session, &target_id, &tab_id, &open)
                    .await
            }
        }
        if let Some(backend_node_id) = focus_node {
            let focus = conn.call(
                Some(cdp),
                "DOM.focus",
                json!({ "backendNodeId": backend_node_id }),
            );
            match call_until_dialog(conn, cdp_target_id, focus).await {
                Ok(Ok(_)) => {}
                Ok(Err(_)) => {
                    let restored = disable_focus_emulation(conn, cdp, FOCUS_RESTORE_TIMEOUT).await;
                    return BrowserRefusal::new(
                        BrowserRefusalCode::BrowserRefStale,
                        "the ref's node can no longer be focused — re-snapshot the tab",
                    )
                    .with_detail(json!({ "pressed": false, "focus_emulation_restored": restored }))
                    .to_tool_result();
                }
                Err(open) => {
                    return self
                        .dialog_before_keys(conn, cdp, &session, &target_id, &tab_id, &open)
                        .await
                }
            }
        }

        // Key downs stop at the first failure or dialog. Every modifier that
        // may have gone down, the unacknowledged one included, is then
        // released, and the key too once its down was sent: a blocked
        // renderer applies the releases when its dialog closes.
        let downs = press.down_events();
        let key_down_index = press.modifiers.len();
        let mut delivery = KeyDelivery::Pressed;
        let mut opened = None;
        let mut sent = 0;
        for (index, event) in downs.into_iter().enumerate() {
            // A handler can open its dialog after its key's reply came back;
            // no further key down may queue behind it.
            if let Some(open) = conn.dialog_state(cdp_target_id) {
                opened = Some(open);
                delivery = KeyDelivery::DialogBeforeKey;
                break;
            }
            sent = index + 1;
            let call = conn.call(Some(cdp), "Input.dispatchKeyEvent", event);
            match call_until_dialog(conn, cdp_target_id, call).await {
                Ok(Ok(_)) => continue,
                Err(open) => {
                    opened = Some(open);
                    if index < key_down_index {
                        delivery = KeyDelivery::DialogBeforeKey;
                    }
                }
                Ok(Err(error)) if index < key_down_index => {
                    delivery = KeyDelivery::NotPressed(error)
                }
                Ok(Err(error)) => delivery = KeyDelivery::Unknown(error),
            }
            break;
        }
        let key_sent = sent > key_down_index;
        let held = sent.min(key_down_index);
        // Every release must be acknowledged for the keys to count as up.
        let mut released = true;
        for event in press.release_events(held, key_sent) {
            let call = conn.call(Some(cdp), "Input.dispatchKeyEvent", event);
            match call_until_dialog(conn, cdp_target_id, call).await {
                Ok(Ok(_)) => {}
                Ok(Err(_)) => released = false,
                Err(open) => {
                    released = false;
                    opened.get_or_insert(open);
                }
            }
        }
        let opened = opened.or_else(|| conn.dialog_state(cdp_target_id));
        let restore_timeout = if opened.is_some() {
            BLOCKED_FOCUS_RESTORE_TIMEOUT
        } else {
            FOCUS_RESTORE_TIMEOUT
        };
        let focus_emulation_restored = disable_focus_emulation(conn, cdp, restore_timeout).await;
        // Whatever reached the page may have reloaded, navigated, or edited it.
        self.engine
            .store
            .invalidate_tab_snapshots(&session, &target_id, &tab_id);

        let refusal = match delivery {
            KeyDelivery::Pressed => None,
            KeyDelivery::NotPressed(error) => Some((
                format!("the key was not pressed: {error}"),
                json!({ "pressed": false }),
            )),
            KeyDelivery::DialogBeforeKey => Some((
                "a page dialog opened before the key was sent, so the key was not pressed; \
                 resolve the dialog with browser_dialog first"
                    .to_owned(),
                json!({ "pressed": false }),
            )),
            KeyDelivery::Unknown(error) => Some((
                format!(
                    "the key down was sent but not acknowledged ({error}); it may have reached \
                     the page, so read the page before pressing it again"
                ),
                json!({ "pressed": "unknown", "delivery": "unknown" }),
            )),
        };
        if let Some((message, mut detail)) = refusal {
            detail["released"] = json!(released);
            detail["refs_invalidated"] = json!(true);
            detail["focus_emulation_restored"] = json!(focus_emulation_restored);
            detail["retryable"] = json!(false);
            if let Some(open) = &opened {
                detail["dialog"] = dialog_block(open);
            }
            return BrowserRefusal::new(BrowserRefusalCode::BrowserInputIncomplete, message)
                .with_detail(detail)
                .to_tool_result();
        }

        let modifiers: Vec<&str> = press.modifiers.iter().map(|m| m.name()).collect();
        let chord = modifiers
            .iter()
            .copied()
            .chain(std::iter::once(key_name.as_str()))
            .collect::<Vec<_>>()
            .join("+");
        let mut structured = json!({
            "status": "ok",
            "target_id": target_id,
            "tab_id": tab_id,
            "key": key_name,
            "modifiers": modifiers,
            "pressed": true,
            "released": released,
            "refs_invalidated": true,
        });
        if let Some(ext_ref) = ext_ref {
            structured["ref"] = json!(ext_ref);
        }
        if !focus_emulation_restored {
            // Behind a dialog the restore is applied once the dialog closes.
            structured["focus_emulation_restored"] = json!(false);
        }
        let Some(open) = opened else {
            return ToolResult::text(format!("pressed {chord} in {tab_id}"))
                .with_structured(structured);
        };
        // The dialog blocks the renderer, so nothing can confirm the effect.
        structured["effect"] = json!("unverifiable");
        structured["dialog"] = dialog_block(&open);
        structured["escalation"] = json!({
            "recommended": "page",
            "reason": "a page-owned JavaScript dialog opened during the key press; resolve it with browser_dialog, then refresh page state",
        });
        ToolResult::text(format!(
            "pressed {chord} in {tab_id}; a {} dialog opened during the press and is blocking \
             the page. Inspect and resolve it with browser_dialog (dialog_id {}) before any \
             other action on this tab",
            open.kind,
            dialog_capability_id(open.generation)
        ))
        .with_structured(structured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: &str, modifiers: &[&str]) -> KeyPress {
        let modifiers: Vec<String> = modifiers.iter().map(|m| m.to_string()).collect();
        KeyPress::parse(key, &modifiers).expect("valid key press")
    }

    #[test]
    fn a_chord_holds_each_modifier_around_the_key_with_the_cdp_bitmask() {
        let events = press("a", &["ctrl", "shift"]).events();
        let summary: Vec<(String, String, i64)> = events
            .iter()
            .map(|event| {
                (
                    event["type"].as_str().unwrap().to_owned(),
                    event["key"].as_str().unwrap().to_owned(),
                    event["modifiers"].as_i64().unwrap(),
                )
            })
            .collect();
        let expected: Vec<(String, String, i64)> = [
            ("rawKeyDown", "Control", CTRL),
            ("rawKeyDown", "Shift", CTRL | SHIFT),
            ("rawKeyDown", "A", CTRL | SHIFT),
            ("keyUp", "A", CTRL | SHIFT),
            ("keyUp", "Shift", CTRL),
            ("keyUp", "Control", 0),
        ]
        .into_iter()
        .map(|(t, k, m)| (t.to_owned(), k.to_owned(), m))
        .collect();
        assert_eq!(summary, expected);
        assert_eq!(events[2]["code"], "KeyA");
        assert_eq!(events[2]["windowsVirtualKeyCode"], 65);
        assert!(
            events[2].get("text").is_none(),
            "a shortcut inserts no text"
        );
    }

    #[test]
    fn a_printable_key_without_command_modifiers_carries_its_text() {
        let plain = press("a", &[]).events();
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[0]["type"], "keyDown");
        assert_eq!(plain[0]["text"], "a");

        let shifted = press("1", &["shift"]).events();
        assert_eq!(shifted[1]["type"], "keyDown");
        assert_eq!(shifted[1]["key"], "!");
        assert_eq!(shifted[1]["text"], "!");
        assert_eq!(shifted[1]["code"], "Digit1");

        let enter = press("return", &[]).events();
        assert_eq!(enter[0]["key"], "Enter");
        assert_eq!(enter[0]["windowsVirtualKeyCode"], 13);
        assert_eq!(enter[0]["text"], "\r");
    }

    #[test]
    fn named_keys_have_their_dom_identity() {
        for (name, key, code, virtual_key) in [
            ("f5", "F5", "F5", 116),
            ("F12", "F12", "F12", 123),
            ("escape", "Escape", "Escape", 27),
            ("left", "ArrowLeft", "ArrowLeft", 37),
            ("pagedown", "PageDown", "PageDown", 34),
            ("delete", "Delete", "Delete", 46),
            ("/", "/", "Slash", 191),
        ] {
            let events = press(name, &[]).events();
            assert_eq!(events[0]["key"], key, "{name}");
            assert_eq!(events[0]["code"], code, "{name}");
            assert_eq!(events[0]["windowsVirtualKeyCode"], virtual_key, "{name}");
        }
        assert_eq!(press("f5", &[]).events()[0]["type"], "rawKeyDown");
    }

    #[test]
    fn modifier_aliases_collapse_and_unknown_names_are_refused() {
        assert_eq!(
            press("t", &["control", "ctrl", "cmd"]).modifiers,
            vec![Modifier::Ctrl, Modifier::Meta]
        );
        assert_eq!(
            KeyPress::parse("a", &["hyper".into()]).unwrap_err(),
            "unknown modifier \"hyper\": use ctrl, shift, alt, or meta (cmd/win)"
        );
        assert!(KeyPress::parse("f13", &[])
            .unwrap_err()
            .starts_with("unknown key \"f13\""));
        assert_eq!(
            KeyPress::parse("shift", &[]).unwrap_err(),
            "key \"shift\" is a modifier: pass it in modifiers with the key it modifies"
        );
    }

    #[test]
    fn macos_editing_chords_name_their_command_and_other_platforms_do_not() {
        let events = press("a", &["meta"]).events();
        if cfg!(target_os = "macos") {
            assert_eq!(events[1]["commands"], json!(["selectAll"]));
        } else {
            assert!(events[1].get("commands").is_none());
        }
        assert!(press("a", &["ctrl"]).events()[1].get("commands").is_none());
    }
}
