//! Shared tool-argument extraction helpers.
//!
//! Every tool in every platform crate does the same dance to pull
//! pid / window_id / element_index / text / etc. out of the inbound
//! JSON `Value`. Before this module they were 4-line `match` blocks
//! repeated 200+ times across the codebase with subtly different
//! error wording. The trait below consolidates those into one
//! consistent shape.
//!
//! Two flavours of accessor:
//!
//! - `require_*` — bails with a `ToolResult::error` if the field is
//!   missing or the wrong type. Narrowing casts (i64 → i32, u64 → u32)
//!   go through `try_from` and surface an actionable range error
//!   instead of silently truncating — the same CodeRabbit fix landed
//!   on PR #1666's page tool now applies uniformly.
//! - `opt_*` — returns `Option<T>` / `Result<Option<T>>` for callers
//!   that already have a sensible default. Range-checked variants
//!   return `Result<Option<T>>` because "the user passed a value but
//!   it's out of range" is still an error, just one we don't enforce
//!   when the field is absent.
//!
//! Error message format: `"Missing required {kind} field: {name}"`.
//! Used uniformly so MCP clients can pattern-match the wording.
//!
//! See `libs/cua-driver/rust/docs/dedup-audit.md` for the audit trail
//! that motivated this extraction.

use serde_json::Value;

use crate::protocol::ToolResult;

/// Format the canonical "missing required field" error.
#[inline]
fn missing(kind: &str, name: &str) -> ToolResult {
    ToolResult::error(format!("Missing required {kind} field: {name}"))
}

/// Format the canonical "wrong type" error. Used when the field IS
/// present but a different JSON type than the caller asked for.
#[inline]
fn wrong_type(kind: &str, name: &str) -> ToolResult {
    ToolResult::error(format!(
        "Field {name} has wrong type — expected {kind}"
    ))
}

/// Format the canonical out-of-range error for narrowing casts.
#[inline]
fn out_of_range(kind: &str, name: &str, raw: i128) -> ToolResult {
    ToolResult::error(format!(
        "Field {name} is out of range for {kind}: {raw}"
    ))
}

/// Extension trait on `&serde_json::Value` (the MCP `arguments` blob)
/// that provides typed, error-formatted accessors for every field
/// shape the tool surface uses.
pub trait ArgsExt {
    // ── Required scalars ──────────────────────────────────────────────────
    fn require_i32(&self, name: &str) -> Result<i32, ToolResult>;
    fn require_i64(&self, name: &str) -> Result<i64, ToolResult>;
    fn require_u32(&self, name: &str) -> Result<u32, ToolResult>;
    fn require_u64(&self, name: &str) -> Result<u64, ToolResult>;
    fn require_f64(&self, name: &str) -> Result<f64, ToolResult>;
    fn require_str(&self, name: &str) -> Result<String, ToolResult>;
    fn require_bool(&self, name: &str) -> Result<bool, ToolResult>;

    // ── Optional scalars ──────────────────────────────────────────────────
    /// Returns `None` if the field is absent. Returns `Err` if the
    /// field is present but doesn't fit in i32 — silent truncation
    /// is the bug class this trait exists to prevent.
    fn opt_i32(&self, name: &str) -> Result<Option<i32>, ToolResult>;
    fn opt_u32(&self, name: &str) -> Result<Option<u32>, ToolResult>;
    fn opt_u64(&self, name: &str) -> Option<u64>;
    fn opt_i64(&self, name: &str) -> Option<i64>;
    fn opt_f64(&self, name: &str) -> Option<f64>;
    fn opt_str(&self, name: &str) -> Option<String>;
    fn opt_bool(&self, name: &str) -> Option<bool>;

    // ── Default-fallback scalars (the most common pattern) ────────────────
    fn u64_or(&self, name: &str, default: u64) -> u64;
    fn i64_or(&self, name: &str, default: i64) -> i64;
    fn f64_or(&self, name: &str, default: f64) -> f64;
    fn str_or<'a>(&'a self, name: &str, default: &'a str) -> String;
    fn bool_or(&self, name: &str, default: bool) -> bool;

    // ── Arrays ────────────────────────────────────────────────────────────
    /// Extract an array of strings (for `modifiers`, `keys`, `urls`,
    /// `attributes`, etc.). Returns an empty vec if the field is
    /// absent or not an array. Non-string elements are skipped.
    fn str_array(&self, name: &str) -> Vec<String>;
}

impl ArgsExt for Value {
    fn require_i32(&self, name: &str) -> Result<i32, ToolResult> {
        let raw = self
            .get(name)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| missing("integer", name))?;
        i32::try_from(raw).map_err(|_| out_of_range("i32", name, raw as i128))
    }

    fn require_i64(&self, name: &str) -> Result<i64, ToolResult> {
        self.get(name)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| missing("integer", name))
    }

    fn require_u32(&self, name: &str) -> Result<u32, ToolResult> {
        let raw = self
            .get(name)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| missing("integer", name))?;
        u32::try_from(raw).map_err(|_| out_of_range("u32", name, raw as i128))
    }

    fn require_u64(&self, name: &str) -> Result<u64, ToolResult> {
        self.get(name)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| missing("integer", name))
    }

    fn require_f64(&self, name: &str) -> Result<f64, ToolResult> {
        self.get(name)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| missing("number", name))
    }

    fn require_str(&self, name: &str) -> Result<String, ToolResult> {
        self.get(name)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or_else(|| missing("string", name))
    }

    fn require_bool(&self, name: &str) -> Result<bool, ToolResult> {
        self.get(name)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| missing("boolean", name))
    }

    fn opt_i32(&self, name: &str) -> Result<Option<i32>, ToolResult> {
        match self.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                let raw = v.as_i64().ok_or_else(|| wrong_type("integer", name))?;
                i32::try_from(raw)
                    .map(Some)
                    .map_err(|_| out_of_range("i32", name, raw as i128))
            }
        }
    }

    fn opt_u32(&self, name: &str) -> Result<Option<u32>, ToolResult> {
        match self.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                let raw = v.as_u64().ok_or_else(|| wrong_type("integer", name))?;
                u32::try_from(raw)
                    .map(Some)
                    .map_err(|_| out_of_range("u32", name, raw as i128))
            }
        }
    }

    fn opt_u64(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(|v| v.as_u64())
    }

    fn opt_i64(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(|v| v.as_i64())
    }

    fn opt_f64(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(|v| v.as_f64())
    }

    fn opt_str(&self, name: &str) -> Option<String> {
        self.get(name)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    fn opt_bool(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(|v| v.as_bool())
    }

    fn u64_or(&self, name: &str, default: u64) -> u64 {
        self.opt_u64(name).unwrap_or(default)
    }

    fn i64_or(&self, name: &str, default: i64) -> i64 {
        self.opt_i64(name).unwrap_or(default)
    }

    fn f64_or(&self, name: &str, default: f64) -> f64 {
        self.opt_f64(name).unwrap_or(default)
    }

    fn str_or<'a>(&'a self, name: &str, default: &'a str) -> String {
        self.opt_str(name).unwrap_or_else(|| default.to_owned())
    }

    fn bool_or(&self, name: &str, default: bool) -> bool {
        self.opt_bool(name).unwrap_or(default)
    }

    fn str_array(&self, name: &str) -> Vec<String> {
        self.get(name)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn require_i32_happy_path() {
        let args = json!({ "pid": 1234 });
        assert_eq!(args.require_i32("pid").unwrap(), 1234);
    }

    #[test]
    fn require_i32_missing_returns_actionable_error() {
        let args = json!({});
        let err = args.require_i32("pid").unwrap_err();
        let body = format!("{:?}", err.content[0]);
        assert!(body.contains("Missing required integer field: pid"));
    }

    #[test]
    fn require_i32_out_of_range_rejected() {
        // i32::MAX + 1 doesn't fit in i32.
        let args = json!({ "pid": (i32::MAX as i64) + 1 });
        let err = args.require_i32("pid").unwrap_err();
        let body = format!("{:?}", err.content[0]);
        assert!(body.contains("out of range for i32"));
    }

    #[test]
    fn require_u32_out_of_range_rejected() {
        let args = json!({ "window_id": (u32::MAX as u64) + 1 });
        let err = args.require_u32("window_id").unwrap_err();
        let body = format!("{:?}", err.content[0]);
        assert!(body.contains("out of range for u32"));
    }

    #[test]
    fn require_str_missing_says_string() {
        let args = json!({});
        let err = args.require_str("text").unwrap_err();
        let body = format!("{:?}", err.content[0]);
        assert!(body.contains("Missing required string field: text"));
    }

    #[test]
    fn opt_u32_returns_none_when_absent() {
        let args = json!({});
        assert_eq!(args.opt_u32("window_id").unwrap(), None);
    }

    #[test]
    fn opt_u32_returns_none_when_null() {
        let args = json!({ "window_id": null });
        assert_eq!(args.opt_u32("window_id").unwrap(), None);
    }

    #[test]
    fn opt_u32_present_and_in_range() {
        let args = json!({ "window_id": 42 });
        assert_eq!(args.opt_u32("window_id").unwrap(), Some(42));
    }

    #[test]
    fn opt_u32_present_but_wrong_type_errors() {
        let args = json!({ "window_id": "not a number" });
        assert!(args.opt_u32("window_id").is_err());
    }

    #[test]
    fn opt_u32_present_but_out_of_range_errors() {
        let args = json!({ "window_id": (u32::MAX as u64) + 1 });
        let err = args.opt_u32("window_id").unwrap_err();
        let body = format!("{:?}", err.content[0]);
        assert!(body.contains("out of range for u32"));
    }

    #[test]
    fn u64_or_returns_default_when_absent() {
        let args = json!({});
        assert_eq!(args.u64_or("delay_ms", 30), 30);
    }

    #[test]
    fn u64_or_returns_value_when_present() {
        let args = json!({ "delay_ms": 100 });
        assert_eq!(args.u64_or("delay_ms", 30), 100);
    }

    #[test]
    fn str_or_uses_default_when_absent() {
        let args = json!({});
        assert_eq!(args.str_or("button", "left"), "left");
    }

    #[test]
    fn str_array_handles_absent_and_non_string_entries() {
        let args = json!({});
        assert_eq!(args.str_array("modifiers"), Vec::<String>::new());

        let args = json!({ "modifiers": ["ctrl", "shift", 42, "alt"] });
        assert_eq!(
            args.str_array("modifiers"),
            vec!["ctrl".to_owned(), "shift".to_owned(), "alt".to_owned()]
        );
    }

    #[test]
    fn bool_or_handles_typical_cases() {
        let args = json!({});
        assert!(!args.bool_or("from_zoom", false));
        let args = json!({ "from_zoom": true });
        assert!(args.bool_or("from_zoom", false));
    }
}
