//! User defaults (`$CUA_HOME/config.toml`, `cua config`) for every
//! language: where sandboxes and Spaces run when `on` is not given, the
//! default kind and runtime, and the cloud defaults. Each value comes with
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
    /// `default.on`, `default.kind`, `default.runtime`, `cloud.warm`,
    /// `cloud.max_pool_size` or `cloud.claim_ttl`.
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
/// takes `local`, `cloud` or a provider). Returns its effective value, which
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

/// The locations `on` accepts: `local`, `cloud`, then registered providers.
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
        let e = config_set("default.on".into(), "cloud".into()).unwrap();
        assert_eq!((e.value.as_str(), e.source.as_str()), ("cloud", "config"));
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
        assert!(config_list().unwrap().iter().any(|e| e.key == "cloud.warm"));
        assert!(config_path().ends_with("config.toml"));
    }

    #[test]
    fn locations_and_placement_checks() {
        let _env = super::super::test_env::EnvGuard::isolated();
        let l = locations();
        let local = l.iter().find(|l| l.name == "local").unwrap();
        assert_eq!(local.runtimes["container"], ["gvisor", "runc"]);
        let cloud = l.iter().find(|l| l.name == "cloud").unwrap();
        assert_eq!(cloud.runtimes["vm"], ["kubevirt"]);
        check_placement("cloud".into(), "vm".into(), "".into()).unwrap();
        let e = check_placement("cloud".into(), "vm".into(), "gvisor".into()).unwrap_err();
        assert!(
            e.to_string().contains("valid runtime: auto, kubevirt"),
            "{e}"
        );
        check_placement("".into(), "".into(), "".into()).unwrap();
    }
}
