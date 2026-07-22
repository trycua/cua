//! Low-latency, stateful input delivery to one macOS window.
//!
//! Automation tools intentionally trade latency for verification, focus
//! containment, and human-readable results. Interactive remoting has a
//! different contract: preserve the source device's event ordering and
//! semantics while doing as little work as possible between receipt and
//! native dispatch. This module provides that lower-level contract without
//! changing the existing one-shot tools.

use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use core_graphics::{
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use foreign_types::ForeignType;

const MAX_BATCH_EVENTS: usize = 256;
const TEXT_EVENT_UTF16_UNITS: usize = 20;
const SCROLL_PHASE_FIELD: u32 = 99;
const SCROLL_MOMENTUM_PHASE_FIELD: u32 = 123;

/// Controls whether events remain PID-routed or use the foreground HID queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveDeliveryMode {
    Background,
    /// Keep the target frontmost for the lifetime of the input session and
    /// deliver through the same global HID queue as physical input.
    PersistentForeground,
}

#[derive(Debug, Clone)]
pub struct InteractiveInputConfig {
    pub pid: i32,
    pub window_id: u32,
    pub delivery_mode: InteractiveDeliveryMode,
    /// Number of batches waiting for the native worker. A full queue returns
    /// backpressure to the caller instead of silently dropping input.
    pub queue_capacity: usize,
}

impl InteractiveInputConfig {
    pub fn persistent_foreground(pid: i32, window_id: u32) -> Self {
        Self {
            pid,
            window_id,
            delivery_mode: InteractiveDeliveryMode::PersistentForeground,
            queue_capacity: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Command,
    Shift,
    Option,
    Control,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerPhase {
    Move,
    Down,
    Up,
    Cancel,
}

/// Native macOS scroll gesture phase. Keeping phase and momentum distinct is
/// important for inertial scrolling and overscroll behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GesturePhase {
    None,
    MayBegin,
    Began,
    Changed,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractiveInputEvent {
    /// Commit already-composed Unicode text. This is intentionally distinct
    /// from physical key events so IME/dead-key composition is not replayed.
    TextCommit { text: String },
    Key {
        key: String,
        state: KeyState,
        modifiers: Vec<Modifier>,
        repeat: bool,
    },
    Pointer {
        phase: PointerPhase,
        button: Option<PointerButton>,
        x_normalized: f64,
        y_normalized: f64,
        modifiers: Vec<Modifier>,
    },
    Scroll {
        x_normalized: f64,
        y_normalized: f64,
        delta_x: f64,
        delta_y: f64,
        phase: GesturePhase,
        momentum_phase: GesturePhase,
        precise: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractiveInputBatch {
    pub first_sequence: u64,
    pub events: Vec<InteractiveInputEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractiveInputReceipt {
    pub through_sequence: u64,
    pub event_count: usize,
    pub dispatch_micros: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum InteractiveInputError {
    #[error("interactive input target is invalid: {0}")]
    InvalidTarget(String),
    #[error("interactive input batch is invalid: {0}")]
    InvalidBatch(String),
    #[error("interactive input queue is full")]
    Backpressure,
    #[error("interactive input session is closed")]
    Closed,
    #[error("native input delivery failed: {0}")]
    Native(String),
}

type Result<T> = std::result::Result<T, InteractiveInputError>;

struct WorkItem {
    batch: InteractiveInputBatch,
    reply: mpsc::Sender<Result<InteractiveInputReceipt>>,
}

/// A persistent, thread-safe handle to one native input worker.
///
/// The worker owns and reuses one `CGEventSource`, pointer-button state, click
/// grouping, and fractional scroll residuals. Native CoreGraphics objects
/// never cross threads.
pub struct InteractiveInputSession {
    sender: Option<SyncSender<WorkItem>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl InteractiveInputSession {
    pub fn open(config: InteractiveInputConfig) -> Result<Self> {
        validate_config(&config)?;
        let capacity = config.queue_capacity.max(1);
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(format!("cua-input-{}-{}", config.pid, config.window_id))
            .spawn(move || run_worker(config, receiver, ready_tx))
            .map_err(|e| InteractiveInputError::Native(e.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                sender: Some(sender),
                worker: Mutex::new(Some(worker)),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(InteractiveInputError::Closed)
            }
        }
    }

    /// Deliver one ordered batch and wait only until CoreGraphics has accepted
    /// every event. This is a native-dispatch acknowledgement, not application
    /// state verification.
    pub fn dispatch(&self, batch: InteractiveInputBatch) -> Result<InteractiveInputReceipt> {
        validate_batch(&batch)?;
        let (reply, receipt) = mpsc::channel();
        let item = WorkItem { batch, reply };
        let sender = self.sender.as_ref().ok_or(InteractiveInputError::Closed)?;
        match sender.try_send(item) {
            Ok(()) => receipt.recv().map_err(|_| InteractiveInputError::Closed)?,
            Err(TrySendError::Full(_)) => Err(InteractiveInputError::Backpressure),
            Err(TrySendError::Disconnected(_)) => Err(InteractiveInputError::Closed),
        }
    }
}

impl Drop for InteractiveInputSession {
    fn drop(&mut self) {
        // Disconnecting the final sender lets the worker drain accepted input
        // before exiting. No accepted key-up or pointer-up is discarded.
        self.sender.take();
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

fn validate_config(config: &InteractiveInputConfig) -> Result<()> {
    if config.pid <= 0 {
        return Err(InteractiveInputError::InvalidTarget(
            "pid must be positive".to_owned(),
        ));
    }
    if config.window_id == 0 {
        return Err(InteractiveInputError::InvalidTarget(
            "window_id must be non-zero".to_owned(),
        ));
    }
    target_window_bounds(config)?;
    Ok(())
}

fn target_window_bounds(config: &InteractiveInputConfig) -> Result<crate::windows::WindowBounds> {
    crate::windows::all_windows()
        .into_iter()
        .find(|window| window.window_id == config.window_id && window.pid == config.pid)
        .map(|window| window.bounds)
        .ok_or_else(|| {
            InteractiveInputError::InvalidTarget(format!(
                "window {} is not owned by pid {}",
                config.window_id, config.pid
            ))
        })
}

fn validate_batch(batch: &InteractiveInputBatch) -> Result<()> {
    if batch.events.is_empty() {
        return Err(InteractiveInputError::InvalidBatch(
            "events must not be empty".to_owned(),
        ));
    }
    if batch.events.len() > MAX_BATCH_EVENTS {
        return Err(InteractiveInputError::InvalidBatch(format!(
            "at most {MAX_BATCH_EVENTS} events are allowed"
        )));
    }
    batch
        .first_sequence
        .checked_add(batch.events.len() as u64 - 1)
        .ok_or_else(|| InteractiveInputError::InvalidBatch("sequence overflow".to_owned()))?;

    for event in &batch.events {
        match event {
            InteractiveInputEvent::TextCommit { text } if text.is_empty() => {
                return Err(InteractiveInputError::InvalidBatch(
                    "text commits must not be empty".to_owned(),
                ));
            }
            InteractiveInputEvent::Pointer {
                x_normalized,
                y_normalized,
                ..
            }
            | InteractiveInputEvent::Scroll {
                x_normalized,
                y_normalized,
                ..
            } if !normalized(*x_normalized) || !normalized(*y_normalized) => {
                return Err(InteractiveInputError::InvalidBatch(
                    "pointer coordinates must be finite and within [0, 1]".to_owned(),
                ));
            }
            InteractiveInputEvent::Scroll {
                delta_x, delta_y, ..
            } if !delta_x.is_finite() || !delta_y.is_finite() => {
                return Err(InteractiveInputError::InvalidBatch(
                    "scroll deltas must be finite".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn normalized(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn run_worker(
    config: InteractiveInputConfig,
    receiver: Receiver<WorkItem>,
    ready: mpsc::Sender<Result<()>>,
) {
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(source) => source,
        Err(_) => {
            let _ = ready.send(Err(InteractiveInputError::Native(
                "CGEventSource::new failed".to_owned(),
            )));
            return;
        }
    };
    let bounds = match target_window_bounds(&config) {
        Ok(bounds) => bounds,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut state = NativeInputState::new(config, source, bounds);
    if let Err(error) = state.prepare_target() {
        let _ = ready.send(Err(error));
        return;
    }
    if ready.send(Ok(())).is_err() {
        return;
    }

    for item in receiver {
        let started = Instant::now();
        let result = state
            .dispatch_batch(&item.batch)
            .map(|()| InteractiveInputReceipt {
                through_sequence: item.batch.first_sequence + item.batch.events.len() as u64 - 1,
                event_count: item.batch.events.len(),
                dispatch_micros: started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            });
        let _ = item.reply.send(result);
    }
}

struct NativeInputState {
    config: InteractiveInputConfig,
    source: CGEventSource,
    pressed_button: Option<PointerButton>,
    click_group_id: i64,
    scroll_residual_x: f64,
    scroll_residual_y: f64,
    bounds: crate::windows::WindowBounds,
    last_bounds_refresh: Instant,
}

impl NativeInputState {
    fn new(
        config: InteractiveInputConfig,
        source: CGEventSource,
        bounds: crate::windows::WindowBounds,
    ) -> Self {
        Self {
            config,
            source,
            pressed_button: None,
            click_group_id: 1,
            scroll_residual_x: 0.0,
            scroll_residual_y: 0.0,
            bounds,
            last_bounds_refresh: Instant::now(),
        }
    }

    fn prepare_target(&self) -> Result<()> {
        if self.config.delivery_mode == InteractiveDeliveryMode::PersistentForeground
            && crate::apps::frontmost_pid() != Some(self.config.pid)
        {
            if !crate::input::skylight::set_front_process_persistently(
                self.config.pid as libc::pid_t,
                self.config.window_id,
            ) {
                crate::apps::activate_pid(self.config.pid);
            }
            // Paid once when opening the session, never on the event hot path.
            thread::sleep(std::time::Duration::from_millis(40));
        }
        Ok(())
    }

    fn dispatch_batch(&mut self, batch: &InteractiveInputBatch) -> Result<()> {
        if self.config.delivery_mode == InteractiveDeliveryMode::PersistentForeground
            && crate::apps::frontmost_pid() != Some(self.config.pid)
        {
            self.prepare_target()?;
        }
        self.refresh_bounds_if_due()?;
        for event in &batch.events {
            self.dispatch_event(event)?;
        }
        Ok(())
    }

    fn dispatch_event(&mut self, event: &InteractiveInputEvent) -> Result<()> {
        match event {
            InteractiveInputEvent::TextCommit { text } => self.commit_text(text),
            InteractiveInputEvent::Key {
                key,
                state,
                modifiers,
                repeat,
            } => self.key(key, *state, modifiers, *repeat),
            InteractiveInputEvent::Pointer {
                phase,
                button,
                x_normalized,
                y_normalized,
                modifiers,
            } => self.pointer(*phase, *button, *x_normalized, *y_normalized, modifiers),
            InteractiveInputEvent::Scroll {
                x_normalized,
                y_normalized,
                delta_x,
                delta_y,
                phase,
                momentum_phase,
                precise,
            } => self.scroll(
                *x_normalized,
                *y_normalized,
                *delta_x,
                *delta_y,
                *phase,
                *momentum_phase,
                *precise,
            ),
        }
    }

    fn commit_text(&self, text: &str) -> Result<()> {
        for chunk in text_event_chunks(text) {
            let down = CGEvent::new_keyboard_event(self.source.clone(), 0, true)
                .map_err(|_| native("keyboard down creation failed"))?;
            down.set_string_from_utf16_unchecked(&chunk);
            down.set_flags(CGEventFlags::CGEventFlagNull);
            self.post_keyboard(&down);

            let up = CGEvent::new_keyboard_event(self.source.clone(), 0, false)
                .map_err(|_| native("keyboard up creation failed"))?;
            up.set_string_from_utf16_unchecked(&chunk);
            up.set_flags(CGEventFlags::CGEventFlagNull);
            self.post_keyboard(&up);
        }
        Ok(())
    }

    fn key(&self, key: &str, state: KeyState, modifiers: &[Modifier], repeat: bool) -> Result<()> {
        let key_code = super::keyboard::key_name_to_code(key)
            .map_err(|error| InteractiveInputError::InvalidBatch(error.to_string()))?;
        let event =
            CGEvent::new_keyboard_event(self.source.clone(), key_code, state == KeyState::Down)
                .map_err(|_| native("keyboard event creation failed"))?;
        event.set_flags(modifier_flags(modifiers));
        event.set_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT, i64::from(repeat));
        self.post_keyboard(&event);
        Ok(())
    }

    fn pointer(
        &mut self,
        phase: PointerPhase,
        button: Option<PointerButton>,
        x_normalized: f64,
        y_normalized: f64,
        modifiers: &[Modifier],
    ) -> Result<()> {
        let (point, window_local) = self.resolve_point(x_normalized, y_normalized)?;
        let effective_button = button
            .or(self.pressed_button)
            .unwrap_or(PointerButton::Left);
        let (event_type, cg_button) =
            if phase == PointerPhase::Move && button.is_none() && self.pressed_button.is_none() {
                (CGEventType::MouseMoved, CGMouseButton::Left)
            } else {
                pointer_event_type(phase, effective_button)
            };
        let event = CGEvent::new_mouse_event(self.source.clone(), event_type, point, cg_button)
            .map_err(|_| native("pointer event creation failed"))?;
        event.set_flags(modifier_flags(modifiers));

        if phase == PointerPhase::Down {
            self.pressed_button = Some(effective_button);
            self.click_group_id = self.click_group_id.wrapping_add(1).max(1);
        }
        self.post_pointer(
            &event,
            window_local,
            effective_button,
            matches!(phase, PointerPhase::Down | PointerPhase::Up),
        );
        if matches!(phase, PointerPhase::Up | PointerPhase::Cancel) {
            self.pressed_button = None;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn scroll(
        &mut self,
        x_normalized: f64,
        y_normalized: f64,
        delta_x: f64,
        delta_y: f64,
        phase: GesturePhase,
        momentum_phase: GesturePhase,
        precise: bool,
    ) -> Result<()> {
        let (point, window_local) = self.resolve_point(x_normalized, y_normalized)?;
        self.scroll_residual_x += delta_x;
        self.scroll_residual_y += delta_y;
        let integral_x = extract_integral(&mut self.scroll_residual_x);
        let integral_y = extract_integral(&mut self.scroll_residual_y);

        // Preserve zero-delta phase transitions. AppKit uses the ended/cancelled
        // event to stop momentum and rubber-banding even when its delta is zero.
        if integral_x == 0
            && integral_y == 0
            && phase == GesturePhase::None
            && momentum_phase == GesturePhase::None
        {
            return Ok(());
        }

        let units = if precise {
            ScrollEventUnit::PIXEL
        } else {
            ScrollEventUnit::LINE
        };
        let event =
            CGEvent::new_scroll_event(self.source.clone(), units, 2, integral_y, integral_x, 0)
                .map_err(|_| native("scroll event creation failed"))?;
        event.set_integer_value_field(
            EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS,
            i64::from(precise),
        );
        event.set_integer_value_field(SCROLL_PHASE_FIELD, scroll_phase_value(phase));
        event.set_integer_value_field(
            SCROLL_MOMENTUM_PHASE_FIELD,
            momentum_phase_value(momentum_phase),
        );
        unsafe { CGEventSetLocation(event.as_ptr().cast(), point.x, point.y) };

        if self.is_foreground() {
            event.post(CGEventTapLocation::HID);
        } else {
            super::mouse::post_mouse_event(
                self.config.pid,
                &event,
                Some(window_local),
                Some(self.config.window_id),
                Some(self.click_group_id),
                0,
                0,
                0,
            );
        }
        Ok(())
    }

    fn resolve_point(&self, x: f64, y: f64) -> Result<(CGPoint, (f64, f64))> {
        let local = (x * self.bounds.width, y * self.bounds.height);
        Ok((
            CGPoint::new(self.bounds.x + local.0, self.bounds.y + local.1),
            local,
        ))
    }

    fn refresh_bounds_if_due(&mut self) -> Result<()> {
        // Window enumeration is substantially more expensive than event
        // creation. A short cache preserves resize responsiveness without
        // putting a WindowServer round trip on every pointer sample.
        if self.last_bounds_refresh.elapsed() >= Duration::from_millis(100) {
            self.bounds = target_window_bounds(&self.config)?;
            self.last_bounds_refresh = Instant::now();
        }
        Ok(())
    }

    fn post_keyboard(&self, event: &CGEvent) {
        if self.is_foreground() {
            event.post(CGEventTapLocation::HID);
        } else {
            super::keyboard::post_keyboard_event(self.config.pid, event);
        }
    }

    fn post_pointer(
        &self,
        event: &CGEvent,
        window_local: (f64, f64),
        button: PointerButton,
        click: bool,
    ) {
        if self.is_foreground() {
            event.post(CGEventTapLocation::HID);
        } else {
            super::mouse::post_mouse_event(
                self.config.pid,
                event,
                Some(window_local),
                Some(self.config.window_id),
                Some(self.click_group_id),
                i64::from(click),
                pointer_button_number(button),
                if click { 3 } else { 0 },
            );
        }
    }

    fn is_foreground(&self) -> bool {
        self.config.delivery_mode == InteractiveDeliveryMode::PersistentForeground
    }
}

fn modifier_flags(modifiers: &[Modifier]) -> CGEventFlags {
    modifiers
        .iter()
        .fold(CGEventFlags::CGEventFlagNull, |flags, modifier| {
            flags
                | match modifier {
                    Modifier::Command => CGEventFlags::CGEventFlagCommand,
                    Modifier::Shift => CGEventFlags::CGEventFlagShift,
                    Modifier::Option => CGEventFlags::CGEventFlagAlternate,
                    Modifier::Control => CGEventFlags::CGEventFlagControl,
                    Modifier::Function => CGEventFlags::CGEventFlagSecondaryFn,
                }
        })
}

fn pointer_event_type(phase: PointerPhase, button: PointerButton) -> (CGEventType, CGMouseButton) {
    match (phase, button) {
        (PointerPhase::Down, PointerButton::Left) => {
            (CGEventType::LeftMouseDown, CGMouseButton::Left)
        }
        (PointerPhase::Up | PointerPhase::Cancel, PointerButton::Left) => {
            (CGEventType::LeftMouseUp, CGMouseButton::Left)
        }
        (PointerPhase::Move, PointerButton::Left) => {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        }
        (PointerPhase::Down, PointerButton::Right) => {
            (CGEventType::RightMouseDown, CGMouseButton::Right)
        }
        (PointerPhase::Up | PointerPhase::Cancel, PointerButton::Right) => {
            (CGEventType::RightMouseUp, CGMouseButton::Right)
        }
        (PointerPhase::Move, PointerButton::Right) => {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        }
        (PointerPhase::Down, PointerButton::Middle) => {
            (CGEventType::OtherMouseDown, CGMouseButton::Center)
        }
        (PointerPhase::Up | PointerPhase::Cancel, PointerButton::Middle) => {
            (CGEventType::OtherMouseUp, CGMouseButton::Center)
        }
        (PointerPhase::Move, PointerButton::Middle) => {
            (CGEventType::OtherMouseDragged, CGMouseButton::Center)
        }
    }
}

fn pointer_button_number(button: PointerButton) -> i64 {
    match button {
        PointerButton::Left => 0,
        PointerButton::Right => 1,
        PointerButton::Middle => 2,
    }
}

fn extract_integral(residual: &mut f64) -> i32 {
    let value = residual.trunc().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    *residual -= f64::from(value);
    value
}

fn text_event_chunks(text: &str) -> Vec<Vec<u16>> {
    let mut chunks = Vec::new();
    let mut current = Vec::with_capacity(TEXT_EVENT_UTF16_UNITS);
    for character in text.chars() {
        let mut encoded = [0_u16; 2];
        let units = character.encode_utf16(&mut encoded);
        if !current.is_empty() && current.len() + units.len() > TEXT_EVENT_UTF16_UNITS {
            chunks.push(std::mem::take(&mut current));
            current = Vec::with_capacity(TEXT_EVENT_UTF16_UNITS);
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn scroll_phase_value(phase: GesturePhase) -> i64 {
    match phase {
        GesturePhase::None => 0,
        GesturePhase::MayBegin => 128,
        GesturePhase::Began => 1,
        GesturePhase::Changed => 2,
        GesturePhase::Ended => 4,
        GesturePhase::Cancelled => 8,
    }
}

fn momentum_phase_value(phase: GesturePhase) -> i64 {
    match phase {
        GesturePhase::None => 0,
        GesturePhase::Began | GesturePhase::MayBegin => 1,
        GesturePhase::Changed => 2,
        GesturePhase::Ended | GesturePhase::Cancelled => 3,
    }
}

fn native(message: &str) -> InteractiveInputError {
    InteractiveInputError::Native(message.to_owned())
}

extern "C" {
    fn CGEventSetLocation(event: *mut std::ffi::c_void, x: f64, y: f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_fractional_scroll_until_it_forms_pixels() {
        let mut residual = 0.0;
        residual += 0.4;
        assert_eq!(extract_integral(&mut residual), 0);
        residual += 0.8;
        assert_eq!(extract_integral(&mut residual), 1);
        assert!((residual - 0.2).abs() < f64::EPSILON * 4.0);
    }

    #[test]
    fn maps_native_scroll_phases() {
        assert_eq!(scroll_phase_value(GesturePhase::MayBegin), 128);
        assert_eq!(scroll_phase_value(GesturePhase::Began), 1);
        assert_eq!(scroll_phase_value(GesturePhase::Changed), 2);
        assert_eq!(scroll_phase_value(GesturePhase::Ended), 4);
        assert_eq!(momentum_phase_value(GesturePhase::Changed), 2);
        assert_eq!(momentum_phase_value(GesturePhase::Ended), 3);
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_coordinates() {
        for value in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
            assert!(!normalized(value));
        }
        assert!(normalized(0.0));
        assert!(normalized(1.0));
    }

    #[test]
    fn text_chunks_do_not_split_surrogate_pairs() {
        let chunks = text_event_chunks(&format!("{}😀", "a".repeat(19)));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(chunks[1], "😀".encode_utf16().collect::<Vec<_>>());
    }
}
