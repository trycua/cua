//! User defaults: where sandboxes run when nothing says, what kind and
//! runtime `auto` starts from, and the cloud defaults.
//!
//! One precedence everywhere:
//!
//! 1. explicit: `--on` / `--kind` / `--runtime`, `on=` / `local=` / `kind=` / `runtime=`
//! 2. environment: `CUA_DEFAULT_ON`, `CUA_DEFAULT_KIND`, `CUA_DEFAULT_RUNTIME`,
//!    `CUA_FLEET_WARM`, `CUA_FLEET_MAX_POOL_SIZE`, `CUA_FLEET_CLAIM_TTL`
//! 3. the config file, `$CUA_HOME/config.toml` (default `~/.cua/config.toml`),
//!    written by `cua config set`
//! 4. the built-in default: `local`, `auto`, `auto`
//!
//! ```toml
//! [default]
//! on = "cloud"
//!
//! [cloud]
//! warm = true
//! ```

use crate::placement::{Kind, On, PlacementError, Runtime};
use std::fmt;
use std::path::{Path, PathBuf};

/// A setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// `section.name`, as `cua config` spells it.
    pub name: &'static str,
    /// The environment variable that overrides it.
    pub env: &'static str,
    /// The built-in default, as text.
    pub default: &'static str,
    /// One line for `cua config list`.
    pub description: &'static str,
}

/// Every setting, in display order.
pub const KEYS: [Key; 7] = [
    Key {
        name: "default.on",
        env: "CUA_DEFAULT_ON",
        default: "local",
        description: "where sandboxes run when --on / local= is not given (local, cloud, or a provider)",
    },
    Key {
        name: "default.kind",
        env: "CUA_DEFAULT_KIND",
        default: "auto",
        description: "the kind when --kind is not given (auto, container, vm)",
    },
    Key {
        name: "default.runtime",
        env: "CUA_DEFAULT_RUNTIME",
        default: "auto",
        description: "the runtime when --runtime is not given (auto, or an engine such as gvisor, runc, qemu)",
    },
    Key {
        name: "cloud.warm",
        env: "CUA_FLEET_WARM",
        default: "auto",
        description: "keep a warm cloud sandbox per image (auto: only the canonical images)",
    },
    Key {
        name: "cloud.max_pool_size",
        env: "CUA_FLEET_MAX_POOL_SIZE",
        default: "10",
        description: "most cloud sandboxes of one image at once",
    },
    Key {
        name: "cloud.claim_ttl",
        env: "CUA_FLEET_CLAIM_TTL",
        default: "15m",
        description: "how long a cloud sandbox outlives its process without a keep-alive",
    },
    Key {
        name: "telemetry.enabled",
        env: "CUA_TELEMETRY",
        default: "on",
        description: "anonymous usage telemetry (on, off; alias `telemetry`); DO_NOT_TRACK=1 also turns it off. See `cua telemetry status`",
    },
];

/// Short names `cua config` accepts for a setting.
const ALIASES: &[(&str, &str)] = &[("telemetry", "telemetry.enabled")];

/// The setting named `name`.
pub fn key(name: &str) -> Result<Key, SettingsError> {
    let n = name.trim().to_ascii_lowercase();
    let n = ALIASES
        .iter()
        .find(|(a, _)| *a == n)
        .map(|(_, k)| k.to_string())
        .unwrap_or(n);
    KEYS.iter().copied().find(|k| k.name == n).ok_or_else(|| {
        SettingsError(format!(
            "unknown setting {name:?}; settings: {}",
            KEYS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ")
        ))
    })
}

/// Where an effective value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Given explicitly (flag or argument).
    Explicit,
    /// The environment variable.
    Env(&'static str),
    /// The config file.
    Config(PathBuf),
    /// The built-in default.
    Default,
}

impl Source {
    /// `explicit`, `env`, `config` or `default`.
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Explicit => "explicit",
            Source::Env(_) => "env",
            Source::Config(_) => "config",
            Source::Default => "default",
        }
    }

    /// Whether the user chose it (anything but the built-in default).
    pub fn is_user_set(&self) -> bool {
        !matches!(self, Source::Default)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Explicit => f.write_str("explicit"),
            Source::Env(v) => write!(f, "env {v}"),
            Source::Config(p) => write!(f, "config {}", p.display()),
            Source::Default => f.write_str("default"),
        }
    }
}

/// A setting's effective value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The setting.
    pub key: Key,
    /// Its effective value, as text.
    pub value: String,
    /// Where the value came from.
    pub source: Source,
}

/// A bad setting name or value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub String);

/// The cua home: `$CUA_HOME`, else `~/.cua`.
pub fn cua_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CUA_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cua")
}

/// `$CUA_HOME/config.toml`.
pub fn config_path() -> PathBuf {
    cua_home().join("config.toml")
}

/// The settings as one process sees them: an environment and a config
/// file. [`Settings::load`] reads the real ones.
#[derive(Clone, Debug)]
pub struct Settings {
    path: PathBuf,
    doc: toml_edit::DocumentMut,
    env: Vec<(&'static str, String)>,
}

impl Settings {
    /// The process environment and `$CUA_HOME/config.toml`.
    pub fn load() -> Result<Self, SettingsError> {
        Self::load_with(config_path(), |k| std::env::var(k).ok())
    }

    /// `path` and an arbitrary environment (tests).
    pub fn load_with(
        path: impl Into<PathBuf>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, SettingsError> {
        let path = path.into();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(SettingsError(format!("{}: {e}", path.display()))),
        };
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| SettingsError(format!("{}: {e}", path.display())))?;
        let env = KEYS
            .iter()
            .filter_map(|k| {
                env(k.env)
                    .filter(|v| !v.trim().is_empty())
                    .map(|v| (k.env, v.trim().to_string()))
            })
            .collect();
        Ok(Self { path, doc, env })
    }

    /// The config file this reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn config_value(&self, key: &Key) -> Option<String> {
        let (section, name) = key.name.split_once('.')?;
        let item = self.doc.get(section)?.get(name)?;
        let v = item.as_value()?;
        Some(match v {
            toml_edit::Value::String(s) => s.value().trim().to_string(),
            toml_edit::Value::Boolean(b) => b.value().to_string(),
            toml_edit::Value::Integer(i) => i.value().to_string(),
            other => other.to_string().trim().to_string(),
        })
        .filter(|s| !s.is_empty())
    }

    /// The effective value of `key` (environment, then config, then the
    /// built-in default).
    pub fn entry(&self, key: Key) -> Entry {
        if let Some((_, v)) = self.env.iter().find(|(e, _)| *e == key.env) {
            return Entry {
                key,
                value: v.clone(),
                source: Source::Env(key.env),
            };
        }
        if let Some(v) = self.config_value(&key) {
            return Entry {
                key,
                value: v,
                source: Source::Config(self.path.clone()),
            };
        }
        Entry {
            key,
            value: key.default.to_string(),
            source: Source::Default,
        }
    }

    /// Every setting's effective value.
    pub fn list(&self) -> Vec<Entry> {
        KEYS.iter().map(|k| self.entry(*k)).collect()
    }

    /// The value the config file holds for `key` (ignoring the
    /// environment).
    pub fn configured(&self, key: Key) -> Option<String> {
        self.config_value(&key)
    }

    /// A lookup for [`cua_fleet::AutoPoolConfig::from_lookup`]: the
    /// environment variable, else the config file's value for the setting
    /// it overrides.
    pub fn lookup(&self, env_var: &str) -> Option<String> {
        let Some(key) = KEYS.iter().find(|k| k.env == env_var) else {
            return std::env::var(env_var).ok();
        };
        if let Some((_, v)) = self.env.iter().find(|(e, _)| *e == env_var) {
            return Some(v.clone());
        }
        self.config_value(key)
            .filter(|v| !(key.name == "cloud.warm" && v == "auto"))
    }

    /// Checks `value` for `key` and writes it to the config file.
    pub fn set(&mut self, key: Key, value: &str) -> Result<String, SettingsError> {
        let value = normalize(key, value)?;
        let (section, name) = key.name.split_once('.').expect("keys are section.name");
        if !self.doc.contains_table(section) {
            self.doc[section] = toml_edit::table();
        }
        let item = match key.name {
            "cloud.warm" if value != "auto" => toml_edit::value(value == "true"),
            "cloud.max_pool_size" => toml_edit::value(
                value
                    .parse::<i64>()
                    .map_err(|e| SettingsError(e.to_string()))?,
            ),
            _ => toml_edit::value(value.clone()),
        };
        self.doc[section][name] = item;
        self.save()?;
        Ok(value)
    }

    /// Removes `key` from the config file. Returns whether it was set.
    pub fn unset(&mut self, key: Key) -> Result<bool, SettingsError> {
        let (section, name) = key.name.split_once('.').expect("keys are section.name");
        let removed = self
            .doc
            .get_mut(section)
            .and_then(|t| t.as_table_like_mut())
            .and_then(|t| t.remove(name))
            .is_some();
        if removed {
            if self
                .doc
                .get(section)
                .and_then(|t| t.as_table_like())
                .is_some_and(|t| t.is_empty())
            {
                self.doc.remove(section);
            }
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<(), SettingsError> {
        let err = |e: std::io::Error| SettingsError(format!("{}: {e}", self.path.display()));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.doc.to_string()).map_err(err)?;
        std::fs::rename(&tmp, &self.path).map_err(err)
    }

    fn parsed<T>(
        &self,
        key: Key,
        parse: impl Fn(&str) -> Result<T, PlacementError>,
    ) -> Result<(T, Source), SettingsError> {
        let e = self.entry(key);
        parse(&e.value)
            .map(|v| (v, e.source.clone()))
            .map_err(|err| SettingsError(format!("{} ({}): {err}", key.name, e.source)))
    }

    /// The default location and where it came from.
    pub fn default_on(&self) -> Result<(On, Source), SettingsError> {
        let (on, src) = self.parsed(KEYS[0], On::parse)?;
        if on.is_existing_machine() {
            return Err(SettingsError(format!(
                "default.on ({src}): {on} names one machine; the default location is local, cloud or a provider"
            )));
        }
        Ok((on, src))
    }

    /// The default kind and where it came from.
    pub fn default_kind(&self) -> Result<(Kind, Source), SettingsError> {
        self.parsed(KEYS[1], Kind::parse)
    }

    /// The default runtime and where it came from.
    pub fn default_runtime(&self) -> Result<(Runtime, Source), SettingsError> {
        self.parsed(KEYS[2], Runtime::parse)
    }
}

/// Checks and normalises a value for `key`.
pub fn normalize(key: Key, value: &str) -> Result<String, SettingsError> {
    let v = value.trim();
    let bad = |e: PlacementError| SettingsError(format!("{}: {e}", key.name));
    Ok(match key.name {
        "default.on" => {
            let on = On::parse(v).map_err(bad)?;
            if on.is_existing_machine() {
                return Err(SettingsError(format!(
                    "default.on: {on} names one machine; use local, cloud or a provider"
                )));
            }
            on.to_string()
        }
        "default.kind" => Kind::parse(v).map_err(bad)?.to_string(),
        "default.runtime" => Runtime::parse(v).map_err(bad)?.to_string(),
        "cloud.warm" => match v.to_ascii_lowercase().as_str() {
            "auto" => "auto".into(),
            "1" | "true" | "yes" | "on" => "true".into(),
            "0" | "false" | "no" | "off" => "false".into(),
            _ => {
                return Err(SettingsError(format!(
                    "cloud.warm: expected true, false or auto, got {v:?}"
                )));
            }
        },
        "cloud.max_pool_size" => {
            let n: u32 = v.parse().map_err(|_| {
                SettingsError(format!("cloud.max_pool_size: expected a number, got {v:?}"))
            })?;
            if n == 0 {
                return Err(SettingsError(
                    "cloud.max_pool_size: must be at least 1".into(),
                ));
            }
            n.to_string()
        }
        "cloud.claim_ttl" => {
            let ok = v.parse::<u64>().is_ok() || humantime::parse_duration(v).is_ok();
            if !ok {
                return Err(SettingsError(format!(
                    "cloud.claim_ttl: expected seconds or a duration such as 15m, got {v:?}"
                )));
            }
            v.to_string()
        }
        "telemetry.enabled" => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => "on".into(),
            "0" | "false" | "no" | "off" => "off".into(),
            _ => {
                return Err(SettingsError(format!(
                    "telemetry: expected on or off, got {v:?}"
                )));
            }
        },
        _ => v.to_string(),
    })
}

/// The effective location, kind and runtime of a request, each with its
/// source: explicit values win; unset ones come from [`Settings`]. A
/// default kind or runtime that contradicts the explicit request (a
/// default `runtime = "gvisor"` with `kind = vm`, or on a location that
/// does not offer it) is skipped rather than failing the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// Where.
    pub on: On,
    /// Where `on` came from.
    pub on_source: Source,
    /// What kind.
    pub kind: Kind,
    /// Where `kind` came from.
    pub kind_source: Source,
    /// Which engine.
    pub runtime: Runtime,
    /// Where `runtime` came from.
    pub runtime_source: Source,
}

impl Settings {
    /// Fills in what the request leaves unset (see [`Resolved`]).
    pub fn resolve(
        &self,
        on: Option<On>,
        kind: Option<Kind>,
        runtime: Option<Runtime>,
    ) -> Result<Resolved, SettingsError> {
        let (on, on_source) = match on {
            Some(o) => (o, Source::Explicit),
            None => self.default_on()?,
        };
        let explicit_kind = kind.filter(|k| *k != Kind::Auto);
        let explicit_runtime = runtime.clone().filter(|r| *r != Runtime::Auto);
        let (mut kind, mut kind_source) = match kind {
            Some(k) => (k, Source::Explicit),
            None => self.default_kind()?,
        };
        let (mut runtime, mut runtime_source) = match runtime {
            Some(r) => (r, Source::Explicit),
            None => self.default_runtime()?,
        };
        if on.is_existing_machine() {
            if explicit_kind.is_none() {
                (kind, kind_source) = (Kind::Auto, Source::Default);
            }
            if explicit_runtime.is_none() {
                (runtime, runtime_source) = (Runtime::Auto, Source::Default);
            }
        }
        // A defaulted kind yields to an explicit runtime's kind.
        if kind_source != Source::Explicit
            && let Some(rk) = explicit_runtime.as_ref().and_then(Runtime::builtin_kind)
            && kind != rk
        {
            (kind, kind_source) = (Kind::Auto, Source::Default);
        }
        // A defaulted runtime that does not fit is skipped.
        if runtime_source != Source::Explicit
            && runtime != Runtime::Auto
            && crate::placement::validate(&on, kind, &runtime).is_err()
        {
            (runtime, runtime_source) = (Runtime::Auto, Source::Default);
        }
        if kind_source != Source::Explicit
            && kind != Kind::Auto
            && crate::placement::validate(&on, kind, &Runtime::Auto).is_err()
        {
            (kind, kind_source) = (Kind::Auto, Source::Default);
        }
        Ok(Resolved {
            on,
            on_source,
            kind,
            kind_source,
            runtime,
            runtime_source,
        })
    }
}

/// The cloud pool settings with the user's config applied:
/// [`cua_fleet::AutoPoolConfig::from_lookup`] over the environment, then
/// `config.toml` (`cloud.warm`, `cloud.max_pool_size`, `cloud.claim_ttl`).
/// A config file that does not parse is ignored here (`cua doctor` reports
/// it).
pub fn auto_pool_config() -> cua_fleet::AutoPoolConfig {
    match Settings::load() {
        Ok(s) => cua_fleet::AutoPoolConfig::from_lookup(|k| s.lookup(k)),
        Err(_) => cua_fleet::AutoPoolConfig::from_env(),
    }
}

/// The hint appended to "no cloud credentials" when the cloud came from
/// a default rather than the call.
pub fn cloud_default_hint(source: &Source) -> Option<String> {
    match source {
        Source::Env(v) => Some(format!(
            "the default location is cloud ({v}); run `cua auth login`, or unset {v} to run locally"
        )),
        Source::Config(p) => Some(format!(
            "the default location is cloud ({}); run `cua auth login`, or switch back with `cua config set default.on local`",
            p.display()
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(dir: &Path, env: &[(&str, &str)]) -> Settings {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Settings::load_with(dir.join("config.toml"), move |k| env.get(k).cloned()).unwrap()
    }

    #[test]
    fn built_in_defaults() {
        let d = tempfile::tempdir().unwrap();
        let s = settings(d.path(), &[]);
        assert_eq!(s.default_on().unwrap(), (On::Local, Source::Default));
        assert_eq!(s.default_kind().unwrap(), (Kind::Auto, Source::Default));
        assert_eq!(
            s.default_runtime().unwrap(),
            (Runtime::Auto, Source::Default)
        );
        let list = s.list();
        assert_eq!(list.len(), KEYS.len());
        assert!(list.iter().all(|e| e.source == Source::Default));
    }

    /// explicit > env > config > default, for each axis.
    #[test]
    fn precedence() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        s.set(key("default.on").unwrap(), "cloud").unwrap();
        s.set(key("default.kind").unwrap(), "vm").unwrap();
        s.set(key("cloud.warm").unwrap(), "yes").unwrap();

        // config
        let s = settings(d.path(), &[]);
        let path = d.path().join("config.toml");
        assert_eq!(
            s.default_on().unwrap(),
            (On::Cloud, Source::Config(path.clone()))
        );
        assert_eq!(s.entry(key("cloud.warm").unwrap()).value, "true");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[default]") && text.contains("on = \"cloud\""),
            "{text}"
        );
        assert!(text.contains("warm = true"), "{text}");

        // env beats config
        let s = settings(
            d.path(),
            &[
                ("CUA_DEFAULT_ON", "local"),
                ("CUA_DEFAULT_KIND", "container"),
            ],
        );
        assert_eq!(
            s.default_on().unwrap(),
            (On::Local, Source::Env("CUA_DEFAULT_ON"))
        );
        assert_eq!(
            s.default_kind().unwrap(),
            (Kind::Container, Source::Env("CUA_DEFAULT_KIND"))
        );

        // explicit beats env
        let r = s.resolve(Some(On::Cloud), Some(Kind::Vm), None).unwrap();
        assert_eq!((r.on, r.on_source), (On::Cloud, Source::Explicit));
        assert_eq!((r.kind, r.kind_source), (Kind::Vm, Source::Explicit));
        assert_eq!(
            (r.runtime, r.runtime_source),
            (Runtime::Auto, Source::Default)
        );

        // unset falls back to the default
        let mut s = settings(d.path(), &[]);
        assert!(s.unset(key("default.on").unwrap()).unwrap());
        assert!(!s.unset(key("default.on").unwrap()).unwrap());
        let s = settings(d.path(), &[]);
        assert_eq!(s.default_on().unwrap(), (On::Local, Source::Default));
        assert_eq!(s.default_kind().unwrap().0, Kind::Vm);
    }

    #[test]
    fn set_validates_and_normalizes() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        assert_eq!(
            s.set(key("default.on").unwrap(), " Cloud ").unwrap(),
            "cloud"
        );
        assert_eq!(
            s.set(key("default.runtime").unwrap(), "runsc").unwrap(),
            "gvisor"
        );
        let e = s.set(key("default.on").unwrap(), "qemu").unwrap_err();
        assert!(e.0.contains("--runtime qemu"), "{e}");
        let e = s
            .set(key("default.on").unwrap(), "direct:1.2.3.4:3211")
            .unwrap_err();
        assert!(e.0.contains("names one machine"), "{e}");
        assert!(s.set(key("default.kind").unwrap(), "docker").is_err());
        assert!(s.set(key("cloud.warm").unwrap(), "maybe").is_err());
        assert!(s.set(key("cloud.max_pool_size").unwrap(), "0").is_err());
        assert!(s.set(key("cloud.claim_ttl").unwrap(), "soon").is_err());
        assert_eq!(
            s.set(key("cloud.claim_ttl").unwrap(), "30m").unwrap(),
            "30m"
        );
        assert!(key("default.where").unwrap_err().0.contains("default.on"));
    }

    #[test]
    fn a_bad_value_names_its_source() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("config.toml"), "[default]\non = \"lume\"\n").unwrap();
        let s = settings(d.path(), &[]);
        let e = s.default_on().unwrap_err();
        assert!(
            e.0.contains("config") && e.0.contains("--runtime lume"),
            "{e}"
        );
        let s = settings(d.path(), &[("CUA_DEFAULT_ON", "fleet")]);
        let e = s.default_on().unwrap_err();
        assert!(e.0.contains("env CUA_DEFAULT_ON"), "{e}");
    }

    #[test]
    fn defaults_that_do_not_fit_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let s = settings(
            d.path(),
            &[
                ("CUA_DEFAULT_RUNTIME", "gvisor"),
                ("CUA_DEFAULT_KIND", "container"),
            ],
        );
        // An explicit VM runtime overrides the default kind.
        let r = s.resolve(None, None, Some(Runtime::Qemu)).unwrap();
        assert_eq!((r.kind, r.runtime), (Kind::Auto, Runtime::Qemu));
        // An explicit kind skips the default runtime of the other kind.
        let r = s.resolve(None, Some(Kind::Vm), None).unwrap();
        assert_eq!((r.kind, r.runtime.clone()), (Kind::Vm, Runtime::Auto));
        assert_eq!(r.runtime_source, Source::Default);
        // A runtime the location lacks (runc in the cloud) is skipped.
        let s = settings(d.path(), &[("CUA_DEFAULT_RUNTIME", "runc")]);
        let r = s.resolve(Some(On::Cloud), None, None).unwrap();
        assert_eq!(r.runtime, Runtime::Auto);
        // An existing machine takes no kind or runtime.
        let r = s
            .resolve(Some(On::Direct("h:1".into())), None, None)
            .unwrap();
        assert_eq!((r.kind, r.runtime), (Kind::Auto, Runtime::Auto));
        // A default that fits is used.
        let r = s.resolve(Some(On::Local), None, None).unwrap();
        assert_eq!(
            (r.runtime, r.runtime_source),
            (Runtime::Runc, Source::Env("CUA_DEFAULT_RUNTIME"))
        );
    }

    #[test]
    fn fleet_lookup_reads_the_config() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        s.set(key("cloud.max_pool_size").unwrap(), "3").unwrap();
        s.set(key("cloud.warm").unwrap(), "auto").unwrap();
        let s = settings(d.path(), &[]);
        assert_eq!(s.lookup("CUA_FLEET_MAX_POOL_SIZE").as_deref(), Some("3"));
        assert_eq!(s.lookup("CUA_FLEET_WARM"), None, "auto is not an override");
        let s = settings(d.path(), &[("CUA_FLEET_MAX_POOL_SIZE", "7")]);
        assert_eq!(s.lookup("CUA_FLEET_MAX_POOL_SIZE").as_deref(), Some("7"));
    }

    #[test]
    fn cloud_settings_reach_the_pool_manager() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        s.set(key("cloud.warm").unwrap(), "true").unwrap();
        s.set(key("cloud.max_pool_size").unwrap(), "4").unwrap();
        s.set(key("cloud.claim_ttl").unwrap(), "20m").unwrap();
        let s = settings(d.path(), &[]);
        let c = cua_fleet::AutoPoolConfig::from_lookup(|k| s.lookup(k));
        assert!(c.warm);
        assert_eq!(c.max_pool_size, 4);
        assert_eq!(c.claim_ttl, std::time::Duration::from_secs(1200));
    }

    #[test]
    fn hints_name_the_way_back() {
        let h = cloud_default_hint(&Source::Config(PathBuf::from("/h/.cua/config.toml"))).unwrap();
        assert!(h.contains("cua config set default.on local") && h.contains("cua auth login"));
        let h = cloud_default_hint(&Source::Env("CUA_DEFAULT_ON")).unwrap();
        assert!(h.contains("unset CUA_DEFAULT_ON"));
        assert!(cloud_default_hint(&Source::Explicit).is_none());
    }

    /// `cua config set telemetry off` writes the `[telemetry] enabled` key
    /// cua-telemetry reads; `CUA_TELEMETRY` overrides it.
    #[test]
    fn telemetry_switch_alias_and_values() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        let k = key("telemetry").unwrap();
        assert_eq!(k.name, "telemetry.enabled");
        assert_eq!(s.entry(k).value, "on");
        assert_eq!(s.set(k, "OFF").unwrap(), "off");
        assert_eq!(s.set(k, "false").unwrap(), "off");
        assert!(s.set(k, "maybe").is_err());
        let text = std::fs::read_to_string(d.path().join("config.toml")).unwrap();
        assert!(
            text.contains("[telemetry]") && text.contains("enabled = \"off\""),
            "{text}"
        );
        let s = settings(d.path(), &[("CUA_TELEMETRY", "1")]);
        assert_eq!(s.entry(k).source, Source::Env("CUA_TELEMETRY"));
    }
}
