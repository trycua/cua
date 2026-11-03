//! Background drag for Electron windows through the app's main process.
//!
//! A window-routed CGEvent drag reaches an Electron page, but Chromium builds
//! each web mouse event's held-button modifiers from
//! `+[NSEvent pressedMouseButtons]`, which reads the HID system button state
//! (`CGSInputButtonState`). A press posted to the process does not change that
//! state. `pointerdown` still reports the pressed button, so the page captures
//! the pointer, but every `mouseDragged` arrives as a `pointermove` with
//! `buttons == 0`. The pointer leaves the "active buttons" state, Blink fires
//! `lostpointercapture` on the first move, and the release is dispatched to
//! the element under the drop point instead of the capturing element: the drop
//! never lands. Widgets that test `event.buttons` during the move fail the
//! same way.
//!
//! This route sends the gesture from the Electron main process instead
//! (through its Node inspector, as the background hover route does), where the
//! button state is part of the event:
//!
//! 1. `webContents.debugger` and CDP `Input.dispatchMouseEvent` with `buttons`
//!    set. `Input.setInterceptDrags` keeps an HTML5 drag-and-drop gesture out
//!    of the native drag session (which follows the real pointer); the drop is
//!    finished with `Input.dispatchDragEvent` at the drop point.
//! 2. When the debugger cannot attach, `webContents.sendInputEvent` with the
//!    `leftButtonDown` (or right/middle) modifier on the press and moves.
//!
//! Neither path moves the real pointer, raises the window or takes focus.

/// Mouse button for the Electron drag route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectronDragButton {
    Left,
    Right,
    Middle,
}

impl ElectronDragButton {
    fn cdp_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }

    /// CDP `buttons` bitmask while this button is held.
    fn cdp_mask(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
        }
    }

    /// Electron `sendInputEvent` modifier that marks this button as held.
    fn held_modifier(self) -> &'static str {
        match self {
            Self::Left => "leftButtonDown",
            Self::Right => "rightButtonDown",
            Self::Middle => "middleButtonDown",
        }
    }
}

/// One drag gesture in window-local points (top-left of the window frame,
/// title bar included), as the pixel tools resolve them.
#[derive(Clone, Debug, PartialEq)]
pub struct ElectronDragSpec {
    pub window_id: u32,
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub steps: usize,
    pub duration_ms: u64,
    pub button: ElectronDragButton,
    pub modifiers: Vec<String>,
}

/// The inspector evaluation times out after 10 s; keep the gesture well
/// inside it.
pub const MAX_DURATION_MS: u64 = 5_000;

/// CDP `modifiers` bitmask and Electron modifier names for the held keys.
fn modifier_sets(modifiers: &[String]) -> (u8, Vec<&'static str>) {
    let mut mask = 0;
    let mut names = Vec::new();
    for m in modifiers {
        let (bit, name) = match m.to_ascii_lowercase().as_str() {
            "option" | "alt" | "opt" => (1, "alt"),
            "ctrl" | "control" => (2, "control"),
            "cmd" | "command" | "meta" | "super" => (4, "meta"),
            "shift" => (8, "shift"),
            _ => continue,
        };
        if mask & bit == 0 {
            mask |= bit;
            names.push(name);
        }
    }
    (mask, names)
}

/// Main-process JavaScript that performs the drag in the BrowserWindow whose
/// native window number is `spec.window_id`. Resolves to a JSON string:
/// `{"ok":true,"route":"cdp"|"send_input_event","drag_intercepted":bool,...}`
/// or `{"ok":false,"reason":..}`.
pub fn electron_drag_js(spec: &ElectronDragSpec) -> String {
    let (cdp_modifiers, names) = modifier_sets(&spec.modifiers);
    let held_keys = serde_json::to_string(&names).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(async () => {{
  const req = (typeof require === 'function') ? require : (process.mainModule && process.mainModule.require.bind(process.mainModule));
  if (!req) return JSON.stringify({{ ok: false, reason: 'no_require' }});
  const {{ BrowserWindow }} = req('electron');
  const w = BrowserWindow.getAllWindows().find((w) => {{
    try {{ return String(w.getMediaSourceId()).split(':')[1] === '{wid}'; }} catch (e) {{ return false; }}
  }});
  if (!w) return JSON.stringify({{ ok: false, reason: 'no_window' }});
  const wc = w.webContents;
  const b = w.getBounds(), c = w.getContentBounds();
  const ox = c.x - b.x, oy = c.y - b.y;
  const local = (p) => [p[0] - ox, p[1] - oy];
  const from = local([{fx}, {fy}]), to = local([{tx}, {ty}]);
  for (const p of [from, to]) {{
    if (p[0] < 0 || p[1] < 0 || p[0] >= c.width || p[1] >= c.height) return JSON.stringify({{ ok: false, reason: 'outside_content' }});
  }}
  const steps = {steps}, stepDelay = Math.floor({duration} / steps);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const path = [];
  for (let i = 1; i <= steps; i++) path.push([from[0] + (to[0] - from[0]) * i / steps, from[1] + (to[1] - from[1]) * i / steps]);
  const dbg = wc.debugger;
  let attached = false;
  try {{ if (!dbg.isAttached()) {{ dbg.attach('1.3'); attached = true; }} }} catch (e) {{}}
  if (dbg.isAttached()) {{
    let drag = null;
    const onMessage = (_e, method, params) => {{ if (method === 'Input.dragIntercepted') drag = params.data; }};
    dbg.on('message', onMessage);
    const zoom = wc.getZoomFactor() || 1;
    const css = (p) => ({{ x: p[0] / zoom, y: p[1] / zoom }});
    const mouse = (type, p, held, extra) => dbg.sendCommand('Input.dispatchMouseEvent', Object.assign({{
      type, ...css(p), modifiers: {cdp_mods}, button: '{button}', buttons: held ? {mask} : 0, clickCount: 1, pointerType: 'mouse' }}, extra || {{}}));
    const dragEvent = (type, p) => dbg.sendCommand('Input.dispatchDragEvent', {{ type, ...css(p), data: drag, modifiers: {cdp_mods} }});
    let dragged = false, dropped = false;
    try {{
      await dbg.sendCommand('Input.setInterceptDrags', {{ enabled: true }});
      await mouse('mouseMoved', from, false, {{ button: 'none', clickCount: 0 }});
      await mouse('mousePressed', from, true);
      await sleep(30);
      for (const p of path) {{
        if (drag) {{
          if (!dragged) {{ await dragEvent('dragEnter', p); dragged = true; }}
          await dragEvent('dragOver', p);
        }} else {{
          await mouse('mouseMoved', p, true, {{ clickCount: 0 }});
        }}
        await sleep(stepDelay);
      }}
      await sleep(30);
      if (drag) {{
        if (!dragged) await dragEvent('dragEnter', to);
        await dragEvent('dragOver', to);
        await dragEvent('drop', to);
        dropped = true;
      }}
      await mouse('mouseReleased', to, false);
    }} finally {{
      dbg.removeListener('message', onMessage);
      try {{ await dbg.sendCommand('Input.setInterceptDrags', {{ enabled: false }}); }} catch (e) {{}}
      if (attached) {{ try {{ dbg.detach(); }} catch (e) {{}} }}
    }}
    return JSON.stringify({{ ok: true, route: 'cdp', drag_intercepted: !!drag, html5_drop: dropped }});
  }}
  const keys = {held_keys};
  const send = (type, p, held, extra) => wc.sendInputEvent(Object.assign({{
    type, x: Math.round(p[0]), y: Math.round(p[1]), button: '{button}',
    modifiers: held ? keys.concat(['{held}']) : keys }}, extra || {{}}));
  send('mouseMove', from, false);
  send('mouseDown', from, true, {{ clickCount: 1 }});
  await sleep(30);
  for (const p of path) {{ send('mouseMove', p, true); await sleep(stepDelay); }}
  await sleep(30);
  send('mouseUp', to, false, {{ clickCount: 1 }});
  return JSON.stringify({{ ok: true, route: 'send_input_event', drag_intercepted: false, html5_drop: false }});
}})()"#,
        wid = spec.window_id,
        fx = spec.from.0,
        fy = spec.from.1,
        tx = spec.to.0,
        ty = spec.to.1,
        steps = spec.steps.clamp(1, 200),
        duration = spec.duration_ms.min(MAX_DURATION_MS),
        cdp_mods = cdp_modifiers,
        button = spec.button.cdp_name(),
        mask = spec.button.cdp_mask(),
        held = spec.button.held_modifier(),
        held_keys = held_keys,
    )
}

/// What the main process reported for a drag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElectronDragReply {
    Delivered {
        route: String,
        drag_intercepted: bool,
        html5_drop: bool,
    },
    Refused {
        reason: String,
    },
}

/// Parse the main-process reply, raw or as a quoted JSON string.
pub fn parse_electron_drag_reply(reply: &str) -> Option<ElectronDragReply> {
    let trimmed = reply.trim();
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let value = match value {
        serde_json::Value::String(inner) => serde_json::from_str(&inner).ok()?,
        other => other,
    };
    if value.get("ok")?.as_bool()? {
        Some(ElectronDragReply::Delivered {
            route: value
                .get("route")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_owned(),
            drag_intercepted: value
                .get("drag_intercepted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            html5_drop: value
                .get("html5_drop")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    } else {
        Some(ElectronDragReply::Refused {
            reason: value
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ElectronDragSpec {
        ElectronDragSpec {
            window_id: 4242,
            from: (120.0, 140.5),
            to: (360.0, 150.0),
            steps: 20,
            duration_ms: 400,
            button: ElectronDragButton::Left,
            modifiers: vec!["shift".into(), "cmd".into(), "Shift".into()],
        }
    }

    #[test]
    fn js_targets_the_window_and_holds_the_button_through_the_gesture() {
        let js = electron_drag_js(&spec());
        assert!(js.contains("split(':')[1] === '4242'"));
        assert!(js.contains("local([120, 140.5])"));
        assert!(js.contains("local([360, 150])"));
        // CDP: the press and moves carry the held button, the release does not.
        assert!(js.contains("buttons: held ? 1 : 0"));
        assert!(js.contains("mouse('mousePressed', from, true)"));
        assert!(js.contains("mouse('mouseReleased', to, false)"));
        // HTML5 drag and drop is intercepted and finished with drag events.
        assert!(js.contains("Input.setInterceptDrags"));
        assert!(js.contains("dragEvent('drop', to)"));
        // Fallback: Electron's own held-button modifier.
        assert!(js.contains("keys.concat(['leftButtonDown'])"));
        // Shift + Cmd once each: CDP mask 8 | 4, Electron names.
        assert!(js.contains("modifiers: 12"));
        assert!(js.contains(r#"const keys = ["shift","meta"]"#));
    }

    #[test]
    fn js_bounds_the_gesture_inside_the_inspector_timeout() {
        let mut long = spec();
        long.duration_ms = 10_000;
        long.steps = 500;
        let js = electron_drag_js(&long);
        assert!(js.contains("const steps = 200"));
        assert!(js.contains("Math.floor(5000 / steps)"));
    }

    #[test]
    fn right_and_middle_buttons_use_their_own_masks() {
        let mut s = spec();
        s.button = ElectronDragButton::Right;
        let js = electron_drag_js(&s);
        assert!(js.contains("button: 'right'"));
        assert!(js.contains("buttons: held ? 2 : 0"));
        assert!(js.contains("'rightButtonDown'"));
        s.button = ElectronDragButton::Middle;
        let js = electron_drag_js(&s);
        assert!(js.contains("buttons: held ? 4 : 0"));
        assert!(js.contains("'middleButtonDown'"));
    }

    #[test]
    fn replies_parse_raw_and_quoted() {
        assert_eq!(
            parse_electron_drag_reply(
                r#"{"ok":true,"route":"cdp","drag_intercepted":true,"html5_drop":true}"#
            ),
            Some(ElectronDragReply::Delivered {
                route: "cdp".into(),
                drag_intercepted: true,
                html5_drop: true
            })
        );
        assert_eq!(
            parse_electron_drag_reply(
                r#""{\"ok\":true,\"route\":\"send_input_event\",\"drag_intercepted\":false}""#
            ),
            Some(ElectronDragReply::Delivered {
                route: "send_input_event".into(),
                drag_intercepted: false,
                html5_drop: false
            })
        );
        assert_eq!(
            parse_electron_drag_reply(r#"{"ok":false,"reason":"no_window"}"#),
            Some(ElectronDragReply::Refused {
                reason: "no_window".into()
            })
        );
        assert_eq!(parse_electron_drag_reply("not json"), None);
    }
}
