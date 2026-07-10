//! Driver-independent desktop side-effect observations for E2E tests.

use std::fmt;
use std::time::Duration;

use crate::e2e::OracleKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetWindow {
    pub pid: u32,
    pub native_id: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObserverCapabilities {
    pub focus: bool,
    pub z_order: bool,
    pub cursor: bool,
    pub leaked_input: bool,
}

impl ObserverCapabilities {
    fn supports(self, oracle: OracleKind) -> bool {
        match oracle {
            OracleKind::Focus => self.focus,
            OracleKind::ZOrder => self.z_order,
            OracleKind::Cursor => self.cursor,
            OracleKind::NoLeakedInput => self.leaked_input,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetZ {
    BackgroundOccluded,
    BackgroundVisible,
    Foreground,
    Minimized,
    NotFound,
}

impl TargetZ {
    fn rank(self) -> Option<u8> {
        match self {
            Self::BackgroundOccluded => Some(0),
            Self::BackgroundVisible => Some(1),
            Self::Foreground => Some(2),
            Self::Minimized | Self::NotFound => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopSnapshot {
    pub foreground: Option<u64>,
    pub input_focus: Option<u64>,
    pub target_z: TargetZ,
    pub cursor_pos: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusEvent {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesktopJournal {
    pub focus_events: Vec<FocusEvent>,
    pub leaked_input_events: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObserverError {
    message: String,
}

impl ObserverError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ObserverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ObserverError {}

pub trait ObserverBackend {
    fn capabilities(&self) -> ObserverCapabilities;
    fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError>;
    fn start_journal(&mut self) -> Result<(), ObserverError>;
    fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObservationDelta {
    pub before: DesktopSnapshot,
    pub after: DesktopSnapshot,
    pub journal: DesktopJournal,
    passed: Vec<OracleKind>,
    unsupported: Vec<OracleKind>,
    violations: Vec<String>,
}

impl ObservationDelta {
    pub fn passed(&self) -> &[OracleKind] {
        &self.passed
    }

    pub fn unsupported(&self) -> &[OracleKind] {
        &self.unsupported
    }

    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    pub fn ensure_supported(&self) -> Result<(), ObserverError> {
        if self.unsupported.is_empty() {
            Ok(())
        } else {
            Err(ObserverError::new(format!(
                "desktop observer does not support required oracles: {:?}",
                self.unsupported
            )))
        }
    }
}

pub struct DesktopObserver<B> {
    backend: B,
    target: TargetWindow,
    settle: Duration,
}

impl<B: ObserverBackend> DesktopObserver<B> {
    pub fn new(backend: B, target: TargetWindow) -> Self {
        Self {
            backend,
            target,
            settle: Duration::from_millis(150),
        }
    }

    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    pub fn observe<R>(
        &mut self,
        requested: &[OracleKind],
        action: impl FnOnce() -> R,
    ) -> Result<(R, ObservationDelta), ObserverError> {
        let before = self.backend.snapshot(self.target)?;
        self.backend.start_journal()?;
        let result = action();
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle);
        }
        let journal = self.backend.drain_journal()?;
        let after = self.backend.snapshot(self.target)?;
        let delta = evaluate(
            self.backend.capabilities(),
            requested,
            before,
            after,
            journal,
        );
        Ok((result, delta))
    }
}

fn evaluate(
    capabilities: ObserverCapabilities,
    requested: &[OracleKind],
    before: DesktopSnapshot,
    after: DesktopSnapshot,
    journal: DesktopJournal,
) -> ObservationDelta {
    let mut passed = Vec::new();
    let mut unsupported = Vec::new();
    let mut violations = Vec::new();

    for oracle in requested.iter().copied() {
        if !capabilities.supports(oracle) {
            unsupported.push(oracle);
            continue;
        }
        let violation = match oracle {
            OracleKind::Focus => {
                if before.foreground != after.foreground {
                    Some(format!(
                        "foreground changed from {:?} to {:?}",
                        before.foreground, after.foreground
                    ))
                } else if !journal.focus_events.is_empty() {
                    Some(format!(
                        "foreground changed transiently: {:?}",
                        journal.focus_events
                    ))
                } else {
                    None
                }
            }
            OracleKind::ZOrder => match (before.target_z.rank(), after.target_z.rank()) {
                (Some(before_rank), Some(after_rank)) if after_rank <= before_rank => None,
                (Some(_), Some(_)) => Some(format!(
                    "target rose from {:?} to {:?}",
                    before.target_z, after.target_z
                )),
                _ => Some(format!(
                    "target z-order could not be compared: {:?} -> {:?}",
                    before.target_z, after.target_z
                )),
            },
            OracleKind::Cursor => match (before.cursor_pos, after.cursor_pos) {
                (Some((before_x, before_y)), Some((after_x, after_y)))
                    if (before_x - after_x).abs() <= 1.0 && (before_y - after_y).abs() <= 1.0 =>
                {
                    None
                }
                (Some(before_pos), Some(after_pos)) => Some(format!(
                    "real cursor moved from {before_pos:?} to {after_pos:?}"
                )),
                _ => Some("real cursor position was unavailable".to_owned()),
            },
            OracleKind::NoLeakedInput => {
                if journal.leaked_input_events.is_empty() {
                    None
                } else {
                    Some(format!(
                        "foreground sentinel received input: {:?}",
                        journal.leaked_input_events
                    ))
                }
            }
            _ => continue,
        };
        if let Some(violation) = violation {
            violations.push(violation);
        } else {
            passed.push(oracle);
        }
    }

    ObservationDelta {
        before,
        after,
        journal,
        passed,
        unsupported,
        violations,
    }
}

#[cfg(target_os = "windows")]
pub mod windows {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowRect, IsIconic, IsWindow,
        WindowFromPoint, GA_ROOT,
    };

    use super::{
        DesktopJournal, DesktopSnapshot, FocusEvent, ObserverBackend, ObserverCapabilities,
        ObserverError, TargetWindow, TargetZ,
    };

    pub struct WindowsObserver {
        stop: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<FocusEvent>>>,
        sampler: Option<JoinHandle<()>>,
    }

    impl WindowsObserver {
        pub fn new() -> Self {
            Self {
                stop: Arc::new(AtomicBool::new(false)),
                events: Arc::new(Mutex::new(Vec::new())),
                sampler: None,
            }
        }
    }

    impl Default for WindowsObserver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ObserverBackend for WindowsObserver {
        fn capabilities(&self) -> ObserverCapabilities {
            ObserverCapabilities {
                focus: true,
                z_order: true,
                cursor: true,
                leaked_input: false,
            }
        }

        fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
            let hwnd = HWND(target.native_id as *mut _);
            let target_z = unsafe {
                if !IsWindow(hwnd).as_bool() {
                    TargetZ::NotFound
                } else if IsIconic(hwnd).as_bool() {
                    TargetZ::Minimized
                } else if root(GetForegroundWindow()) == root(hwnd) {
                    TargetZ::Foreground
                } else if is_occluded(hwnd)? {
                    TargetZ::BackgroundOccluded
                } else {
                    TargetZ::BackgroundVisible
                }
            };
            let foreground = unsafe { raw(root(GetForegroundWindow())) };
            let mut cursor = POINT::default();
            let cursor_pos = unsafe {
                GetCursorPos(&mut cursor)
                    .ok()
                    .map(|_| (f64::from(cursor.x), f64::from(cursor.y)))
            };
            Ok(DesktopSnapshot {
                foreground,
                input_focus: foreground,
                target_z,
                cursor_pos,
            })
        }

        fn start_journal(&mut self) -> Result<(), ObserverError> {
            if self.sampler.is_some() {
                return Err(ObserverError::new("Windows focus journal already active"));
            }
            self.stop.store(false, Ordering::Release);
            self.events.lock().expect("focus journal lock").clear();
            let stop = Arc::clone(&self.stop);
            let events = Arc::clone(&self.events);
            self.sampler = Some(std::thread::spawn(move || {
                let mut previous = unsafe { raw(root(GetForegroundWindow())) };
                while !stop.load(Ordering::Acquire) {
                    let current = unsafe { raw(root(GetForegroundWindow())) };
                    if current != previous {
                        events.lock().expect("focus journal lock").push(FocusEvent {
                            from: previous,
                            to: current,
                        });
                        previous = current;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }));
            Ok(())
        }

        fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError> {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                sampler
                    .join()
                    .map_err(|_| ObserverError::new("Windows focus journal panicked"))?;
            }
            Ok(DesktopJournal {
                focus_events: self.events.lock().expect("focus journal lock").clone(),
                leaked_input_events: Vec::new(),
            })
        }
    }

    impl Drop for WindowsObserver {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                let _ = sampler.join();
            }
        }
    }

    unsafe fn root(hwnd: HWND) -> HWND {
        if hwnd.0.is_null() {
            hwnd
        } else {
            unsafe { GetAncestor(hwnd, GA_ROOT) }
        }
    }

    fn raw(hwnd: HWND) -> Option<u64> {
        (!hwnd.0.is_null()).then_some(hwnd.0 as usize as u64)
    }

    unsafe fn is_occluded(hwnd: HWND) -> Result<bool, ObserverError> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }
            .map_err(|error| ObserverError::new(format!("GetWindowRect failed: {error}")))?;
        if rect.right - rect.left <= 4 || rect.bottom - rect.top <= 4 {
            return Ok(false);
        }
        let points = [
            POINT {
                x: rect.left + 2,
                y: rect.top + 2,
            },
            POINT {
                x: rect.right - 3,
                y: rect.top + 2,
            },
            POINT {
                x: rect.left + 2,
                y: rect.bottom - 3,
            },
            POINT {
                x: rect.right - 3,
                y: rect.bottom - 3,
            },
            POINT {
                x: (rect.left + rect.right) / 2,
                y: (rect.top + rect.bottom) / 2,
            },
        ];
        let target_root = unsafe { root(hwnd) };
        let covered = points
            .into_iter()
            .filter(|point| {
                let owner = unsafe { WindowFromPoint(*point) };
                !owner.0.is_null() && unsafe { root(owner) } != target_root
            })
            .count();
        Ok(covered >= 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(foreground: u64, target_z: TargetZ, cursor: (f64, f64)) -> DesktopSnapshot {
        DesktopSnapshot {
            foreground: Some(foreground),
            input_focus: Some(foreground),
            target_z,
            cursor_pos: Some(cursor),
        }
    }

    #[test]
    fn transient_focus_change_fails_even_when_pre_and_post_match() {
        let before = snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0));
        let after = before.clone();
        let delta = evaluate(
            ObserverCapabilities {
                focus: true,
                ..ObserverCapabilities::default()
            },
            &[OracleKind::Focus],
            before,
            after,
            DesktopJournal {
                focus_events: vec![
                    FocusEvent {
                        from: Some(10),
                        to: Some(20),
                    },
                    FocusEvent {
                        from: Some(20),
                        to: Some(10),
                    },
                ],
                leaked_input_events: Vec::new(),
            },
        );
        assert!(delta.passed().is_empty());
        assert!(delta.violations()[0].contains("transiently"));
    }

    #[test]
    fn target_raise_cursor_move_and_input_leak_are_independent_failures() {
        let delta = evaluate(
            ObserverCapabilities {
                focus: true,
                z_order: true,
                cursor: true,
                leaked_input: true,
            },
            &[
                OracleKind::Focus,
                OracleKind::ZOrder,
                OracleKind::Cursor,
                OracleKind::NoLeakedInput,
            ],
            snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0)),
            snapshot(10, TargetZ::BackgroundVisible, (102.0, 100.0)),
            DesktopJournal {
                focus_events: Vec::new(),
                leaked_input_events: vec!["keydown:A".to_owned()],
            },
        );
        assert_eq!(delta.passed(), &[OracleKind::Focus]);
        assert_eq!(delta.violations().len(), 3);
    }

    #[test]
    fn unsupported_oracle_is_never_counted_as_passed() {
        let state = snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0));
        let delta = evaluate(
            ObserverCapabilities::default(),
            &[OracleKind::NoLeakedInput],
            state.clone(),
            state,
            DesktopJournal::default(),
        );
        assert_eq!(delta.unsupported(), &[OracleKind::NoLeakedInput]);
        assert!(delta.ensure_supported().is_err());
        assert!(delta.passed().is_empty());
    }
}
