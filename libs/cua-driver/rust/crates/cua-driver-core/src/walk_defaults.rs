// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Saved defaults for the `get_window_state` walk budget (#6345).
//!
//! `get_window_state.max_elements` and `get_window_state.max_depth` live in
//! `~/.cua-driver/config.json` as `{"get_window_state": {"max_elements": N}}`.
//! A plain read (no `query`, no `full_output`) that omits the argument uses the
//! saved value in place of the built-in default; an argument passed on the call
//! always wins. See [`crate::window_state_view::ViewOptions::max_elements`].
//!
//! A saved value may not exceed the platform walk cap, the budget a
//! `full_output` read uses, so a default read is never larger than a full one.
//! A per-call argument is not bounded by it.

use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_ELEMENTS_KEY: &str = "get_window_state.max_elements";
pub const MAX_DEPTH_KEY: &str = "get_window_state.max_depth";
const SECTION: &str = "get_window_state";
const KEYS: [&str; 2] = [MAX_ELEMENTS_KEY, MAX_DEPTH_KEY];

/// A platform walker's own caps: the budget a `full_output` read uses.
/// `max_depth: None` means the walker has no depth cap (Linux).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkLimits {
    pub max_elements: usize,
    pub max_depth: Option<usize>,
}

/// The saved defaults; `None` means nothing is saved for that key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WalkDefaults {
    pub max_elements: Option<usize>,
    pub max_depth: Option<usize>,
}

fn field(key: &str) -> &str {
    key.trim_start_matches("get_window_state.")
}

fn cap(key: &str, limits: WalkLimits) -> Option<usize> {
    if key == MAX_ELEMENTS_KEY {
        Some(limits.max_elements)
    } else {
        limits.max_depth
    }
}

/// Parse one value: a whole number from 1 to the platform cap, or `null` /
/// `"default"` to clear the saved value.
fn parse(key: &str, value: &Value, limits: WalkLimits) -> Result<Option<usize>, String> {
    let range = match cap(key, limits) {
        Some(max) => format!("a whole number from 1 to {max}"),
        None => "a whole number of at least 1".to_owned(),
    };
    let invalid = || format!("{key} must be {range}, or default; got {value}");
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text.trim().eq_ignore_ascii_case("default") => Ok(None),
        Value::Number(number) => {
            let n = number
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(invalid)?;
            if n == 0 || cap(key, limits).is_some_and(|max| n > max) {
                return Err(invalid());
            }
            Ok(Some(n))
        }
        _ => Err(invalid()),
    }
}

impl WalkDefaults {
    fn get(&self, key: &str) -> Option<usize> {
        if key == MAX_ELEMENTS_KEY {
            self.max_elements
        } else {
            self.max_depth
        }
    }

    fn set(&mut self, key: &str, value: Option<usize>) {
        if key == MAX_ELEMENTS_KEY {
            self.max_elements = value;
        } else {
            self.max_depth = value;
        }
    }
}

/// Read the saved defaults from `path`. A missing file or a value that is out
/// of range for `limits` reads as unset; a bad value is logged, never fatal.
pub fn load_from(path: &Path, limits: WalkLimits) -> WalkDefaults {
    let section = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|json| json.get(SECTION).cloned());
    let mut defaults = WalkDefaults::default();
    for key in KEYS {
        if let Some(value) = section.as_ref().and_then(|s| s.get(field(key))) {
            match parse(key, value, limits) {
                Ok(n) => defaults.set(key, n),
                Err(error) => tracing::warn!("ignoring saved {error}"),
            }
        }
    }
    defaults
}

/// Validate and persist the walk keys present in `args` to `path`, keeping
/// every other key in the file. Nothing is written when any value is invalid.
/// Returns the saved defaults and the keys written.
pub fn set_at(
    path: &Path,
    args: &Value,
    limits: WalkLimits,
) -> Result<(WalkDefaults, Vec<&'static str>), String> {
    let mut updates = Vec::new();
    for key in KEYS {
        if let Some(value) = args.get(key) {
            updates.push((key, parse(key, value, limits)?));
        }
    }
    if updates.is_empty() {
        return Ok((load_from(path, limits), Vec::new()));
    }
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let object = root.as_object_mut().expect("root is an object");
    let section = object.entry(SECTION).or_insert_with(|| json!({}));
    if !section.is_object() {
        *section = json!({});
    }
    let section = section.as_object_mut().expect("section is an object");
    let mut keys = Vec::new();
    for (key, value) in updates {
        match value {
            Some(n) => section.insert(field(key).into(), json!(n)),
            None => section.remove(field(key)),
        };
        keys.push(key);
    }
    if section.is_empty() {
        object.remove(SECTION);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    std::fs::write(path, body)
        .map_err(|error| format!("failed to persist get_window_state defaults: {error}"))?;
    Ok((load_from(path, limits), keys))
}

// Process-wide saved defaults every `get_window_state` reads. They start
// unset so unit tests never read the user's file; each platform loads them
// when it builds its tool state and on every config write. 0 means unset.
static MAX_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
static MAX_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// The saved defaults `get_window_state` uses right now.
pub fn active() -> WalkDefaults {
    let read = |slot: &AtomicUsize| Some(slot.load(Ordering::Relaxed)).filter(|&n| n > 0);
    WalkDefaults {
        max_elements: read(&MAX_ELEMENTS),
        max_depth: read(&MAX_DEPTH),
    }
}

fn set_active(defaults: WalkDefaults) {
    MAX_ELEMENTS.store(defaults.max_elements.unwrap_or(0), Ordering::Relaxed);
    MAX_DEPTH.store(defaults.max_depth.unwrap_or(0), Ordering::Relaxed);
}

/// Load `~/.cua-driver/config.json` into the process-wide value.
pub fn load_active(limits: WalkLimits) -> WalkDefaults {
    let defaults = crate::config_file::default_config_path()
        .map(|path| load_from(&path, limits))
        .unwrap_or_default();
    set_active(defaults);
    defaults
}

/// Apply the walk-default writes in a `set_config` call: validate, persist,
/// and switch `get_window_state` at once. Returns `key=value` for each key
/// written.
pub fn apply_config_args(args: &Value, limits: WalkLimits) -> Result<Vec<String>, String> {
    if !KEYS.iter().any(|key| args.get(*key).is_some()) {
        return Ok(Vec::new());
    }
    let path = crate::config_file::default_config_path()
        .ok_or_else(|| "$HOME is not set".to_string())?;
    let (defaults, keys) = set_at(&path, args, limits)?;
    set_active(defaults);
    Ok(keys
        .into_iter()
        .map(|key| match defaults.get(key) {
            Some(n) => format!("{key}={n}"),
            None => format!("{key}=default"),
        })
        .collect())
}

/// The `get_window_state` object of `get_config`: the budget a plain read
/// that omits the argument walks with, and what is saved.
pub fn config_json(limits: WalkLimits) -> Value {
    let saved = active();
    json!({
        "max_elements": saved.max_elements.unwrap_or(
            crate::window_state_view::DEFAULT_MAX_ELEMENTS.min(limits.max_elements)
        ),
        "max_depth": saved.max_depth.or(limits.max_depth),
        "saved": {
            "max_elements": saved.max_elements,
            "max_depth": saved.max_depth,
        },
    })
}

/// Input-schema properties to merge into each platform's `set_config` tool.
pub fn config_schema_properties(limits: WalkLimits) -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        MAX_ELEMENTS_KEY.to_owned(),
        json!({
            "type": ["integer", "string", "null"],
            "description": format!(
                "Saved get_window_state node budget for reads that omit max_elements and pass no \
                 query or full_output: 1 to {} (the full_output budget), or default to clear it \
                 and use {}. A max_elements passed on the call always wins. Saved to \
                 ~/.cua-driver/config.json and applied at once.",
                limits.max_elements,
                crate::window_state_view::DEFAULT_MAX_ELEMENTS.min(limits.max_elements),
            ),
        }),
    );
    let depth_range = match limits.max_depth {
        Some(max) => format!("1 to {max}"),
        None => "1 or more".to_owned(),
    };
    let depth_default = match limits.max_depth {
        Some(max) => max.to_string(),
        None => "no depth cap".to_owned(),
    };
    properties.insert(
        MAX_DEPTH_KEY.to_owned(),
        json!({
            "type": ["integer", "string", "null"],
            "description": format!(
                "Saved get_window_state depth cap for reads that omit max_depth and pass no query \
                 or full_output: {depth_range}, or default to clear it and use {depth_default}. A \
                 max_depth passed on the call always wins. Saved to ~/.cua-driver/config.json and \
                 applied at once."
            ),
        }),
    );
    properties
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: WalkLimits = WalkLimits {
        max_elements: 2000,
        max_depth: Some(25),
    };
    const LINUX: WalkLimits = WalkLimits {
        max_elements: 5000,
        max_depth: None,
    };

    fn config_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join(".cua-driver").join("config.json")
    }

    #[test]
    fn saved_defaults_round_trip_and_keep_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"max_image_dimension": 1568}"#).unwrap();

        let (saved, keys) = set_at(
            &path,
            &json!({MAX_ELEMENTS_KEY: 800, MAX_DEPTH_KEY: 12}),
            MAC,
        )
        .unwrap();
        assert_eq!(keys, [MAX_ELEMENTS_KEY, MAX_DEPTH_KEY]);
        assert_eq!(
            saved,
            WalkDefaults {
                max_elements: Some(800),
                max_depth: Some(12),
            }
        );
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            file,
            json!({
                "max_image_dimension": 1568,
                "get_window_state": {"max_elements": 800, "max_depth": 12},
            })
        );
        assert_eq!(load_from(&path, MAC), saved);
    }

    #[test]
    fn default_and_null_clear_the_saved_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path(&dir);
        set_at(&path, &json!({MAX_ELEMENTS_KEY: 800, MAX_DEPTH_KEY: 12}), MAC).unwrap();

        let (saved, _) = set_at(&path, &json!({MAX_ELEMENTS_KEY: "default"}), MAC).unwrap();
        assert_eq!(saved.max_elements, None);
        assert_eq!(saved.max_depth, Some(12));
        let (saved, _) = set_at(&path, &json!({MAX_DEPTH_KEY: null}), MAC).unwrap();
        assert_eq!(saved, WalkDefaults::default());
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file, json!({}));
    }

    #[test]
    fn out_of_range_values_are_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path(&dir);
        set_at(&path, &json!({MAX_ELEMENTS_KEY: 800}), MAC).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        for (args, error) in [
            (
                json!({MAX_ELEMENTS_KEY: 2001}),
                "get_window_state.max_elements must be a whole number from 1 to 2000, or default; got 2001",
            ),
            (
                json!({MAX_ELEMENTS_KEY: 0}),
                "get_window_state.max_elements must be a whole number from 1 to 2000, or default; got 0",
            ),
            (
                json!({MAX_DEPTH_KEY: 26}),
                "get_window_state.max_depth must be a whole number from 1 to 25, or default; got 26",
            ),
            (
                json!({MAX_DEPTH_KEY: -1}),
                "get_window_state.max_depth must be a whole number from 1 to 25, or default; got -1",
            ),
            (
                json!({MAX_DEPTH_KEY: 2.5}),
                "get_window_state.max_depth must be a whole number from 1 to 25, or default; got 2.5",
            ),
            (
                json!({MAX_ELEMENTS_KEY: "lots"}),
                "get_window_state.max_elements must be a whole number from 1 to 2000, or default; got \"lots\"",
            ),
            // A valid key alongside an invalid one writes neither.
            (
                json!({MAX_ELEMENTS_KEY: 900, MAX_DEPTH_KEY: 0}),
                "get_window_state.max_depth must be a whole number from 1 to 25, or default; got 0",
            ),
        ] {
            assert_eq!(set_at(&path, &args, MAC).unwrap_err(), error);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        }
    }

    #[test]
    fn an_uncapped_platform_accepts_any_positive_depth() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path(&dir);
        let (saved, _) = set_at(&path, &json!({MAX_DEPTH_KEY: 200}), LINUX).unwrap();
        assert_eq!(saved.max_depth, Some(200));
        assert_eq!(
            set_at(&path, &json!({MAX_DEPTH_KEY: 0}), LINUX).unwrap_err(),
            "get_window_state.max_depth must be a whole number of at least 1, or default; got 0"
        );
    }

    #[test]
    fn a_saved_value_out_of_range_for_this_platform_reads_as_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = config_path(&dir);
        set_at(&path, &json!({MAX_ELEMENTS_KEY: 4000, MAX_DEPTH_KEY: 40}), LINUX).unwrap();
        assert_eq!(load_from(&path, MAC), WalkDefaults::default());
        std::fs::write(&path, r#"{"get_window_state": {"max_elements": "x"}}"#).unwrap();
        assert_eq!(load_from(&path, MAC), WalkDefaults::default());
    }
}
