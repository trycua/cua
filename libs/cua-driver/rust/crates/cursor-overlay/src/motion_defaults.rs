//! Saved default cursor motion.
//!
//! `cursor.motion.style`, `cursor.motion.timing` and
//! `cursor.motion.effects.<name>` live in `~/.cua-driver/config.json`
//! (`{"cursor": {"motion": {...}}}`) next to the other persistent driver
//! settings. They seed the motion of every session cursor created after the
//! value is read. Precedence, highest first: a per-call
//! `set_agent_cursor_motion`, the `cursor_motion` of `start_session`, this
//! saved default, the built-in `signature_arc`. A reduced-motion theme or
//! policy always wins at render time, so it overrides all of them.

use crate::motion::{MotionConfig, MotionEffects, MotionStyle, MotionTiming};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Config keys this module owns.
pub const KEY_PREFIX: &str = "cursor.motion";
pub const STYLE_KEY: &str = "cursor.motion.style";
pub const TIMING_KEY: &str = "cursor.motion.timing";
pub const EFFECT_NAMES: [&str; 5] = ["trail", "glow", "magnet", "ripple", "squish"];

/// Saved motion defaults. `None` means "not set": the built-in applies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SavedMotionDefaults {
    pub style: Option<MotionStyle>,
    pub timing: Option<MotionTiming>,
    pub effects: MotionEffects,
}

impl SavedMotionDefaults {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Layer these defaults over `motion` (used for a new cursor, before any
    /// session or per-call override).
    pub fn apply(&self, motion: &mut MotionConfig) {
        if let Some(style) = self.style {
            motion.style = style;
        }
        if let Some(timing) = self.timing {
            motion.timing = timing;
        }
        let effects = &mut motion.effects;
        effects.trail = self.effects.trail.or(effects.trail);
        effects.glow = self.effects.glow.or(effects.glow);
        effects.magnet = self.effects.magnet.or(effects.magnet);
        effects.ripple = self.effects.ripple.or(effects.ripple);
        effects.squish = self.effects.squish.or(effects.squish);
    }

    /// Value shown by `get_config` under `cursor.motion`: the effective style
    /// and timing, plus only the effects that are explicitly saved.
    pub fn config_json(&self) -> Value {
        let mut effects = Map::new();
        for (name, value) in [
            ("trail", self.effects.trail),
            ("glow", self.effects.glow),
            ("magnet", self.effects.magnet),
            ("ripple", self.effects.ripple),
            ("squish", self.effects.squish),
        ] {
            if let Some(value) = value {
                effects.insert(name.to_owned(), Value::Bool(value));
            }
        }
        json!({
            "style": self.style.unwrap_or_default().as_str(),
            "timing": self.timing.unwrap_or_default().as_str(),
            "effects": effects,
        })
    }
}

/// `~/.cua-driver/config.json`, the file `set_config` writes everywhere.
pub fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".cua-driver").join("config.json"))
}

fn allowed_styles() -> String {
    MotionStyle::ALL.map(MotionStyle::as_str).join(", ")
}

fn effect_flag(key: &str, value: &Value) -> Result<Option<bool>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(flag) => Ok(Some(*flag)),
        Value::String(text) => match text.as_str() {
            "true" | "on" => Ok(Some(true)),
            "false" | "off" => Ok(Some(false)),
            "default" => Ok(None),
            _ => Err(format!(
                "{key} must be true, false or default, got `{text}`"
            )),
        },
        other => Err(format!("{key} must be true, false or default, got {other}")),
    }
}

/// Validate one config write and fold it into `saved`.
///
/// Accepted keys: `cursor.motion.style`, `cursor.motion.timing`,
/// `cursor.motion.effects.<trail|glow|magnet|ripple|squish>`, and
/// `cursor.motion` itself with a null/`"default"` value to clear all of them.
/// A null or `"default"` value clears a single key. Errors name the allowed
/// values.
pub fn apply_key(saved: &mut SavedMotionDefaults, key: &str, value: &Value) -> Result<(), String> {
    let clear = value.is_null() || value.as_str() == Some("default");
    match key {
        KEY_PREFIX => {
            if clear {
                *saved = SavedMotionDefaults::default();
                Ok(())
            } else {
                Err(format!(
                    "{KEY_PREFIX} can only be reset (null or `default`); set {STYLE_KEY}, {TIMING_KEY} or {KEY_PREFIX}.effects.<{}> instead",
                    EFFECT_NAMES.join("|")
                ))
            }
        }
        STYLE_KEY => {
            saved.style = if clear {
                None
            } else {
                let name = value
                    .as_str()
                    .ok_or_else(|| format!("{STYLE_KEY} must be a string, got {value}"))?;
                Some(MotionStyle::parse(name).ok_or_else(|| {
                    format!(
                        "unknown cursor motion style `{name}`; expected one of {}",
                        allowed_styles()
                    )
                })?)
            };
            Ok(())
        }
        TIMING_KEY => {
            saved.timing = if clear {
                None
            } else {
                let name = value
                    .as_str()
                    .ok_or_else(|| format!("{TIMING_KEY} must be a string, got {value}"))?;
                Some(MotionTiming::parse(name).ok_or_else(|| {
                    format!(
                        "unknown cursor motion timing `{name}`; expected native, fitts or fixed"
                    )
                })?)
            };
            Ok(())
        }
        other => {
            let Some(effect) = other.strip_prefix("cursor.motion.effects.") else {
                return Err(format!(
                    "unknown cursor motion key `{other}`; expected {STYLE_KEY}, {TIMING_KEY} or {KEY_PREFIX}.effects.<{}>",
                    EFFECT_NAMES.join("|")
                ));
            };
            let flag = effect_flag(other, value)?;
            match effect {
                "trail" => saved.effects.trail = flag,
                "glow" => saved.effects.glow = flag,
                "magnet" => saved.effects.magnet = flag,
                "ripple" => saved.effects.ripple = flag,
                "squish" => saved.effects.squish = flag,
                _ => {
                    return Err(format!(
                        "unknown cursor effect `{effect}`; expected {}",
                        EFFECT_NAMES.join(", ")
                    ))
                }
            }
            Ok(())
        }
    }
}

/// Whether `key` belongs to the cursor motion defaults.
pub fn is_motion_key(key: &str) -> bool {
    key == KEY_PREFIX || key.starts_with("cursor.motion.")
}

/// Input-schema properties to merge into each platform's `set_config` tool.
pub fn config_schema_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        STYLE_KEY.to_owned(),
        json!({
            "type": "string",
            "enum": MotionStyle::ALL.map(MotionStyle::as_str),
            "description": "Saved default cursor motion style for new sessions. Overridden by start_session cursor_motion and set_agent_cursor_motion; reduced motion always wins. Built-in default: signature_arc."
        }),
    );
    properties.insert(
        TIMING_KEY.to_owned(),
        json!({
            "type": "string",
            "enum": ["native", "fitts", "fixed"],
            "description": "Saved default cursor move timing for new sessions."
        }),
    );
    for name in EFFECT_NAMES {
        properties.insert(
            format!("cursor.motion.effects.{name}"),
            json!({
                "type": "boolean",
                "description": format!("Saved default for the `{name}` cursor effect. Unset follows the style.")
            }),
        );
    }
    properties.insert(
        KEY_PREFIX.to_owned(),
        json!({
            "type": "string",
            "enum": ["default"],
            "description": "Pass `default` (or omit / null at runtime) to clear every saved cursor motion default."
        }),
    );
    properties
}

/// Apply every cursor motion write in a `set_config` call, accepting both the
/// `{key, value}` shape and direct dotted fields. Returns the keys written, or
/// the first validation error (later keys are not written after an error).
pub fn apply_config_args(args: &Value) -> Result<Vec<String>, String> {
    let mut writes: Vec<(String, Value)> = Vec::new();
    if let (Some(key), Some(value)) = (args.get("key").and_then(Value::as_str), args.get("value")) {
        if is_motion_key(key) {
            writes.push((key.to_owned(), value.clone()));
        }
    }
    if let Some(object) = args.as_object() {
        for (key, value) in object {
            if is_motion_key(key) {
                writes.push((key.clone(), value.clone()));
            }
        }
    }
    let mut applied = Vec::new();
    for (key, value) in writes {
        set_key(&key, &value)?;
        applied.push(key);
    }
    Ok(applied)
}

/// Read the saved defaults from `path`. Missing files, malformed JSON and
/// invalid values fall back to the built-ins; a bad value is logged, never
/// fatal, so a hand-edited file cannot stop the daemon.
pub fn load_from(path: &Path) -> SavedMotionDefaults {
    let mut saved = SavedMotionDefaults::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return saved;
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return saved;
    };
    let Some(motion) = json.get("cursor").and_then(|cursor| cursor.get("motion")) else {
        return saved;
    };
    let mut apply = |key: String, value: Option<&Value>| {
        if let Some(value) = value {
            if let Err(error) = apply_key(&mut saved, &key, value) {
                tracing::warn!("ignoring saved {key}: {error}");
            }
        }
    };
    apply(STYLE_KEY.to_owned(), motion.get("style"));
    apply(TIMING_KEY.to_owned(), motion.get("timing"));
    if let Some(effects) = motion.get("effects").and_then(Value::as_object) {
        for name in EFFECT_NAMES {
            apply(format!("cursor.motion.effects.{name}"), effects.get(name));
        }
    }
    saved
}

fn to_json(saved: &SavedMotionDefaults) -> Value {
    let mut motion = Map::new();
    if let Some(style) = saved.style {
        motion.insert("style".into(), json!(style.as_str()));
    }
    if let Some(timing) = saved.timing {
        motion.insert("timing".into(), json!(timing.as_str()));
    }
    let effects = saved.config_json()["effects"].clone();
    if effects.as_object().is_some_and(|map| !map.is_empty()) {
        motion.insert("effects".into(), effects);
    }
    Value::Object(motion)
}

/// Validate and persist one write to `path`, keeping every other key in the
/// file. Returns the new saved defaults. Nothing is written when validation
/// fails.
pub fn set_key_at(path: &Path, key: &str, value: &Value) -> Result<SavedMotionDefaults, String> {
    let mut saved = load_from(path);
    apply_key(&mut saved, key, value)?;
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let motion = to_json(&saved);
    let cursor = root
        .as_object_mut()
        .expect("root is an object")
        .entry("cursor")
        .or_insert_with(|| json!({}));
    if !cursor.is_object() {
        *cursor = json!({});
    }
    let cursor = cursor.as_object_mut().expect("cursor is an object");
    if motion.as_object().is_some_and(Map::is_empty) {
        cursor.remove("motion");
    } else {
        cursor.insert("motion".into(), motion);
    }
    if cursor.is_empty() {
        root.as_object_mut()
            .expect("root is an object")
            .remove("cursor");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    std::fs::write(path, body).map_err(|error| format!("failed to persist {key}: {error}"))?;
    Ok(saved)
}

// Process-wide copy applied to cursors created after it is set. It starts
// empty so unit tests never read the user's file; the daemon fills it in at
// startup (`CursorConfig::from_args`) and on every config write.
static ACTIVE: RwLock<SavedMotionDefaults> = RwLock::new(SavedMotionDefaults {
    style: None,
    timing: None,
    effects: MotionEffects {
        trail: None,
        glow: None,
        magnet: None,
        ripple: None,
        squish: None,
    },
});

/// Defaults new cursors currently start from.
pub fn active() -> SavedMotionDefaults {
    *ACTIVE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Replace the process-wide copy.
pub fn set_active(saved: SavedMotionDefaults) {
    *ACTIVE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = saved;
}

/// Load `~/.cua-driver/config.json` into the process-wide copy.
pub fn load_active() -> SavedMotionDefaults {
    let saved = default_config_path()
        .map(|path| load_from(&path))
        .unwrap_or_default();
    set_active(saved);
    saved
}

/// Read what is saved on disk right now (for `get_config`).
pub fn read_saved() -> SavedMotionDefaults {
    default_config_path()
        .map(|path| load_from(&path))
        .unwrap_or_default()
}

/// Validate, persist to `~/.cua-driver/config.json`, and make the value the
/// default for cursors created from now on.
pub fn set_key(key: &str, value: &Value) -> Result<SavedMotionDefaults, String> {
    let path = default_config_path().ok_or_else(|| "$HOME is not set".to_string())?;
    let saved = set_key_at(&path, key, value)?;
    set_active(saved);
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cua-motion-defaults-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.json")
    }

    #[test]
    fn built_in_default_is_signature_arc() {
        let mut motion = MotionConfig::default();
        SavedMotionDefaults::default().apply(&mut motion);
        assert_eq!(motion.style, MotionStyle::SignatureArc);
        assert_eq!(motion.timing, MotionTiming::Native);
    }

    #[test]
    fn saved_style_timing_and_effects_persist_and_reload() {
        let path = temp_path("persist");
        set_key_at(&path, STYLE_KEY, &json!("magnetic")).unwrap();
        set_key_at(&path, TIMING_KEY, &json!("fitts")).unwrap();
        set_key_at(&path, "cursor.motion.effects.trail", &json!(true)).unwrap();
        set_key_at(&path, "cursor.motion.effects.glow", &json!("false")).unwrap();

        let saved = load_from(&path);
        assert_eq!(saved.style, Some(MotionStyle::Magnetic));
        assert_eq!(saved.timing, Some(MotionTiming::Fitts));
        assert_eq!(saved.effects.trail, Some(true));
        assert_eq!(saved.effects.glow, Some(false));
        assert_eq!(saved.effects.ripple, None);

        let mut motion = MotionConfig::default();
        saved.apply(&mut motion);
        assert_eq!(motion.style, MotionStyle::Magnetic);
        assert_eq!(motion.timing, MotionTiming::Fitts);
        assert!(motion.resolved_effects().trail);
        assert!(!motion.resolved_effects().glow);
        // Untouched effects keep the style's defaults (magnetic: ripple on).
        assert!(motion.resolved_effects().ripple);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn writes_keep_unrelated_keys_and_clear_cleanly() {
        let path = temp_path("keep");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"max_image_dimension": 800, "cursor": {"other": 1}}"#,
        )
        .unwrap();
        set_key_at(&path, STYLE_KEY, &json!("classic")).unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["max_image_dimension"], 800);
        assert_eq!(root["cursor"]["other"], 1);
        assert_eq!(root["cursor"]["motion"]["style"], "classic");

        set_key_at(&path, KEY_PREFIX, &Value::Null).unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["max_image_dimension"], 800);
        assert!(root["cursor"].get("motion").is_none());
        assert_eq!(load_from(&path), SavedMotionDefaults::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn validation_lists_allowed_values_and_writes_nothing() {
        let path = temp_path("validate");
        let error = set_key_at(&path, STYLE_KEY, &json!("wobble")).unwrap_err();
        for style in MotionStyle::ALL {
            assert!(error.contains(style.as_str()), "{error}");
        }
        assert!(set_key_at(&path, TIMING_KEY, &json!("slow"))
            .unwrap_err()
            .contains("native, fitts or fixed"));
        assert!(set_key_at(&path, "cursor.motion.effects.sparkle", &json!(true)).is_err());
        assert!(set_key_at(&path, "cursor.motion.effects.trail", &json!(3)).is_err());
        assert!(set_key_at(&path, STYLE_KEY, &json!(7)).is_err());
        assert!(!path.exists(), "a rejected write must not create the file");
    }

    #[test]
    fn invalid_saved_values_fall_back_to_built_ins() {
        let path = temp_path("invalid");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"cursor":{"motion":{"style":"wobble","timing":"fixed"}}}"#,
        )
        .unwrap();
        let saved = load_from(&path);
        assert_eq!(saved.style, None);
        assert_eq!(saved.timing, Some(MotionTiming::Fixed));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn precedence_per_call_beats_session_beats_saved_beats_built_in() {
        // Saved default seeds a new cursor.
        let mut motion = MotionConfig::default();
        SavedMotionDefaults {
            style: Some(MotionStyle::Magnetic),
            ..Default::default()
        }
        .apply(&mut motion);
        assert_eq!(motion.style, MotionStyle::Magnetic);
        // start_session cursor_motion applies on top.
        let motion = motion
            .with_style_args(&json!({"style": "comet_swoop"}))
            .unwrap();
        assert_eq!(motion.style, MotionStyle::CometSwoop);
        // A later set_agent_cursor_motion wins; omitted fields keep the session value.
        let motion = motion.with_style_args(&json!({"timing": "fitts"})).unwrap();
        assert_eq!(motion.style, MotionStyle::CometSwoop);
        let motion = motion
            .with_style_args(&json!({"style": "spring_settle"}))
            .unwrap();
        assert_eq!(motion.style, MotionStyle::SpringSettle);
    }

    #[test]
    fn start_session_motion_args_apply_over_the_current_motion() {
        let base = MotionConfig::default();
        let motion = base
            .with_motion_args(
                &json!({"style": "magnetic", "effects": {"trail": true}, "glide_duration_ms": 900}),
            )
            .unwrap();
        assert_eq!(motion.style, MotionStyle::Magnetic);
        assert_eq!(motion.effects.trail, Some(true));
        assert_eq!(motion.glide_duration_ms, 900.0);
        let error = base
            .with_motion_args(&json!({"style": "wobble"}))
            .unwrap_err();
        assert!(
            error.contains("signature_arc") && error.contains("classic"),
            "{error}"
        );
    }

    #[test]
    fn schema_exposes_every_key_a_write_accepts() {
        let properties = config_schema_properties();
        assert!(properties.contains_key(STYLE_KEY));
        assert!(properties.contains_key(TIMING_KEY));
        assert!(properties.contains_key(KEY_PREFIX));
        for name in EFFECT_NAMES {
            assert!(properties.contains_key(&format!("cursor.motion.effects.{name}")));
        }
        // Vertex/Gemini reject description-only nodes (#4798).
        assert_eq!(properties[KEY_PREFIX]["type"], "string");
        assert_eq!(properties[KEY_PREFIX]["enum"], json!(["default"]));
        for schema in properties.values() {
            assert!(
                schema.get("type").and_then(Value::as_str).is_some(),
                "set_config cursor.motion.* fields need a single string type: {schema}"
            );
        }
    }
}
