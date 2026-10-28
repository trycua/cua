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
pub const KEYS: [Key; 19] = [
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
        description: "the kind when --kind is not given (auto, container, vm, microvm)",
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
        name: "runtime.container",
        env: "CUA_RUNTIME_CONTAINER",
        default: "auto",
        description: "what runs container Spaces: auto (a working Docker engine here, else on a Mac cua's own; cua's own once it exists), builtin (cua's own: on a Mac a VM with Docker and gVisor, set up on first use) or system (only the engine already installed here; nothing is downloaded). Was runtime.linux",
    },
    Key {
        name: "runtime.container_path",
        env: "CUA_RUNTIME_CONTAINER_PATH",
        default: "",
        description: "with runtime.container = system: the docker or podman binary to use instead of the one found on PATH (its engine is the one Spaces use)",
    },
    Key {
        name: "runtime.vm",
        env: "CUA_RUNTIME_VM",
        default: "auto",
        description: "what runs virtual machine Spaces: auto (on a Mac this Mac's own Lume when new enough, else cua's own), builtin (cua's own: on a Mac its pinned, signed Lume, set up on first use) or system (only the one already installed here; nothing is downloaded). Was runtime.lume",
    },
    Key {
        name: "runtime.vm_path",
        env: "CUA_RUNTIME_VM_PATH",
        default: "",
        description: "with runtime.vm = system: the lume (Mac) or qemu-system-<arch> (Linux) binary to use instead of the one found on PATH",
    },
    Key {
        name: "runtime.microvm",
        env: "CUA_RUNTIME_MICROVM",
        default: "auto",
        description: "what runs microVM Spaces: auto (cua's own when this machine runs it, else what is installed here), builtin (cua's own: on a Mac its monitor, on Linux its Firecracker) or system (only what is already installed here: on a Mac Apple's container, on Linux Firecracker)",
    },
    Key {
        name: "runtime.microvm_path",
        env: "CUA_RUNTIME_MICROVM_PATH",
        default: "",
        description: "with runtime.microvm = system: the container (Mac) or firecracker (Linux) binary to use instead of the one found on PATH",
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
        name: "cache.repos",
        env: "CUA_CACHE_REPOS",
        default: "on",
        description: "the repo cache: public git repositories this machine keeps a verified copy of, so a repeat `git clone` in a Space starts from it (on, off). Only public repositories, only on this machine's disk; `cua host cache clear` empties it",
    },
    Key {
        name: "cache.repos_size_gb",
        env: "CUA_CACHE_REPOS_SIZE_GB",
        default: "auto",
        description: "the most disk the repo cache may use, in GB (auto: the smaller of 5 GB and a tenth of the free disk); the least recently used repositories go first",
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
const ALIASES: &[(&str, &str)] = &[
    ("telemetry", "telemetry.enabled"),
    // The per-OS runtime settings became per kind (`runtime.container`,
    // `runtime.vm`): the old names still work.
    ("runtime.linux", "runtime.container"),
    ("runtime.lume", "runtime.vm"),
];

/// The settings that replaced a per-OS one: the new name, the old name
/// in the config file's `[runtime]` table and the old environment
/// variable. Both are still read (the new ones win), and a write keeps the
/// old name in step for an older `cua-spacesd` that reads only it.
pub const LEGACY_RUNTIME: &[(&str, &str, &str)] = &[
    ("runtime.container", "linux", "CUA_RUNTIME_LINUX"),
    ("runtime.vm", "lume", "CUA_RUNTIME_LUME"),
];

fn legacy_of(key: &Key) -> Option<(&'static str, &'static str)> {
    LEGACY_RUNTIME
        .iter()
        .find(|(n, _, _)| *n == key.name)
        .map(|(_, toml, env)| (*toml, *env))
}

/// The kinds of Space with a runtime setting each (`runtime.<kind>` and
/// `runtime.<kind>_path`), as `--kind` names them.
pub const RUNTIME_KINDS: [&str; 3] = ["container", "vm", "microvm"];

/// `auto`, `builtin` or `system` (`built-in` too), as a runtime setting
/// takes it.
pub fn runtime_source_word(v: &str) -> Option<&'static str> {
    match v.trim().to_ascii_lowercase().as_str() {
        "auto" | "" => Some("auto"),
        "builtin" | "built-in" | "cua" => Some("builtin"),
        "system" | "installed" => Some("system"),
        _ => None,
    }
}

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
        let mut s = Self::load_with(config_path(), |k| std::env::var(k).ok())?;
        // The per-OS runtime settings under their per-kind names (a read
        // still works without: a read-only home only skips this).
        let _ = s.migrate_legacy();
        Ok(s)
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
        let mut vars: Vec<(&'static str, String)> = KEYS
            .iter()
            .filter_map(|k| {
                env(k.env)
                    .filter(|v| !v.trim().is_empty())
                    .map(|v| (k.env, v.trim().to_string()))
            })
            .collect();
        // The old per-OS variables, when the new one is not set.
        for (name, _, old) in LEGACY_RUNTIME {
            let k = KEYS
                .iter()
                .find(|k| k.name == *name)
                .expect("legacy keys exist");
            if !vars.iter().any(|(e, _)| *e == k.env)
                && let Some(v) = env(old).filter(|v| !v.trim().is_empty())
            {
                vars.push((old, v.trim().to_string()));
            }
        }
        Ok(Self {
            path,
            doc,
            env: vars,
        })
    }

    /// The config file this reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn config_value(&self, key: &Key) -> Option<String> {
        let (section, name) = key.name.split_once('.')?;
        self.config_value_at(section, name).or_else(|| {
            let (old, _) = legacy_of(key)?;
            self.config_value_at(section, old)
        })
    }

    fn config_value_at(&self, section: &str, name: &str) -> Option<String> {
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
        let old_env = legacy_of(&key).map(|(_, e)| e);
        if let Some((e, v)) = self
            .env
            .iter()
            .find(|(e, _)| *e == key.env || Some(*e) == old_env)
        {
            return Entry {
                key,
                value: v.clone(),
                source: Source::Env(e),
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
        self.doc[section][name] = item.clone();
        // An older cua-spacesd reads only the per-OS name: keep it in step.
        if let Some((old, _)) = legacy_of(&key) {
            self.doc[section][old] = item;
        }
        self.save()?;
        Ok(value)
    }

    /// Writes each per-kind runtime setting that the config file holds
    /// only under its old per-OS name (`[runtime] linux`, `lume`) under its
    /// new one too. Returns whether anything was written. [`Settings::load`]
    /// runs it, so the file says the new names from then on.
    pub fn migrate_legacy(&mut self) -> Result<bool, SettingsError> {
        let mut changed = false;
        for (name, old, _) in LEGACY_RUNTIME {
            let (section, new) = name.split_once('.').expect("keys are section.name");
            if self.config_value_at(section, new).is_some() {
                continue;
            }
            let Some(v) = self.config_value_at(section, old) else {
                continue;
            };
            let v = runtime_source_word(&v).unwrap_or("auto");
            self.doc[section][new] = toml_edit::value(v);
            changed = true;
        }
        if changed {
            self.save()?;
        }
        Ok(changed)
    }

    /// Removes `key` from the config file. Returns whether it was set.
    pub fn unset(&mut self, key: Key) -> Result<bool, SettingsError> {
        let (section, name) = key.name.split_once('.').expect("keys are section.name");
        let mut removed = false;
        for n in std::iter::once(name).chain(legacy_of(&key).map(|(old, _)| old)) {
            removed |= self
                .doc
                .get_mut(section)
                .and_then(|t| t.as_table_like_mut())
                .and_then(|t| t.remove(n))
                .is_some();
        }
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

    /// The repo cache's settings: whether it is on (`cache.repos`, default
    /// on) and its size cap in bytes (`cache.repos_size_gb`; `None`: auto).
    pub fn repo_cache(&self) -> (bool, Option<u64>) {
        let on = key("cache.repos")
            .map(|k| self.entry(k).value)
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "off" | "false" | "0" | "no"
                )
            })
            .unwrap_or(true);
        let cap = key("cache.repos_size_gb")
            .map(|k| self.entry(k).value)
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|g| g.is_finite() && *g >= 0.0)
            .map(|g| (g * f64::from(1u32 << 30)) as u64);
        (on, cap)
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
        "cache.repos" => match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => "on".into(),
            "0" | "false" | "no" | "off" => "off".into(),
            _ => {
                return Err(SettingsError(format!(
                    "cache.repos: expected on or off, got {v:?}"
                )));
            }
        },
        "cache.repos_size_gb" => {
            if v.eq_ignore_ascii_case("auto") || v.is_empty() {
                "auto".into()
            } else {
                let g: f64 = v.parse().map_err(|_| {
                    SettingsError(format!(
                        "cache.repos_size_gb: expected auto or a number of GB, got {v:?}"
                    ))
                })?;
                if !g.is_finite() || !(0.0..=100_000.0).contains(&g) {
                    return Err(SettingsError(
                        "cache.repos_size_gb: expected auto or a number of GB between 0 and 100000"
                            .into(),
                    ));
                }
                // 0 means nothing is cached.
                if g.fract() == 0.0 {
                    format!("{}", g as u64)
                } else {
                    format!("{g}")
                }
            }
        }
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
        "runtime.container" | "runtime.vm" | "runtime.microvm" => runtime_source_word(v)
            .ok_or_else(|| {
                SettingsError(format!(
                    "{}: expected auto, builtin or system, got {v:?}",
                    key.name
                ))
            })?
            .to_string(),
        "runtime.container_path" | "runtime.vm_path" | "runtime.microvm_path" => {
            let path = match v.strip_prefix("~/") {
                Some(rest) => std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join(rest),
                None => PathBuf::from(v),
            };
            if !path.is_absolute() {
                return Err(SettingsError(format!(
                    "{}: expected the full path of a program (unset it to use the one found), got {v:?}",
                    key.name
                )));
            }
            path.display().to_string()
        }
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

    /// The repo cache is on by default with an automatic size; the words and
    /// numbers a person sets are checked.
    #[test]
    fn the_repo_cache_settings() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        assert_eq!(s.repo_cache(), (true, None));
        s.set(key("cache.repos").unwrap(), "off").unwrap();
        s.set(key("cache.repos_size_gb").unwrap(), "2.5").unwrap();
        assert_eq!(s.repo_cache(), (false, Some(2_684_354_560)));
        s.set(key("cache.repos").unwrap(), "yes").unwrap();
        s.set(key("cache.repos_size_gb").unwrap(), "auto").unwrap();
        assert_eq!(s.repo_cache(), (true, None));
        let e = s
            .set(key("cache.repos_size_gb").unwrap(), "lots")
            .unwrap_err();
        assert!(e.0.contains("cache.repos_size_gb"), "{e}");
        let e = s.set(key("cache.repos").unwrap(), "maybe").unwrap_err();
        assert!(e.0.contains("expected on or off"), "{e}");
        // The environment wins over the file.
        let s = settings(d.path(), &[("CUA_CACHE_REPOS", "off")]);
        assert_eq!(s.repo_cache().0, false);
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

    /// The per-OS runtime settings became per kind: an old config file
    /// and old variables still count, `cua config` takes the old names, a
    /// write keeps the old name in step (an older cua-spacesd reads it) and
    /// the migration writes the new names.
    #[test]
    fn per_os_runtime_settings_migrate_to_per_kind() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(&path, "[runtime]\nlinux = \"builtin\"\nlume = \"system\"\n").unwrap();
        let s = settings(d.path(), &[]);
        let container = s.entry(key("runtime.container").unwrap());
        assert_eq!(
            (container.value.as_str(), container.source.kind()),
            ("builtin", "config")
        );
        assert_eq!(s.entry(key("runtime.vm").unwrap()).value, "system");
        assert_eq!(s.entry(key("runtime.microvm").unwrap()).value, "auto");
        // The old names are aliases.
        assert_eq!(key("runtime.linux").unwrap().name, "runtime.container");
        assert_eq!(key("runtime.lume").unwrap().name, "runtime.vm");

        let mut s = settings(d.path(), &[]);
        assert!(s.migrate_legacy().unwrap());
        assert!(!s.migrate_legacy().unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("container = \"builtin\""), "{text}");
        assert!(text.contains("vm = \"system\""), "{text}");

        // A new value wins and the old name follows it.
        let mut s = settings(d.path(), &[]);
        s.set(key("runtime.container").unwrap(), "system").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("container = \"system\"") && text.contains("linux = \"system\""),
            "{text}"
        );
        assert_eq!(
            settings(d.path(), &[])
                .entry(key("runtime.container").unwrap())
                .value,
            "system"
        );
        // Unset removes both.
        let mut s = settings(d.path(), &[]);
        assert!(s.unset(key("runtime.container").unwrap()).unwrap());
        let s = settings(d.path(), &[]);
        let e = s.entry(key("runtime.container").unwrap());
        assert_eq!((e.value.as_str(), e.source.kind()), ("auto", "default"));

        // The old variable counts; the new one wins over it.
        let s = settings(d.path(), &[("CUA_RUNTIME_LINUX", "builtin")]);
        assert_eq!(
            s.entry(key("runtime.container").unwrap()).source,
            Source::Env("CUA_RUNTIME_LINUX")
        );
        let s = settings(
            d.path(),
            &[
                ("CUA_RUNTIME_LINUX", "builtin"),
                ("CUA_RUNTIME_CONTAINER", "system"),
            ],
        );
        assert_eq!(s.entry(key("runtime.container").unwrap()).value, "system");
    }

    /// A runtime path is a full path (`~/` is the home folder); unset goes
    /// back to the one found.
    #[test]
    fn runtime_paths_are_full_paths() {
        let d = tempfile::tempdir().unwrap();
        let mut s = settings(d.path(), &[]);
        let k = key("runtime.container_path").unwrap();
        assert_eq!(s.entry(k).value, "");
        assert_eq!(
            s.set(k, " /opt/homebrew/bin/podman ").unwrap(),
            "/opt/homebrew/bin/podman"
        );
        assert!(s.set(k, "podman").unwrap_err().0.contains("full path"));
        assert!(s.set(key("runtime.microvm").unwrap(), "docker").is_err());
        assert_eq!(
            s.set(key("runtime.microvm").unwrap(), "Built-in").unwrap(),
            "builtin"
        );
        assert!(s.unset(k).unwrap());
        assert_eq!(settings(d.path(), &[]).entry(k).source, Source::Default);
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
