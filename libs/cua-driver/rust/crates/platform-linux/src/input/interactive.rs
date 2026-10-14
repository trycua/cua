//! Low-latency, stateful input delivery to one X11 target (window or display
//! region): the Linux native session behind
//! [`cua_driver_core::interactive_input`].
//!
//! The session keeps one X connection and keyboard map for its lifetime, so a
//! streaming client's pointer samples do not pay a connect per event.
//!
//! - `PersistentForeground`: the target window is activated when the session
//!   opens (and again when something else took the focus), and events go
//!   through XTest into the focused root, exactly like physical input.
//! - `Background`: events are XSendEvent-addressed to the target window
//!   without moving the pointer or focus. Opening a background session on a
//!   window whose toolkit drops synthetic events (GTK3/4, Chromium) is refused
//!   with [`InteractiveInputError::WouldRequireActivation`] instead of
//!   producing input that is reported as dispatched and never arrives.
//!
//! Every batch ends with an X round-trip, so the receipt means the server
//! processed the events. Held keys and buttons are released when the session
//! is released or dropped. Wayland has no per-window targeting and is refused
//! at open.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Instant;

use cua_driver_core::interactive_input::{
    denormalize, extract_integral, validate_batch, InteractiveDeliveryMode, InteractiveInputBatch,
    InteractiveInputError, InteractiveInputEvent, InteractiveInputReceipt, KeyState, Modifier,
    PointerButton, PointerPhase, TargetedInputError,
};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use super::targeted::{self as x, KeyboardMap};

/// Pixels per wheel click for precise (trackpad) deltas.
const PRECISE_PIXELS_PER_CLICK: f64 = 40.0;

#[derive(Debug, Clone)]
pub struct InteractiveInputConfig {
    /// Target window, or `None` for a display region.
    pub window: Option<u32>,
    /// Display region in root coordinates `(x, y, width, height)`, used when
    /// `window` is `None`.
    pub region: Option<(i32, i32, u32, u32)>,
    pub delivery_mode: InteractiveDeliveryMode,
}

struct State {
    conn: RustConnection,
    root: Window,
    keymap: KeyboardMap,
    held_keys: HashSet<String>,
    held_buttons: HashSet<u8>,
    last_point: (i32, i32),
    scroll_residual_x: f64,
    scroll_residual_y: f64,
}

pub struct InteractiveInputSession {
    config: InteractiveInputConfig,
    state: Mutex<Option<State>>,
}

fn native(error: TargetedInputError) -> InteractiveInputError {
    match error {
        TargetedInputError::WouldRequireActivation(message) => {
            InteractiveInputError::WouldRequireActivation(message)
        }
        TargetedInputError::TargetUnavailable(message)
        | TargetedInputError::Unsupported(message) => InteractiveInputError::InvalidTarget(message),
        TargetedInputError::DeliveryFailed(message) => InteractiveInputError::Native(message),
    }
}

impl InteractiveInputSession {
    pub fn open(config: InteractiveInputConfig) -> Result<Self, InteractiveInputError> {
        if config.window.is_none() && config.region.is_none() {
            return Err(InteractiveInputError::InvalidTarget(
                "a window or a display region is required".into(),
            ));
        }
        let (conn, root) = x::connect().map_err(native)?;
        if let Some(window) = config.window {
            conn.get_geometry(window)
                .map_err(|e| InteractiveInputError::Native(e.to_string()))?
                .reply()
                .map_err(|_| InteractiveInputError::InvalidTarget("window is gone".into()))?;
            if config.delivery_mode == InteractiveDeliveryMode::Background {
                if let Some(toolkit) =
                    x::window_pid(&conn, window).and_then(x::synthetic_input_ignored_by)
                {
                    return Err(InteractiveInputError::WouldRequireActivation(format!(
                        "{toolkit} ignores synthetic (XSendEvent) input; open the session with \
                         persistent foreground delivery"
                    )));
                }
            }
        } else if config.delivery_mode == InteractiveDeliveryMode::Background {
            return Err(InteractiveInputError::WouldRequireActivation(
                "a display region has no window to address background input to".into(),
            ));
        }
        let keymap = KeyboardMap::load(&conn).map_err(native)?;
        let session = Self {
            config,
            state: Mutex::new(Some(State {
                conn,
                root,
                keymap,
                held_keys: HashSet::new(),
                held_buttons: HashSet::new(),
                last_point: (0, 0),
                scroll_residual_x: 0.0,
                scroll_residual_y: 0.0,
            })),
        };
        {
            let guard = session.state.lock().unwrap();
            let state = guard.as_ref().expect("fresh session state");
            session.prepare(state).map_err(native)?;
        }
        Ok(session)
    }

    fn is_foreground(&self) -> bool {
        self.config.delivery_mode == InteractiveDeliveryMode::PersistentForeground
    }

    /// Foreground sessions keep the target active; paid when the session opens
    /// and only again when another window took the focus.
    fn prepare(&self, state: &State) -> Result<(), TargetedInputError> {
        if let (true, Some(window)) = (self.is_foreground(), self.config.window) {
            if x::active_window(&state.conn, state.root) != Some(window) {
                x::activate(&state.conn, state.root, window)?;
            }
        }
        Ok(())
    }

    /// Deliver one ordered batch and wait until the X server processed it.
    pub fn dispatch(
        &self,
        batch: &InteractiveInputBatch,
    ) -> Result<InteractiveInputReceipt, InteractiveInputError> {
        let started = Instant::now();
        let through = validate_batch(batch)?;
        let mut guard = self.state.lock().unwrap();
        let state = guard.as_mut().ok_or(InteractiveInputError::Closed)?;
        self.prepare(state).map_err(native)?;
        for event in &batch.events {
            self.dispatch_event(state, event).map_err(native)?;
        }
        x::sync(&state.conn).map_err(native)?;
        Ok(InteractiveInputReceipt {
            through_sequence: through,
            event_count: batch.events.len(),
            dispatch_micros: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        })
    }

    /// Release every held key and button (best effort) and close the session.
    pub fn release_all(&self) {
        let Some(mut state) = self.state.lock().unwrap().take() else {
            return;
        };
        let keys: Vec<String> = state.held_keys.drain().collect();
        for key in keys {
            let _ = self.key_edge(&mut state, &key, false, &[]);
        }
        let buttons: Vec<u8> = state.held_buttons.drain().collect();
        let (px, py) = state.last_point;
        for button in buttons {
            let _ = self.button_edge(&mut state, px, py, button, false);
        }
        let _ = x::sync(&state.conn);
    }

    fn root_point(
        &self,
        state: &State,
        x_norm: f64,
        y_norm: f64,
    ) -> Result<(i32, i32), TargetedInputError> {
        let (ox, oy, width, height) = match (self.config.window, self.config.region) {
            (Some(window), _) => {
                let geometry = state
                    .conn
                    .get_geometry(window)
                    .map_err(x::failed)?
                    .reply()
                    .map_err(|_| x::gone())?;
                let origin = state
                    .conn
                    .translate_coordinates(window, state.root, 0, 0)
                    .map_err(x::failed)?
                    .reply()
                    .map_err(x::failed)?;
                (
                    i32::from(origin.dst_x),
                    i32::from(origin.dst_y),
                    u32::from(geometry.width),
                    u32::from(geometry.height),
                )
            }
            (None, Some(region)) => region,
            (None, None) => unreachable!("validated at open"),
        };
        Ok((
            ox + denormalize(x_norm, width),
            oy + denormalize(y_norm, height),
        ))
    }

    fn dispatch_event(
        &self,
        state: &mut State,
        event: &InteractiveInputEvent,
    ) -> Result<(), TargetedInputError> {
        match event {
            InteractiveInputEvent::TextCommit { text } => {
                if self.is_foreground() {
                    x::type_xtest(&state.conn, state.root, &state.keymap, text)
                } else {
                    super::send_type_text(u64::from(self.window()?), text).map_err(x::failed)
                }
            }
            InteractiveInputEvent::Key {
                key,
                state: key_state,
                modifiers,
                ..
            } => {
                let key = x::x_key_name(key);
                let modifiers = modifier_names(modifiers);
                match key_state {
                    KeyState::Down => {
                        state.held_keys.insert(key.clone());
                        self.key_edge(state, &key, true, &modifiers)
                    }
                    KeyState::Up => {
                        // A chord was delivered whole on its down edge.
                        if !state.held_keys.remove(&key) || !modifiers.is_empty() {
                            return Ok(());
                        }
                        self.key_edge(state, &key, false, &[])
                    }
                }
            }
            InteractiveInputEvent::Pointer {
                phase,
                button,
                x_normalized,
                y_normalized,
                ..
            } => {
                let (px, py) = self.root_point(state, *x_normalized, *y_normalized)?;
                state.last_point = (px, py);
                let button = match button {
                    Some(PointerButton::Right) => x::XBUTTON_RIGHT,
                    Some(PointerButton::Middle) => x::XBUTTON_MIDDLE,
                    _ => x::XBUTTON_LEFT,
                };
                match phase {
                    PointerPhase::Move => self.motion(state, px, py),
                    PointerPhase::Down => {
                        state.held_buttons.insert(button);
                        self.button_edge(state, px, py, button, true)
                    }
                    PointerPhase::Up | PointerPhase::Cancel => {
                        state.held_buttons.remove(&button);
                        self.button_edge(state, px, py, button, false)
                    }
                }
            }
            InteractiveInputEvent::Scroll {
                x_normalized,
                y_normalized,
                delta_x,
                delta_y,
                precise,
                ..
            } => {
                let (px, py) = self.root_point(state, *x_normalized, *y_normalized)?;
                let scale = if *precise {
                    1.0 / PRECISE_PIXELS_PER_CLICK
                } else {
                    1.0
                };
                state.scroll_residual_x += delta_x * scale;
                state.scroll_residual_y += delta_y * scale;
                let dx = extract_integral(&mut state.scroll_residual_x);
                let dy = extract_integral(&mut state.scroll_residual_y);
                for (amount, negative, positive) in [
                    (dy, x::XBUTTON_SCROLL_UP, x::XBUTTON_SCROLL_DOWN),
                    (dx, x::XBUTTON_SCROLL_LEFT, x::XBUTTON_SCROLL_RIGHT),
                ] {
                    let button = if amount < 0 { negative } else { positive };
                    for _ in 0..amount.unsigned_abs() {
                        self.button_edge(state, px, py, button, true)?;
                        self.button_edge(state, px, py, button, false)?;
                    }
                }
                Ok(())
            }
        }
    }

    fn window(&self) -> Result<u32, TargetedInputError> {
        self.config.window.ok_or_else(|| {
            TargetedInputError::WouldRequireActivation("no window for background input".into())
        })
    }

    fn local(
        &self,
        state: &State,
        px: i32,
        py: i32,
    ) -> Result<(u32, i32, i32), TargetedInputError> {
        let window = self.window()?;
        let reply = state
            .conn
            .translate_coordinates(state.root, window, px as i16, py as i16)
            .map_err(x::failed)?
            .reply()
            .map_err(|_| x::gone())?;
        Ok((window, i32::from(reply.dst_x), i32::from(reply.dst_y)))
    }

    fn motion(&self, state: &State, px: i32, py: i32) -> Result<(), TargetedInputError> {
        if self.is_foreground() {
            return x::fake_motion(&state.conn, state.root, px, py);
        }
        let (window, lx, ly) = self.local(state, px, py)?;
        let held = state.held_buttons.iter().next().copied();
        super::send_motion(u64::from(window), lx, ly, held).map_err(x::failed)
    }

    fn button_edge(
        &self,
        state: &mut State,
        px: i32,
        py: i32,
        button: u8,
        press: bool,
    ) -> Result<(), TargetedInputError> {
        if self.is_foreground() {
            x::fake_motion(&state.conn, state.root, px, py)?;
            return x::fake_button(&state.conn, press, button, state.root);
        }
        let (window, lx, ly) = self.local(state, px, py)?;
        let xid = u64::from(window);
        if press {
            super::send_button_down(xid, lx, ly, button)
        } else {
            super::send_button_up(xid, lx, ly, button)
        }
        .map_err(x::failed)
    }

    fn key_edge(
        &self,
        state: &mut State,
        key: &str,
        press: bool,
        modifiers: &[String],
    ) -> Result<(), TargetedInputError> {
        if !modifiers.is_empty() {
            // A chord: deliver as one press with modifiers on the down edge.
            if !press {
                return Ok(());
            }
            if self.is_foreground() {
                let held = x::press_modifiers(&state.conn, state.root, &state.keymap, modifiers)?;
                let (code, shift) = x::key_code(&state.keymap, key)?;
                x::tap(&state.conn, state.root, &state.keymap, code, shift)?;
                return x::release_keys(&state.conn, state.root, &held);
            }
            let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            return super::send_key(u64::from(self.window()?), key, &modifiers).map_err(x::failed);
        }
        let (code, _) = x::key_code(&state.keymap, key)?;
        if self.is_foreground() {
            x::fake_key(&state.conn, press, code, state.root)
        } else {
            x::background_key_event(&state.conn, state.root, self.window()?, code, press)
        }
    }
}

impl Drop for InteractiveInputSession {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn modifier_names(modifiers: &[Modifier]) -> Vec<String> {
    modifiers
        .iter()
        .filter_map(|modifier| match modifier {
            Modifier::Shift => Some("shift"),
            Modifier::Control => Some("ctrl"),
            Modifier::Option => Some("alt"),
            Modifier::Command => Some("super"),
            Modifier::Function => None,
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_map_to_x_names() {
        assert_eq!(
            modifier_names(&[
                Modifier::Command,
                Modifier::Shift,
                Modifier::Option,
                Modifier::Control,
                Modifier::Function
            ]),
            vec!["super", "shift", "alt", "ctrl"]
        );
    }

    #[test]
    fn a_session_needs_a_target() {
        let error = InteractiveInputSession::open(InteractiveInputConfig {
            window: None,
            region: None,
            delivery_mode: InteractiveDeliveryMode::PersistentForeground,
        })
        .err()
        .expect("no target");
        assert!(matches!(error, InteractiveInputError::InvalidTarget(_)));
    }

    #[test]
    fn background_display_regions_are_refused() {
        // Refused before any X connection is needed when there is no display;
        // with a display it is refused for lack of a window.
        match InteractiveInputSession::open(InteractiveInputConfig {
            window: None,
            region: Some((0, 0, 10, 10)),
            delivery_mode: InteractiveDeliveryMode::Background,
        }) {
            Err(InteractiveInputError::WouldRequireActivation(_))
            | Err(InteractiveInputError::InvalidTarget(_)) => {}
            Err(other) => panic!("unexpected error {other}"),
            Ok(_) => panic!("background display input must be refused"),
        }
    }
}
