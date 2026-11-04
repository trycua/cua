// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Extras of the agent cursor.
//!
//! The default agent cursor is a plain arrow with a soft halo that levitates
//! gently and points its tip along the path while it moves. Four saved settings in `~/.cua-driver/config.json`
//! (`{"cursor": {"marks": true}}`) switch the extras:
//!
//! - `cursor.marks`: the theme's action marks (observe arcs, click lines,
//!   I-beam, key cap and the rest), including the navigate and click marks a
//!   move or click used to start by itself, and the macOS focus rectangle
//!   around a clicked element. Off: every theme paints its idle frame.
//! - `cursor.float` (on by default): the float of the built-in theme, a slow
//!   vertical bob at rest and a smaller lift while the cursor travels that
//!   settles to exactly zero on arrival. The arrow stays upright. Off: the
//!   cursor never floats, so a resting overlay stops redrawing.
//! - `cursor.halo` (on by default): a soft glow in the cursor's accent colour
//!   behind the arrow body. No click effect.
//! - `cursor.rotate` (`heading` by default): `heading` points the tip along
//!   the path; `lean` tilts the arrow at most 30° toward where it is going;
//!   `off` keeps it upright. Both turning modes ease back upright before
//!   arrival. `on` is the same as `heading`.
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
pub const HALO_KEY: &str = "cursor.halo";

/// How the arrow turns while it travels (`cursor.rotate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RotateMode {
    /// Always upright.
    Off,
    /// Leans toward the direction of travel by at most 30°.
    Lean,
    /// The tip points along the path (the default).
    #[default]
    Heading,
}

impl RotateMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lean => "lean",
            Self::Heading => "heading",
        }
    }

    const fn to_bits(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Lean => 1,
            Self::Heading => 2,
        }
    }

    const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Lean,
            2 => Self::Heading,
            _ => Self::Off,
        }
    }
}

/// The cursor extras. `Default` is the float, the halo and `heading`; [`CursorOptions::NONE`]
/// is everything off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorOptions {
    pub marks: bool,
    pub float: bool,
    pub rotate: RotateMode,
    pub halo: bool,
}

impl Default for CursorOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl CursorOptions {
    /// The built-in default: a plain arrow with a halo that floats and points
    /// along its path while it moves.
    pub const DEFAULT: Self = Self {
        marks: false,
        float: true,
        rotate: RotateMode::Heading,
        halo: true,
    };

    /// Every extra off: a plain arrow that is still at rest.
    pub const NONE: Self = Self {
        marks: false,
        float: false,
        rotate: RotateMode::Off,
        halo: false,
    };

    /// Every extra on.
    pub const ALL_ON: Self = Self {
        marks: true,
        float: true,
        rotate: RotateMode::Heading,
        halo: true,
    };

    const KEYS: [&'static str; 4] = [MARKS_KEY, FLOAT_KEY, ROTATE_KEY, HALO_KEY];

    fn field(key: &str) -> &str {
        key.trim_start_matches("cursor.")
    }

    /// The value of one key as `get_config` and `set_config` show it.
    fn value_str(&self, key: &str) -> &'static str {
        let on_off = |flag: bool| if flag { "on" } else { "off" };
        match key {
            MARKS_KEY => on_off(self.marks),
            FLOAT_KEY => on_off(self.float),
            HALO_KEY => on_off(self.halo),
            _ => self.rotate.as_str(),
        }
    }

    fn apply(&mut self, update: Update) {
        let default = Self::DEFAULT;
        match update {
            Update::Marks(value) => self.marks = value.unwrap_or(default.marks),
            Update::Float(value) => self.float = value.unwrap_or(default.float),
            Update::Halo(value) => self.halo = value.unwrap_or(default.halo),
            Update::Rotate(value) => self.rotate = value.unwrap_or(default.rotate),
        }
    }

    const fn to_u8(self) -> u8 {
        self.marks as u8
            | (self.float as u8) << 1
            | self.rotate.to_bits() << 2
            | (self.halo as u8) << 4
    }

    const fn from_u8(value: u8) -> Self {
        Self {
            marks: value & 1 != 0,
            float: value & 2 != 0,
            rotate: RotateMode::from_bits((value >> 2) & 3),
            halo: value & 16 != 0,
        }
    }

    /// Value shown by `get_config` under `cursor`.
    pub fn config_json(&self) -> Map<String, Value> {
        Self::KEYS
            .iter()
            .map(|key| {
                let value = match *key {
                    MARKS_KEY => Value::Bool(self.marks),
                    FLOAT_KEY => Value::Bool(self.float),
                    HALO_KEY => Value::Bool(self.halo),
                    _ => Value::String(self.rotate.as_str().into()),
                };
                (Self::field(key).to_owned(), value)
            })
            .collect()
    }
}

/// One parsed write; `None` means the built-in default.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Update {
    Marks(Option<bool>),
    Float(Option<bool>),
    Halo(Option<bool>),
    Rotate(Option<RotateMode>),
}

impl Update {
    /// What the config file stores for this write; `None` removes the key.
    fn saved(self) -> Option<Value> {
        match self {
            Self::Marks(value) | Self::Float(value) | Self::Halo(value) => value.map(Value::Bool),
            Self::Rotate(value) => value.map(|mode| Value::String(mode.as_str().into())),
        }
    }
}

/// Parse one on/off value: `on`, `off` or `default` (also a bool or null,
/// like the `cursor.motion.effects.*` keys). `None` means the built-in
/// default.
pub fn parse_value(key: &str, value: &Value) -> Result<Option<bool>, String> {
    cua_driver_contract::CursorEffectSetting::from_json(value)
        .map(cua_driver_contract::CursorEffectSetting::override_value)
        .ok_or_else(|| format!("{key} must be on, off or default, got {value}"))
}

/// Parse a `cursor.rotate` value: `off`, `lean`, `heading` or `default`.
/// `on` (or `true`), from before the modes existed, means `heading`.
pub fn parse_rotate(value: &Value) -> Result<Option<RotateMode>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(true) => Ok(Some(RotateMode::Heading)),
        Value::Bool(false) => Ok(Some(RotateMode::Off)),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "default" => Ok(None),
            "off" => Ok(Some(RotateMode::Off)),
            "lean" => Ok(Some(RotateMode::Lean)),
            "heading" | "on" => Ok(Some(RotateMode::Heading)),
            _ => Err(format!(
                "{ROTATE_KEY} must be off, lean, heading or default, got {value}"
            )),
        },
        _ => Err(format!(
            "{ROTATE_KEY} must be off, lean, heading or default, got {value}"
        )),
    }
}

fn parse_update(key: &str, value: &Value) -> Result<Update, String> {
    Ok(match key {
        MARKS_KEY => Update::Marks(parse_value(key, value)?),
        FLOAT_KEY => Update::Float(parse_value(key, value)?),
        HALO_KEY => Update::Halo(parse_value(key, value)?),
        _ => Update::Rotate(parse_rotate(value)?),
    })
}

/// Read the saved options from `path`. A missing file or an invalid value
/// falls back to the built-in default; a bad value is logged, never fatal.
pub fn load_from(path: &Path) -> CursorOptions {
    let cursor = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|json| json.get("cursor").cloned());
    let mut options = CursorOptions::default();
    let Some(cursor) = cursor else {
        return options;
    };
    for key in CursorOptions::KEYS {
        if let Some(value) = cursor.get(CursorOptions::field(key)) {
            match parse_update(key, value) {
                Ok(update) => options.apply(update),
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
    for key in CursorOptions::KEYS {
        if let Some(value) = args.get(key) {
            updates.push((key, parse_update(key, value)?));
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
    for (key, update) in updates {
        let field = CursorOptions::field(key);
        match update.saved() {
            Some(value) => {
                cursor.insert(field.into(), value);
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

// Process-wide options every cursor reads per frame. They start at the
// built-in default so unit tests never read the user's file; the daemon
// loads them at startup (`CursorConfig::from_args`) and on every config
// write.
static ACTIVE: AtomicU8 = AtomicU8::new(CursorOptions::DEFAULT.to_u8());

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

/// Apply the `cursor.marks` / `cursor.float` / `cursor.rotate` /
/// `cursor.halo` writes in a
/// `set_config` call: validate, persist, and switch every cursor at once.
/// Returns `key=value` for each key written.
pub fn apply_config_args(args: &Value) -> Result<Vec<String>, String> {
    if !CursorOptions::KEYS
        .iter()
        .any(|key| args.get(*key).is_some())
    {
        return Ok(Vec::new());
    }
    let path = crate::motion_defaults::default_config_path()
        .ok_or_else(|| "$HOME is not set".to_string())?;
    let (options, keys) = set_at(&path, args)?;
    set_active(options);
    Ok(keys
        .into_iter()
        .map(|key| format!("{key}={}", options.value_str(key)))
        .collect())
}

/// Input-schema properties to merge into each platform's `set_config` tool.
pub fn config_schema_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    let on_off = ["on", "off", "default"];
    for (key, values, description) in [
        (
            MARKS_KEY,
            &on_off[..],
            "Agent cursor action marks: the small cue next to the arrow for observe, click, type, keys, scroll, drag, apps, files, recording and system calls, plus the focus box around a clicked element on macOS. off (default): a plain arrow. Applies immediately to every cursor.",
        ),
        (
            FLOAT_KEY,
            &on_off[..],
            "Agent cursor float: a slow four-second sway, bob and slight tilt, at rest and while moving, that eases to zero on every arrival and holds there for the click, drag, drop, scroll or typing, so they land exactly upright on the hotspot. on (default). off: the cursor never floats and a resting overlay stops redrawing. Applies immediately to every cursor.",
        ),
        (
            HALO_KEY,
            &on_off[..],
            "Agent cursor halo: a soft glow in the cursor's colour behind the arrow, so it is easier to follow on any background. on (default). off: no halo. Applies immediately to every cursor.",
        ),
        (
            ROTATE_KEY,
            &["off", "lean", "heading", "on", "default"][..],
            "Agent cursor rotation while it moves: heading (default) points the tip along the path; lean tilts the arrow at most 30 degrees toward where it is going (on is the same as heading). Both return upright before arrival, so clicks, typing and scrolling stay upright. off: the arrow always stays upright. Applies immediately to every cursor.",
        ),
    ] {
        properties.insert(
            key.to_owned(),
            json!({
                "type": "string",
                "enum": values,
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
    fn the_float_the_halo_and_heading_are_on_by_default() {
        let default = CursorOptions::default();
        assert!(default.float && default.halo);
        assert!(!default.marks);
        assert_eq!(default.rotate, RotateMode::Heading);
        assert_eq!(default, CursorOptions::from_u8(2 | 8 | 16));
        assert_eq!(CursorOptions::NONE, CursorOptions::from_u8(0));
        assert_eq!(active(), CursorOptions::DEFAULT);
        let path = temp_config("missing");
        assert_eq!(load_from(&path), CursorOptions::default());
    }

    #[test]
    fn the_float_can_be_turned_off_and_reset() {
        let path = temp_config("float-off");
        let (options, _) = set_at(&path, &json!({"cursor.float": "off"})).unwrap();
        assert_eq!(
            options,
            CursorOptions {
                halo: true,
                rotate: RotateMode::Heading,
                ..CursorOptions::NONE
            }
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["cursor"]["float"], false);

        let (options, _) = set_at(&path, &json!({"cursor.float": "default"})).unwrap();
        assert!(options.float, "default brings the float back");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn round_trips_through_the_bit_packing() {
        for marks in [false, true] {
            for float in [false, true] {
                for halo in [false, true] {
                    for rotate in [RotateMode::Off, RotateMode::Lean, RotateMode::Heading] {
                        let options = CursorOptions {
                            marks,
                            float,
                            rotate,
                            halo,
                        };
                        assert_eq!(CursorOptions::from_u8(options.to_u8()), options);
                    }
                }
            }
        }
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
                ..CursorOptions::DEFAULT
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
        for key in [MARKS_KEY, FLOAT_KEY, ROTATE_KEY, HALO_KEY] {
            assert!(properties.contains_key(key), "{key}");
        }
        assert_eq!(
            properties[ROTATE_KEY]["enum"],
            json!(["off", "lean", "heading", "on", "default"])
        );
    }

    #[test]
    fn rotate_takes_a_mode_and_on_means_heading() {
        let path = temp_config("rotate");
        let (options, keys) = set_at(&path, &json!({"cursor.rotate": "lean"})).unwrap();
        assert_eq!(keys, vec![ROTATE_KEY]);
        assert_eq!(options.rotate, RotateMode::Lean);
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["cursor"]["rotate"], "lean");
        assert_eq!(options.config_json()["rotate"], "lean");
        assert_eq!(options.value_str(ROTATE_KEY), "lean");

        for (value, mode) in [
            (json!("heading"), RotateMode::Heading),
            (json!("on"), RotateMode::Heading),
            (json!(true), RotateMode::Heading),
            (json!("off"), RotateMode::Off),
            (json!(false), RotateMode::Off),
            (json!("default"), RotateMode::Heading),
        ] {
            let (options, _) = set_at(&path, &json!({ "cursor.rotate": value })).unwrap();
            assert_eq!(options.rotate, mode, "{value}");
        }
        assert!(set_at(&path, &json!({"cursor.rotate": "spin"})).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_halo_can_be_turned_off() {
        let path = temp_config("halo");
        let (options, _) = set_at(&path, &json!({"cursor.halo": "off"})).unwrap();
        assert!(!options.halo);
        assert!(options.float);
        assert_eq!(options.config_json()["halo"], false);
        let (options, _) = set_at(&path, &json!({"cursor.halo": "default"})).unwrap();
        assert!(options.halo);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
