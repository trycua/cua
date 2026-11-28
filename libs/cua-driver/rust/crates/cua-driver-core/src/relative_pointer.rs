// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `mouse_move_relative`: genuine relative pointer motion.
//!
//! An app that locks or captures the pointer (a first-person game, a page
//! under browser Pointer Lock) reads motion deltas and ignores where the
//! cursor is, so an absolute warp to a computed point turns nothing. This
//! tool sends the native relative event each platform has: a mouse-moved
//! event carrying delta fields on macOS, `SendInput` without
//! `MOUSEEVENTF_ABSOLUTE` on Windows, XTest relative motion on X11, and
//! relative virtual-pointer or libei motion on Wayland. A backend that has no
//! relative route refuses with [`UNSUPPORTED_CODE`] and sends nothing; it
//! never substitutes an absolute move.
//!
//! The schema, argument checks, step splitting and result shapes live here
//! so every platform adapter only submits one native event per step.

use std::time::Duration;

use serde_json::{json, Value};

use crate::action_record::{ActionExecutionRecord, ActionTransport};
use crate::protocol::ToolResult;
use crate::tool::ToolDef;

pub const TOOL_NAME: &str = "mouse_move_relative";

/// Largest delta per axis. X11 carries XTest motion offsets as 16-bit
/// integers, so this is the range every backend can send in one event.
pub const MAX_DELTA: i64 = i16::MAX as i64;

/// Longest a move may be spread over, the same bound as `drag`.
pub const MAX_DURATION_MS: u64 = 10_000;

/// Gap between the steps of a spread move: about one event per 125 Hz frame.
const STEP_INTERVAL_MS: u64 = 8;

/// Refusal code for a backend with no relative pointer route.
pub const UNSUPPORTED_CODE: &str = "unsupported_operation";

/// A validated relative move.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelativeMove {
    pub dx: i32,
    pub dy: i32,
    pub duration_ms: u64,
}

/// One native event's delta and the pause before the next event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelativeStep {
    pub dx: i32,
    pub dy: i32,
    pub pause_after: Duration,
}

/// The tool definition every platform registers. `extra_properties` adds the
/// platform-owned arguments (macOS takes `delivery_mode` for its desktop
/// pointer guard).
pub fn tool_def(platform_note: &str, extra_properties: &[(&str, Value)]) -> ToolDef {
    let mut properties = json!({
        "dx": {
            "type": "integer",
            "minimum": -MAX_DELTA,
            "maximum": MAX_DELTA,
            "description": "Horizontal motion in device units (mouse counts). Positive moves right."
        },
        "dy": {
            "type": "integer",
            "minimum": -MAX_DELTA,
            "maximum": MAX_DELTA,
            "description": "Vertical motion in device units (mouse counts). Positive moves down."
        },
        "duration_ms": {
            "type": "integer",
            "minimum": 0,
            "maximum": MAX_DURATION_MS,
            "description": "Spread the motion over this many milliseconds as a stream of smaller deltas that sum exactly to dx/dy. Default 0: one event."
        },
        "session": crate::tool_schema::session_schema(),
    });
    for (name, schema) in extra_properties {
        properties[*name] = schema.clone();
    }
    ToolDef {
        name: TOOL_NAME.into(),
        description: format!(
            "Move the user's real OS pointer by a relative (dx, dy) delta, sent as native \
             relative motion events rather than an absolute warp. Use it for an app that locks \
             or captures the pointer, such as a first-person game's camera or a web page under \
             Pointer Lock, where move_cursor and click coordinates have no effect. The focused \
             app receives the motion; there is no window target. It presses and releases \
             nothing, so a button or key already held stays held across repeated calls. A backend \
             with no relative route refuses with code \"{UNSUPPORTED_CODE}\" and sends nothing.{platform_note}"
        ),
        input_schema: json!({
            "type": "object",
            "required": ["dx", "dy"],
            "properties": properties,
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    }
}

/// Validate the arguments. Errors carry `code: "invalid_arguments"`.
pub fn parse(args: &Value) -> Result<RelativeMove, ToolResult> {
    let dx = delta(args, "dx")?;
    let dy = delta(args, "dy")?;
    let duration_ms = match args.get("duration_ms") {
        None | Some(Value::Null) => 0,
        Some(value) => match value.as_u64() {
            Some(ms) if ms <= MAX_DURATION_MS => ms,
            _ => {
                return Err(invalid(format!(
                    "duration_ms must be an integer from 0 to {MAX_DURATION_MS}"
                )))
            }
        },
    };
    Ok(RelativeMove {
        dx,
        dy,
        duration_ms,
    })
}

fn delta(args: &Value, name: &str) -> Result<i32, ToolResult> {
    match args.get(name).and_then(Value::as_i64) {
        Some(value) if (-MAX_DELTA..=MAX_DELTA).contains(&value) => Ok(value as i32),
        _ => Err(invalid(format!(
            "{name} is required and must be an integer from {} to {MAX_DELTA}",
            -MAX_DELTA
        ))),
    }
}

fn invalid(detail: String) -> ToolResult {
    ToolResult::error(format!("{TOOL_NAME}: {detail}")).with_structured(json!({
        "code": "invalid_arguments",
        "tool": TOOL_NAME,
        "detail": detail,
    }))
}

impl RelativeMove {
    /// Split the move into native events. Step `i` of `n` carries
    /// `round(d*i/n) - round(d*(i-1)/n)`, so the steps sum exactly to the
    /// requested delta with no drift. There is at most one step per
    /// [`STEP_INTERVAL_MS`] and never more steps than the larger delta, so
    /// no step is empty. Each step is followed by an equal share of the
    /// duration, so the move takes `duration_ms` whatever its step count. A
    /// zero move has no steps.
    pub fn steps(&self) -> Vec<RelativeStep> {
        let span = i64::from(self.dx.unsigned_abs().max(self.dy.unsigned_abs()));
        if span == 0 {
            return Vec::new();
        }
        let count = (self.duration_ms / STEP_INTERVAL_MS).clamp(1, span as u64) as i64;
        let pause = Duration::from_millis(self.duration_ms) / count as u32;
        let at = |delta: i32, i: i64| -> i64 {
            // Round half away from zero so a move and its negation split
            // symmetrically.
            let scaled = i64::from(delta) * i;
            let half = count / 2;
            if scaled >= 0 {
                (scaled + half) / count
            } else {
                (scaled - half) / count
            }
        };
        (1..=count)
            .map(|i| RelativeStep {
                dx: (at(self.dx, i) - at(self.dx, i - 1)) as i32,
                dy: (at(self.dy, i) - at(self.dy, i - 1)) as i32,
                pause_after: pause,
            })
            .collect()
    }
}

/// How far a move got before a native send failed.
#[derive(Debug)]
pub struct Incomplete {
    pub sent_steps: usize,
    pub sent_dx: i32,
    pub sent_dy: i32,
    pub error: anyhow::Error,
}

/// Send every step on the calling (blocking) thread, pausing between steps.
/// Stops at the first failed send and reports what was already delivered.
pub fn send_steps(
    steps: &[RelativeStep],
    mut send: impl FnMut(i32, i32) -> anyhow::Result<()>,
) -> Result<(), Incomplete> {
    let (mut sent_dx, mut sent_dy) = (0, 0);
    for (index, step) in steps.iter().enumerate() {
        if let Err(error) = send(step.dx, step.dy) {
            return Err(Incomplete {
                sent_steps: index,
                sent_dx,
                sent_dy,
                error,
            });
        }
        sent_dx += step.dx;
        sent_dy += step.dy;
        if !step.pause_after.is_zero() {
            std::thread::sleep(step.pause_after);
        }
    }
    Ok(())
}

/// The result of a move. Success and a partly sent move carry an action
/// record; a move that sent nothing is a plain error.
pub fn outcome(
    request: RelativeMove,
    steps: usize,
    transport: ActionTransport,
    args: &Value,
    sent: Result<(), Incomplete>,
) -> ToolResult {
    let RelativeMove { dx, dy, .. } = request;
    match sent {
        Ok(()) if steps == 0 => ToolResult::text("dx and dy are both 0; no motion was sent.")
            .with_action_record(ActionExecutionRecord::relative_pointer_move(
                transport, args, None,
            )),
        Ok(()) => ToolResult::text(format!(
            "Sent relative pointer motion ({dx}, {dy}) in {steps} event{} to the focused app.",
            if steps == 1 { "" } else { "s" }
        ))
        .with_action_record(ActionExecutionRecord::relative_pointer_move(
            transport, args, None,
        )),
        Err(incomplete) if incomplete.sent_steps == 0 => not_sent(&incomplete.error),
        Err(incomplete) => ToolResult::text(format!(
            "Sent ({}, {}) of the requested ({dx}, {dy}) in {} of {steps} events, then the next \
             event failed: {:#}. The pointer has already moved by the part that was sent.",
            incomplete.sent_dx, incomplete.sent_dy, incomplete.sent_steps, incomplete.error
        ))
        .with_action_record(ActionExecutionRecord::relative_pointer_move(
            transport,
            args,
            Some(incomplete.sent_steps as u32),
        )),
    }
}

/// A move that failed before any event went out, such as a display or
/// input device that could not be opened.
pub fn not_sent(error: &anyhow::Error) -> ToolResult {
    ToolResult::error(format!("{TOOL_NAME}: no motion was sent: {error:#}")).with_structured(
        json!({
            "code": "delivery_failed",
            "tool": TOOL_NAME,
            "detail": format!("{error:#}"),
        }),
    )
}

/// Refusal for a backend with no relative pointer route. Nothing was sent.
pub fn unsupported(detail: impl Into<String>) -> ToolResult {
    let detail = detail.into();
    ToolResult::error(format!(
        "{TOOL_NAME} is not supported here: {detail}. No motion was sent."
    ))
    .with_structured(json!({
        "code": UNSUPPORTED_CODE,
        "tool": TOOL_NAME,
        "detail": detail,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sums(request: RelativeMove) -> (i32, i32, usize) {
        let steps = request.steps();
        (
            steps.iter().map(|step| step.dx).sum(),
            steps.iter().map(|step| step.dy).sum(),
            steps.len(),
        )
    }

    #[test]
    fn steps_sum_exactly_to_the_request() {
        for (dx, dy) in [
            (240, -30),
            (7, 3),
            (-1, 1),
            (1, 0),
            (0, -32767),
            (32767, -32767),
            (97, -13),
        ] {
            for duration_ms in [0, 1, 8, 9, 100, 300, 1001, MAX_DURATION_MS] {
                let request = RelativeMove {
                    dx,
                    dy,
                    duration_ms,
                };
                let (sum_x, sum_y, _) = sums(request);
                assert_eq!((sum_x, sum_y), (dx, dy), "{request:?}");
            }
        }
    }

    #[test]
    fn an_instant_move_is_one_event() {
        let steps = RelativeMove {
            dx: 240,
            dy: -30,
            duration_ms: 0,
        }
        .steps();
        assert_eq!(
            steps,
            vec![RelativeStep {
                dx: 240,
                dy: -30,
                pause_after: Duration::ZERO
            }]
        );

        // One count cannot be split, but the move still takes its duration.
        let steps = RelativeMove {
            dx: 1,
            dy: 0,
            duration_ms: 1000,
        }
        .steps();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].pause_after, Duration::from_millis(1000));
    }

    #[test]
    fn a_spread_move_paces_evenly_and_has_no_empty_step() {
        let request = RelativeMove {
            dx: 240,
            dy: -30,
            duration_ms: 300,
        };
        let steps = request.steps();
        assert_eq!(steps.len(), 37);
        assert!(steps.iter().all(|step| step.dx != 0 || step.dy != 0));
        let paused: Duration = steps.iter().map(|step| step.pause_after).sum();
        assert!(
            paused <= Duration::from_millis(300) && paused > Duration::from_millis(290),
            "{paused:?}"
        );

        // Fewer counts than frames: one count per event.
        let (_, _, count) = sums(RelativeMove {
            dx: 3,
            dy: -2,
            duration_ms: 1000,
        });
        assert_eq!(count, 3);
    }

    #[test]
    fn a_move_and_its_negation_split_symmetrically() {
        let forward = RelativeMove {
            dx: 97,
            dy: -13,
            duration_ms: 200,
        }
        .steps();
        let backward = RelativeMove {
            dx: -97,
            dy: 13,
            duration_ms: 200,
        }
        .steps();
        for (a, b) in forward.iter().zip(&backward) {
            assert_eq!((a.dx, a.dy), (-b.dx, -b.dy));
        }
    }

    #[test]
    fn a_zero_move_has_no_steps() {
        assert!(RelativeMove {
            dx: 0,
            dy: 0,
            duration_ms: 500
        }
        .steps()
        .is_empty());
    }

    #[test]
    fn parse_accepts_the_issue_example() {
        assert_eq!(
            parse(&json!({"dx": 240, "dy": -30, "duration_ms": 300})).unwrap(),
            RelativeMove {
                dx: 240,
                dy: -30,
                duration_ms: 300
            }
        );
        assert_eq!(parse(&json!({"dx": 5, "dy": 0})).unwrap().duration_ms, 0);
    }

    #[test]
    fn parse_refuses_bad_arguments_with_invalid_arguments() {
        for args in [
            json!({"dy": 1}),
            json!({"dx": 1}),
            json!({"dx": 1.5, "dy": 0}),
            json!({"dx": "1", "dy": 0}),
            json!({"dx": 32768, "dy": 0}),
            json!({"dx": 0, "dy": -32768}),
            json!({"dx": i64::MIN, "dy": 0}),
            json!({"dx": 0, "dy": i64::MAX}),
            json!({"dx": 0, "dy": 0, "duration_ms": -1}),
            json!({"dx": 0, "dy": 0, "duration_ms": 10_001}),
        ] {
            let refusal = parse(&args).expect_err(&args.to_string());
            assert_eq!(
                refusal.structured_content.as_ref().unwrap()["code"],
                "invalid_arguments",
                "{args}"
            );
        }
    }

    #[test]
    fn send_steps_reports_what_was_delivered_before_a_failure() {
        let steps = RelativeMove {
            dx: 30,
            dy: -9,
            duration_ms: 0,
        }
        .steps();
        assert!(send_steps(&steps, |_, _| Ok(())).is_ok());

        let steps = RelativeMove {
            dx: 3,
            dy: 0,
            duration_ms: 24,
        }
        .steps();
        let mut calls = 0;
        let incomplete = send_steps(&steps, |_, _| {
            calls += 1;
            if calls == 3 {
                anyhow::bail!("device gone")
            }
            Ok(())
        })
        .expect_err("third send fails");
        assert_eq!(
            (
                incomplete.sent_steps,
                incomplete.sent_dx,
                incomplete.sent_dy
            ),
            (2, 2, 0)
        );
    }

    #[test]
    fn outcome_is_partial_only_when_some_motion_was_sent() {
        let request = RelativeMove {
            dx: 3,
            dy: 0,
            duration_ms: 24,
        };
        let args = json!({"dx": 3, "dy": 0});
        let failed = |sent_steps, sent_dx| {
            Err(Incomplete {
                sent_steps,
                sent_dx,
                sent_dy: 0,
                error: anyhow::anyhow!("device gone"),
            })
        };

        let none = outcome(request, 3, ActionTransport::LinuxXTest, &args, failed(0, 0));
        assert_eq!(none.is_error, Some(true));
        assert_eq!(none.structured_content.unwrap()["code"], "delivery_failed");

        let some = outcome(request, 3, ActionTransport::LinuxXTest, &args, failed(2, 2));
        assert_ne!(some.is_error, Some(true));
        let record = some.action_record.expect("partial record");
        assert_eq!(record.delivered_count, Some(2));
        assert!(record.validate().is_ok());

        let done = outcome(request, 3, ActionTransport::LinuxXTest, &args, Ok(()));
        assert!(done.action_record.expect("record").validate().is_ok());
    }

    #[test]
    fn unsupported_names_the_code_and_sends_nothing() {
        let refusal = unsupported("no relative pointer route");
        assert_eq!(refusal.is_error, Some(true));
        let structured = refusal.structured_content.unwrap();
        assert_eq!(structured["code"], UNSUPPORTED_CODE);
        assert_eq!(structured["tool"], TOOL_NAME);
    }
}
