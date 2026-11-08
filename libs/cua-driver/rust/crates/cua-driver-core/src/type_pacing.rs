// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared pacing contract for `type_text` key-event delivery.
//!
//! `delay_ms` defaults to 0, which means "send the text as fast as the target
//! can take it": printable characters go out in batches of at most
//! [`BATCH_UNITS`] events, and Return and Tab stay real key presses between
//! batches. A positive `delay_ms` means one paced key per character, for
//! debounced search boxes and autocomplete. When a target takes only part of
//! a batch, the undelivered tail is retyped paced at [`TAIL_DELAY_MS`].
//!
//! The macOS, Windows and Linux backends share this contract; only the
//! delivery primitive differs. This module holds the platform-neutral part:
//! the default, the clamp and the chunk planner.

use serde_json::Value;
use std::ops::Range;

/// Default `delay_ms`. Zero selects batched delivery.
pub const DEFAULT_DELAY_MS: u64 = 0;

/// Largest accepted `delay_ms`.
pub const MAX_DELAY_MS: u64 = 200;

/// Most text units (UTF-16 units or keycodes) in one batch.
pub const BATCH_UNITS: usize = 16;

/// Per-character pause when a batch's undelivered tail is retyped.
pub const TAIL_DELAY_MS: u64 = 30;

/// How the units of a text are grouped for delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    /// Part of a batch.
    Text,
    /// First half of a surrogate pair: stays in the same batch as the next unit.
    Lead,
    /// A key that must be a real press of its own (Return, Tab).
    Key,
}

/// The `delay_ms` a call asked for: the default when absent, clamped to
/// [`MAX_DELAY_MS`].
pub fn delay_from_args(args: &Value) -> u64 {
    args.get("delay_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_DELAY_MS)
        .min(MAX_DELAY_MS)
}

/// Whether `delay_ms` selects batched delivery.
pub fn is_batched(delay_ms: u64) -> bool {
    delay_ms == 0
}

/// Split `kinds` into delivery ranges. A [`UnitKind::Key`] is a range of its
/// own; consecutive other units share a range of at most `max` units, and a
/// [`UnitKind::Lead`] is never separated from the unit after it. The ranges
/// cover every unit exactly once, in order.
pub fn plan_chunks(kinds: &[UnitKind], max: usize) -> Vec<Range<usize>> {
    let max = max.max(2);
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i] == UnitKind::Key {
            if start < i {
                out.push(start..i);
            }
            out.push(i..i + 1);
            i += 1;
            start = i;
            continue;
        }
        let mut end = i + 1;
        if kinds[i] == UnitKind::Lead && end < kinds.len() && kinds[end] != UnitKind::Key {
            end += 1;
        }
        // Close the batch before this unit when it would overflow.
        if end - start > max {
            out.push(start..i);
            start = i;
        }
        i = end;
    }
    if start < kinds.len() {
        out.push(start..kinds.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use UnitKind::*;

    fn text(n: usize) -> Vec<UnitKind> {
        vec![Text; n]
    }

    #[test]
    fn default_is_batched_and_clamped() {
        assert_eq!(delay_from_args(&serde_json::json!({})), 0);
        assert_eq!(delay_from_args(&serde_json::json!({"delay_ms": 30})), 30);
        assert_eq!(delay_from_args(&serde_json::json!({"delay_ms": 9000})), 200);
        assert!(is_batched(0));
        assert!(!is_batched(1));
    }

    #[test]
    fn splits_at_the_limit() {
        let ranges = plan_chunks(&text(40), 16);
        assert_eq!(ranges, vec![0..16, 16..32, 32..40]);
    }

    #[test]
    fn keys_are_their_own_chunks() {
        let kinds = [Text, Text, Key, Text, Key, Key, Text];
        assert_eq!(
            plan_chunks(&kinds, 16),
            vec![0..2, 2..3, 3..4, 4..5, 5..6, 6..7]
        );
    }

    #[test]
    fn surrogate_pairs_are_never_split() {
        // 15 text units, then a pair: the pair would straddle the limit.
        let mut kinds = text(15);
        kinds.extend([Lead, Text]);
        let ranges = plan_chunks(&kinds, 16);
        assert_eq!(ranges, vec![0..15, 15..17]);
        // Pairs inside a batch stay whole at every limit.
        for max in 2..8 {
            let mut kinds = Vec::new();
            for _ in 0..9 {
                kinds.extend([Lead, Text]);
            }
            for r in plan_chunks(&kinds, max) {
                assert_eq!(kinds[r.start], Lead, "limit {max}");
                assert_eq!(r.len() % 2, 0, "limit {max}");
            }
        }
    }

    #[test]
    fn ranges_cover_everything_in_order() {
        let kinds: Vec<UnitKind> = (0..200)
            .map(|i| match i % 23 {
                0 => Key,
                5 => Lead,
                6 => Text,
                _ => Text,
            })
            .collect();
        let ranges = plan_chunks(&kinds, BATCH_UNITS);
        let mut next = 0;
        for r in &ranges {
            assert_eq!(r.start, next);
            assert!(r.len() <= BATCH_UNITS);
            next = r.end;
        }
        assert_eq!(next, kinds.len());
    }

    #[test]
    fn empty_text_has_no_chunks() {
        assert!(plan_chunks(&[], 16).is_empty());
    }

    #[test]
    fn a_lone_trailing_lead_still_terminates() {
        assert_eq!(plan_chunks(&[Text, Lead], 16), vec![0..2]);
        assert_eq!(plan_chunks(&[Lead, Key], 16), vec![0..1, 1..2]);
    }
}
