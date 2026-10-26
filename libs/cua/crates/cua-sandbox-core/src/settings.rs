//! User defaults: where sandboxes run when nothing says, and what kind and
//! runtime `auto` starts from.
//!
//! One precedence everywhere:
//!
//! 1. explicit: `--on` / `--kind` / `--runtime`, `on=` / `local=` / `kind=` / `runtime=`
//! 2. environment: `CUA_DEFAULT_ON`, `CUA_DEFAULT_KIND`, `CUA_DEFAULT_RUNTIME`
//! 3. the config file, `$CUA_HOME/config.toml` (default `~/.cua/config.toml`),
//!    written by `cua config set`
//! 4. the built-in default: `local`, `auto`, `auto`
//!
//! ```toml
//! [default]
//! on = "aws"
//!
//! [runtime]
//! linux = "builtin"
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
pub const KEYS: [Key; 13] = [
    Key {
        name: "default.on",
        env: "CUA_DEFAULT_ON",
        default: "local",
        description: "where sandboxes run when --on / local= is not given (local or a provider)",
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
        name: "telemetry.enabled",
        env: "CUA_TELEMETRY",
        default: "on",
        description: "anonymous usage telemetry (on, off; alias `telemetry`); DO_NOT_TRACK=1 also turns it off. See `cua telemetry status`",
    },
    Key {
        name: "runtime.lume",
        env: "CUA_RUNTIME_LUME",
        default: "auto",
        description: "the Lume macOS Spaces run on: auto (this Mac's own, else built-in), builtin (cua's pinned, signed Lume, set up on first use) or system (only this Mac's own; nothing is downloaded)",
    },
    Key {
        name: "runtime.linux",
        env: "CUA_RUNTIME_LINUX",
        default: "auto",
        description: "the engine local Linux Spaces run on, on a Mac: auto (a working Docker engine on this Mac, else built-in; built-in once it exists), builtin (cua's own VM with Docker and gVisor, set up on first use) or system (only this Mac's own engine; nothing is downloaded)",
    },
    Key {
        name: "runtime.linux_cpus",
        env: "CUA_RUNTIME_LINUX_CPUS",
        default: "4",
        description: "CPUs of the built-in Linux runtime VM on a Mac, which every local Linux Space shares (at most this Mac's; applies when the VM next starts)",
    },
    Key {
        name: "runtime.linux_memory_gib",
        env: "CUA_RUNTIME_LINUX_MEMORY_GIB",
        default: "4",
        description: "memory (GiB) of the built-in Linux runtime VM on a Mac (4 GiB always stays with the Mac; applies when the VM next starts)",
    },
    Key {
        name: "pool.auto",
        env: "CUA_POOL_AUTO",
        default: "on",
        description: "keep Spaces ready only where this machine's own creates pay for it (bursts, batches, periodic jobs; never for occasional creates, nor on battery), so those creates take about 0.1 s (off: only explicit pools)",
    },
    Key {
        name: "placement.overflow",
        env: "CUA_PLACEMENT_OVERFLOW",
        default: "on",
        description: "when no location is given and a linked cloud has a budget (`cua cloud budget`), Spaces go to this machine first and overflow into your clouds when it is full, never past the budget (off: this machine only)",
    },
    Key {
        name: "pool.memory_percent",
        env: "CUA_POOL_MEMORY_PERCENT",
        default: "15",
        description: "most memory ready Spaces may hold, as a percentage of this machine's (of the built-in Linux runtime VM's for containers on a Mac); none starts when memory runs short",
    },
    Key {
        name: "pool.warm",
        env: "CUA_POOL_WARM",
        default: "none",
        description: "images kept ready regardless of use, as IMAGE[=COUNT] separated by commas (a dedicated host: `ghcr.io/trycua/linux:24.04-slim=8`)",
    },
    Key {
        name: "nudges.risky_actions",
        env: "CUA_NUDGE_RISKY_ACTIONS",
        default: "off",
        description: "before an agent does something risky on this Mac's own desktop through `cua mcp` (deleting files outside a project, emptying the Trash, uninstalling apps, system settings, passwords), the Cua app offers to run it in a separate Space (on, off). No answer in 30 s runs it here",
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

    /// Checks `value` for `key` and writes it to the config file.
    pub fn set(&mut self, key: Key, value: &str) -> Result<String, SettingsError> {
        let value = normalize(key, value)?;
        let (section, name) = key.name.split_once('.').expect("keys are section.name");
        if !self.doc.contains_table(section) {
            self.doc[section] = toml_edit::table();
        }
        let item = match key.name {
            "runtime.linux_cpus" | "runtime.linux_memory_gib" | "pool.memory_percent" => {
                toml_edit::value(
                    value
                        .parse::<i64>()
                        .map_err(|e| SettingsError(e.to_string()))?,
                )
            }
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
                "default.on ({src}): {on} names one computer; the default location is local, cloud or a provider"
            )));
        }
        Ok((on, src))
    }

    /// Whether an omitted location may overflow from this machine into a
    /// linked cloud that has a budget (`placement.overflow`, default on).
    pub fn overflow(&self) -> bool {
        key("placement.overflow")
            .map(|k| self.entry(k).value)
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "off" | "false" | "0" | "no"
                )
            })
            .unwrap_or(true)
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
            if on == On::Cloud {
                return Err(SettingsError(crate::CLOUD_ENDED.into()));
            }
            if on.is_existing_machine() {
                return Err(SettingsError(format!(
                    "default.on: {on} names one computer; use local, cloud or a provider"
                )));
            }
            on.to_string()
        }
        "default.kind" => Kind::parse(v).map_err(bad)?.to_string(),
        "default.runtime" => Runtime::parse(v).map_err(bad)?.to_string(),
        "runtime.linux_cpus" | "runtime.linux_memory_gib" => {
            let n: u32 = v.parse().map_err(|_| {
                SettingsError(format!("{}: expected a number, got {v:?}", key.name))
            })?;
            if n == 0 {
                return Err(SettingsError(format!("{}: must be at least 1", key.name)));
            }
            n.to_string()
        }
        "placement.overflow" => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => "on".into(),
            "0" | "false" | "no" | "off" => "off".into(),
            _ => {
                return Err(SettingsError(format!(
                    "placement.overflow: expected on or off, got {v:?}"
                )));
            }
        },
        "pool.auto" => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => "on".into(),
            "0" | "false" | "no" | "off" => "off".into(),
            _ => {
                return Err(SettingsError(format!(
                    "pool.auto: expected on or off, got {v:?}"
                )));
            }
        },
        "pool.memory_percent" => {
            let n: u32 = v.parse().map_err(|_| {
                SettingsError(format!("pool.memory_percent: expected a number, got {v:?}"))
            })?;
            if n > 90 {
                return Err(SettingsError(
                    "pool.memory_percent: at most 90 (memory must stay free for Spaces in use)"
                        .into(),
                ));
            }
            n.to_string()
        }
        "pool.warm" => {
            let mut out = Vec::new();
            for e in v.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                if e.eq_ignore_ascii_case("none") {
                    continue;
                }
                let (img, n) = match e.rsplit_once('=') {
                    Some((i, n)) => (i.trim(), n.trim()),
                    None => (e, "1"),
                };
                let n: u32 = n.parse().map_err(|_| {
                    SettingsError(format!(
                        "pool.warm: {e:?}: expected IMAGE or IMAGE=COUNT (a number)"
                    ))
                })?;
                if img.is_empty() {
                    return Err(SettingsError(format!("pool.warm: {e:?}: no image")));
                }
                out.push(if n == 1 {
                    img.to_string()
                } else {
                    format!("{img}={n}")
                });
            }
            if out.is_empty() {
                "none".into()
            } else {
                out.join(",")
            }
        }
        "runtime.lume" => cua_vmm::lume::builtin::LumeSource::parse(v)
            .ok_or_else(|| {
                SettingsError(format!(
                    "runtime.lume: expected auto, builtin or system, got {v:?}"
                ))
            })?
            .as_str()
            .to_string(),
        "runtime.linux" => cua_vmm::managed::LinuxSource::parse(v)
            .ok_or_else(|| {
                SettingsError(format!(
                    "runtime.linux: expected auto, builtin or system, got {v:?}"
                ))
            })?
            .as_str()
            .to_string(),
        "nudges.risky_actions" => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => "on".into(),
            "0" | "false" | "no" | "off" => "off".into(),
            _ => {
                return Err(SettingsError(format!(
                    "nudges.risky_actions: expected on or off, got {v:?}"
                )));
            }
        },
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
        s.set(key("default.on").unwrap(), "aws").unwrap();
        s.set(key("default.kind").unwrap(), "vm").unwrap();
        s.set(key("runtime.linux").unwrap(), "builtin").unwrap();

        // config
        let s = settings(d.path(), &[]);
        let path = d.path().join("config.toml");
        assert_eq!(
            s.default_on().unwrap(),
            (On::Provider("aws".into()), Source::Config(path.clone()))
        );
        assert_eq!(s.entry(key("runtime.linux").unwrap()).value, "builtin");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[default]") && text.contains("on = \"aws\""),
            "{text}"
        );
        assert!(text.contains("linux = \"builtin\""), "{text}");

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
        let r = s.resolve(Some(On::Local), Some(Kind::Vm), None).unwrap();
        assert_eq!((r.on, r.on_source), (On::Local, Source::Explicit));
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
        assert_eq!(s.set(key("default.on").unwrap(), " AWS ").unwrap(), "aws");
        // Cua Cloud has ended: refused with the one message.
        let e = s.set(key("default.on").unwrap(), "cloud").unwrap_err();
        assert_eq!(e.0, crate::CLOUD_ENDED);
        assert_eq!(
            s.set(key("default.runtime").unwrap(), "runsc").unwrap(),
            "gvisor"
        );
        let e = s.set(key("default.on").unwrap(), "qemu").unwrap_err();
        assert!(e.0.contains("--runtime qemu"), "{e}");
        let e = s
            .set(key("default.on").unwrap(), "direct:1.2.3.4:3211")
            .unwrap_err();
        assert!(e.0.contains("names one computer"), "{e}");
        assert!(s.set(key("default.kind").unwrap(), "docker").is_err());
        assert_eq!(
            s.set(key("runtime.linux").unwrap(), "builtin").unwrap(),
            "builtin"
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
        let s = settings(d.path(), &[("CUA_DEFAULT_ON", "kubevirt")]);
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
        let s = settings(d.path(), &[("CUA_DEFAULT_RUNTIME", "runc")]);
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
