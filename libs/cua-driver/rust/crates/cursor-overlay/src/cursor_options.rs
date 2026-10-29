// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in extras of the agent cursor, all off by default.
//!
//! The default agent cursor is a plain, upright arrow that only moves when
//! the agent moves it. Three saved settings in `~/.cua-driver/config.json`
//! (`{"cursor": {"marks": true}}`) bring the extras back:
//!
//! - `cursor.marks`: the theme's action marks (observe arcs, click lines,
//!   I-beam, key cap and the rest), including the navigate and click marks a
//!   move or click used to start by itself, and the macOS focus rectangle
//!   around a clicked element. Off: every theme paints its idle frame.
//! - `cursor.float`: the resting float of the built-in theme (a slow bob and
//!   tilt). Off: a resting cursor is still, so the overlay stops redrawing.
//! - `cursor.rotate`: the arrow turns toward the direction it travels and
//!   settles back upright on arrival. Off: the arrow stays upright.
//!
//! Like `cursor.badges` they apply live: every cursor reads the process-wide
//! value on each frame, so a `set_config` write changes cursors already on
//! screen.

use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

pub const MARKS_KEY: &str = "cursor.marks";
pub const FLOAT_KEY: &str = "cursor.float";
pub const ROTATE_KEY: &str = "cursor.rotate";

/// The opt-in cursor extras. `Default` is everything off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CursorOptions {
    pub marks: bool,
    pub float: bool,
    pub rotate: bool,
}

impl CursorOptions {
    /// Every extra on: the cursor as it looked before these settings.
    pub const ALL_ON: Self = Self {
        marks: true,
        float: true,
        rotate: true,
    };

    const KEYS: [(&'static str, &'static str); 3] = [
        (MARKS_KEY, "marks"),
        (FLOAT_KEY, "float"),
        (ROTATE_KEY, "rotate"),
    ];

    fn get(&self, field: &str) -> bool {
        match field {
            "marks" => self.marks,
            "float" => self.float,
            _ => self.rotate,
        }
    }

    fn set(&mut self, field: &str, value: bool) {
        match field {
            "marks" => self.marks = value,
            "float" => self.float = value,
            _ => self.rotate = value,
        }
    }

    const fn to_u8(self) -> u8 {
        self.marks as u8 | (self.float as u8) << 1 | (self.rotate as u8) << 2
    }

    const fn from_u8(value: u8) -> Self {
        Self {
            marks: value & 1 != 0,
            float: value & 2 != 0,
            rotate: value & 4 != 0,
        }
    }

    /// Value shown by `get_config` under `cursor`.
    pub fn config_json(&self) -> Map<String, Value> {
        Self::KEYS
            .iter()
            .map(|(_, field)| ((*field).to_owned(), Value::Bool(self.get(field))))
            .collect()
    }
}

/// Parse one value: `on`, `off` or `default` (also a bool or null, like the
/// `cursor.motion.effects.*` keys). `None` means the built-in default (off).
pub fn parse_value(key: &str, value: &Value) -> Result<Option<bool>, String> {
    cua_driver_contract::CursorEffectSetting::from_json(value)
        .map(cua_driver_contract::CursorEffectSetting::override_value)
        .ok_or_else(|| format!("{key} must be on, off or default, got {value}"))
}

/// Read the saved options from `path`. A missing file or an invalid value
/// falls back to off; a bad value is logged, never fatal.
pub fn load_from(path: &Path) -> CursorOptions {
    let cursor = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|json| json.get("cursor").cloned());
    let mut options = CursorOptions::default();
    let Some(cursor) = cursor else {
        return options;
    };
    for (key, field) in CursorOptions::KEYS {
        if let Some(value) = cursor.get(field) {
            match parse_value(key, value) {
                Ok(flag) => options.set(field, flag.unwrap_or(false)),
                Err(error) => tracing::warn!("ignoring saved {key}: {error}"),
            }
        }
    }
    options
}

/// Validate and persist the option keys present in `args` to `path`,
/// keeping every other key in the file. Nothing is written when any value
/// is invalid. Returns the new options and the keys written.
pub fn set_at(path: &Path, args: &Value) -> Result<(CursorOptions, Vec<&'static str>), String> {
    let mut updates = Vec::new();
    for (key, field) in CursorOptions::KEYS {
        if let Some(value) = args.get(key) {
            updates.push((key, field, parse_value(key, value)?));
        }
    }
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if updates.is_empty() {
        return Ok((load_from(path), Vec::new()));
    }
    let object = root.as_object_mut().expect("root is an object");
    let cursor = object.entry("cursor").or_insert_with(|| json!({}));
    if !cursor.is_object() {
        *cursor = json!({});
    }
    let cursor = cursor.as_object_mut().expect("cursor is an object");
    let mut keys = Vec::new();
    for (key, field, flag) in updates {
        match flag {
            Some(flag) => {
                cursor.insert(field.into(), Value::Bool(flag));
            }
            None => {
                cursor.remove(field);
            }
        }
        keys.push(key);
    }
    if cursor.is_empty() {
        object.remove("cursor");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    std::fs::write(path, body)
        .map_err(|error| format!("failed to persist cursor options: {error}"))?;
    Ok((load_from(path), keys))
}

// Process-wide options every cursor reads per frame. They start off so unit
// tests never read the user's file; the daemon loads them at startup
// (`CursorConfig::from_args`) and on every config write.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// The options cursors paint with right now.
pub fn active() -> CursorOptions {
    CursorOptions::from_u8(ACTIVE.load(Ordering::Relaxed))
}

pub fn set_active(options: CursorOptions) {
    ACTIVE.store(options.to_u8(), Ordering::Relaxed);
}

/// Load `~/.cua-driver/config.json` into the process-wide value.
pub fn load_active() -> CursorOptions {
    let options = read_saved();
    set_active(options);
    options
}

/// What is saved on disk right now (for `get_config`).
pub fn read_saved() -> CursorOptions {
    crate::motion_defaults::default_config_path()
        .map(|path| load_from(&path))
        .unwrap_or_default()
}

/// The `cursor` object of `get_config`: the saved motion defaults, the
/// badge mode and these extras, as saved on disk right now.
pub fn saved_config_json() -> Value {
    let mut cursor = read_saved().config_json();
    cursor.insert(
        "motion".into(),
        crate::motion_defaults::read_saved().config_json(),
    );
    cursor.insert(
        "badges".into(),
        crate::badge_mode::read_saved().as_str().into(),
    );
    Value::Object(cursor)
}

/// Apply the `cursor.marks` / `cursor.float` / `cursor.rotate` writes in a
/// `set_config` call: validate, persist, and switch every cursor at once.
/// Returns `key=value` for each key written.
pub fn apply_config_args(args: &Value) -> Result<Vec<String>, String> {
    if !CursorOptions::KEYS
        .iter()
        .any(|(key, _)| args.get(*key).is_some())
    {
        return Ok(Vec::new());
    }
    let path = crate::motion_defaults::default_config_path()
        .ok_or_else(|| "$HOME is not set".to_string())?;
    let (options, keys) = set_at(&path, args)?;
    set_active(options);
    Ok(keys
        .into_iter()
        .map(|key| {
            let field = key.trim_start_matches("cursor.");
            format!("{key}={}", if options.get(field) { "on" } else { "off" })
        })
        .collect())
}

/// Input-schema properties to merge into each platform's `set_config` tool.
pub fn config_schema_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    for (key, description) in [
        (
            MARKS_KEY,
            "Agent cursor action marks: the small cue next to the arrow for observe, click, type, keys, scroll, drag, apps, files, recording and system calls, plus the focus box around a clicked element on macOS. off (default): a plain arrow. Applies immediately to every cursor.",
        ),
        (
            FLOAT_KEY,
            "Agent cursor resting float: a slow bob and tilt while the cursor rests. off (default): the cursor stays still at rest. Applies immediately to every cursor.",
        ),
        (
            ROTATE_KEY,
            "Agent cursor rotation: the arrow turns toward the direction it moves and settles upright on arrival. off (default): the arrow always stays upright. Applies immediately to every cursor.",
        ),
    ] {
        properties.insert(
            key.to_owned(),
            json!({
                "type": "string",
                "enum": ["on", "off", "default"],
                "description": description,
            }),
        );
    }
    properties
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cursor-options-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir.join("config.json")
    }

    #[test]
    fn everything_is_off_by_default() {
        assert_eq!(CursorOptions::default(), CursorOptions::from_u8(0));
        let path = temp_config("missing");
        assert_eq!(load_from(&path), CursorOptions::default());
    }

    #[test]
    fn round_trips_through_the_bit_packing() {
        for bits in 0..8 {
            assert_eq!(CursorOptions::from_u8(bits).to_u8(), bits);
        }
        assert_eq!(CursorOptions::ALL_ON.to_u8(), 7);
    }

    #[test]
    fn set_at_persists_and_keeps_other_keys() {
        let path = temp_config("persist");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"max_image_dimension": 1024, "cursor": {"badges": "verbose"}}"#,
        )
        .unwrap();

        let (options, keys) =
            set_at(&path, &json!({"cursor.marks": "on", "cursor.float": true})).unwrap();
        assert_eq!(keys, vec![MARKS_KEY, FLOAT_KEY]);
        assert_eq!(
            options,
            CursorOptions {
                marks: true,
                float: true,
                rotate: false
            }
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["max_image_dimension"], 1024);
        assert_eq!(saved["cursor"]["badges"], "verbose");
        assert_eq!(saved["cursor"]["marks"], true);

        let (options, _) = set_at(&path, &json!({"cursor.marks": "default"})).unwrap();
        assert!(!options.marks);
        assert!(options.float);
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved["cursor"].get("marks").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn invalid_values_write_nothing() {
        let path = temp_config("invalid");
        let error = set_at(&path, &json!({"cursor.float": "sometimes"})).unwrap_err();
        assert!(error.contains(FLOAT_KEY), "{error}");
        assert!(!path.exists());
    }

    #[test]
    fn schema_lists_every_key() {
        let properties = config_schema_properties();
        for key in [MARKS_KEY, FLOAT_KEY, ROTATE_KEY] {
            assert!(properties.contains_key(key), "{key}");
        }
    }
}
