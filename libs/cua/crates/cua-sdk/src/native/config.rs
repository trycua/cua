//! User defaults (`$CUA_HOME/config.toml`, `cua config`) for every
//! language: where sandboxes and Spaces run when `on` is not given, the
//! default kind and runtime, and the runtime sources. Each value comes with
//! where it came from (the environment, the config file, or the built-in
//! default); an explicit argument always wins over all three.
//!
//! The same settings back `cua config`, `cua doctor`, the Spaces app's
//! Settings and every SDK's `create`, so a default set in one place applies
//! everywhere.

use crate::{CuaError, Result};
use cua_sandbox_core::placement;
use cua_sandbox_core::settings::{self, Entry, Settings};

/// One setting's effective value.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ConfigEntry {
    /// `default.on`, `default.kind`, `default.runtime`, `runtime.container`,
    /// `runtime.vm`, `runtime.microvm` (and their `_path`) or
    /// `telemetry.enabled`.
    pub key: String,
    /// The effective value.
    pub value: String,
    /// `env`, `config` or `default`.
    pub source: String,
    /// Where exactly: `env CUA_DEFAULT_ON`, `config /home/me/.cua/config.toml`
    /// or `default`.
    pub from: String,
    /// The environment variable that overrides it.
    pub env: String,
    /// The built-in default.
    pub default_value: String,
    /// One line about it.
    pub description: String,
}

impl From<Entry> for ConfigEntry {
    fn from(e: Entry) -> Self {
        Self {
            key: e.key.name.into(),
            value: e.value,
            source: e.source.kind().into(),
            from: e.source.to_string(),
            env: e.key.env.into(),
            default_value: e.key.default.into(),
            description: e.key.description.into(),
        }
    }
}

fn err(e: settings::SettingsError) -> CuaError {
    CuaError::InvalidArgument(e.0)
}

fn load() -> Result<Settings> {
    Settings::load().map_err(err)
}

/// Every setting with its effective value and source.
#[uniffi::export]
pub fn config_list() -> Result<Vec<ConfigEntry>> {
    Ok(load()?.list().into_iter().map(Into::into).collect())
}

/// One setting's effective value and source.
#[uniffi::export]
pub fn config_get(key: String) -> Result<ConfigEntry> {
    let k = settings::key(&key).map_err(err)?;
    Ok(load()?.entry(k).into())
}

/// Writes a setting to the config file (checked and normalised: `default.on`
/// takes `local` or a provider). Returns its effective value, which
/// an environment variable may still override.
#[uniffi::export]
pub fn config_set(key: String, value: String) -> Result<ConfigEntry> {
    let k = settings::key(&key).map_err(err)?;
    let mut s = load()?;
    s.set(k, &value).map_err(err)?;
    Ok(s.entry(k).into())
}

/// Removes a setting from the config file. Returns its effective value
/// afterwards (the environment variable or the built-in default).
#[uniffi::export]
pub fn config_unset(key: String) -> Result<ConfigEntry> {
    let k = settings::key(&key).map_err(err)?;
    let mut s = load()?;
    s.unset(k).map_err(err)?;
    Ok(s.entry(k).into())
}

/// The config file's path (`$CUA_HOME/config.toml`).
#[uniffi::export]
pub fn config_path() -> String {
    settings::config_path().display().to_string()
}

/// The locations `on` accepts: `local`, then registered providers.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocationInfo {
    /// The location's name.
    pub name: String,
    /// One line about it.
    pub description: String,
    /// The kinds it runs (`container`, `vm`).
    pub kinds: Vec<String>,
    /// Its runtimes, in `auto` preference order, per kind
    /// (`container: gvisor, runc`).
    pub runtimes: std::collections::HashMap<String, Vec<String>>,
}

/// Every location with the kinds and runtimes it offers (for pickers and
/// help text).
#[uniffi::export]
pub fn locations() -> Vec<LocationInfo> {
    placement::locations()
        .into_iter()
        .map(|c| LocationInfo {
            kinds: c.kinds().iter().map(|k| k.to_string()).collect(),
            runtimes: c
                .kinds
                .iter()
                .map(|k| {
                    (
                        k.kind.to_string(),
                        k.runtimes.iter().map(|r| r.to_string()).collect(),
                    )
                })
                .collect(),
            name: c.name,
            description: c.description,
        })
        .collect()
}

/// Checks a location, kind and runtime against each other without creating
/// anything: `InvalidPlacement` (listing the valid values) when the
/// combination does not exist. Empty strings are `auto`; an empty `on` is
/// the user default.
#[uniffi::export]
pub fn check_placement(on: String, kind: String, runtime: String) -> Result<()> {
    let place = |e: placement::PlacementError| CuaError::InvalidPlacement(e.message);
    let on = match on.trim() {
        "" => load()?.default_on().map_err(err)?.0,
        o => placement::On::parse(o).map_err(place)?,
    };
    let kind = placement::Kind::parse(&kind).map_err(place)?;
    let runtime = placement::Runtime::parse(&runtime).map_err(place)?;
    placement::validate(&on, kind, &runtime).map_err(place)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_unset_and_sources() {
        let env = super::super::test_env::EnvGuard::isolated();
        assert_eq!(config_get("default.on".into()).unwrap().value, "local");
        let e = config_set("default.on".into(), "aws".into()).unwrap();
        assert_eq!((e.value.as_str(), e.source.as_str()), ("aws", "config"));
        // Cua Cloud has ended.
        let e = config_set("default.on".into(), "cloud".into()).unwrap_err();
        assert!(e.to_string().contains("Cua Cloud has ended"), "{e}");
        env.set("CUA_DEFAULT_ON", "local");
        let e = config_get("default.on".into()).unwrap();
        assert_eq!((e.value.as_str(), e.source.as_str()), ("local", "env"));
        assert_eq!(e.env, "CUA_DEFAULT_ON");
        let e = config_unset("default.on".into()).unwrap();
        assert_eq!(e.source, "env");
        assert!(matches!(
            config_set("default.on".into(), "qemu".into()),
            Err(CuaError::InvalidArgument(_))
        ));
        assert!(
            config_list()
                .unwrap()
                .iter()
                .any(|e| e.key == "runtime.container")
        );
        assert!(config_path().ends_with("config.toml"));
    }

    #[test]
    fn locations_and_placement_checks() {
        let _env = super::super::test_env::EnvGuard::isolated();
        let l = locations();
        let local = l.iter().find(|l| l.name == "local").unwrap();
        assert_eq!(local.runtimes["container"], ["gvisor", "runc"]);
        // Cua Cloud has ended: no longer listed.
        assert!(!l.iter().any(|l| l.name == "cloud"));
        check_placement("".into(), "".into(), "".into()).unwrap();
    }
}

/// What the repo cache holds and what it saved, in plain words for
/// Settings (`cache.repos` and `cache.repos_size_gb` are its settings,
/// changed with [`config_set`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RepoCacheStatus {
    /// The cache is on (`cache.repos`).
    pub enabled: bool,
    /// Repositories held.
    pub repos: u32,
    /// What they take on disk.
    pub bytes: u64,
    /// The size cap in force.
    pub cap_bytes: u64,
    /// The size setting: `auto`, or the number of GB.
    pub size_setting: String,
    /// `Repos: 3, 1.2 GB of 5 GB`.
    pub summary: String,
    /// `Last week: 80% of clones came from the cache, about 2 minutes saved`.
    pub week: String,
    /// Clones served from the cache in the last week.
    pub hits: u32,
    /// Clones that downloaded in the last week.
    pub misses: u32,
    /// Seconds the hits saved.
    pub seconds_saved: u64,
}

fn repo_cache_status_of(s: &Settings) -> RepoCacheStatus {
    let cache = cua_sandbox_core::repo_cache::RepoCache::at_default({
        let (enabled, cap_bytes) = s.repo_cache();
        cua_sandbox_core::repo_cache::Config { enabled, cap_bytes }
    });
    let st = cache.status();
    RepoCacheStatus {
        enabled: st.enabled,
        repos: st.repos,
        bytes: st.bytes,
        cap_bytes: st.cap_bytes,
        size_setting: s
            .configured(settings::key("cache.repos_size_gb").expect("known key"))
            .unwrap_or_else(|| "auto".into()),
        summary: st.size_line(),
        week: st.week_line(),
        hits: st.hits,
        misses: st.misses,
        seconds_saved: st.seconds_saved,
    }
}

/// The repo cache now: on or off, how much it holds of its cap, and what
/// the last week saved.
#[uniffi::export]
pub fn repo_cache_status() -> Result<RepoCacheStatus> {
    Ok(repo_cache_status_of(&load()?))
}

/// Deletes every repository the repo cache holds (they are copied again the
/// next time a Space clones them). Returns the cache after.
#[uniffi::export]
pub fn repo_cache_clear() -> Result<RepoCacheStatus> {
    let s = load()?;
    let (enabled, cap_bytes) = s.repo_cache();
    cua_sandbox_core::repo_cache::RepoCache::at_default(cua_sandbox_core::repo_cache::Config {
        enabled,
        cap_bytes,
    })
    .clear();
    Ok(repo_cache_status_of(&s))
}
