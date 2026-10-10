//! Installed-product identity.
//!
//! Release and source builds deliberately use different executable/app names.
//! Deriving the channel from the canonical executable path keeps every runtime
//! entry point (MCP proxy, daemon, status, autostart) in the same namespace
//! without a mutable "active version" switch.

use std::path::Path;
#[cfg(any(target_os = "macos", test))]
use std::path::PathBuf;

pub const RELEASE_CLI_NAME: &str = "cua-driver";
pub const LOCAL_CLI_NAME: &str = "cua-driver-local";

pub const RELEASE_APP_NAME: &str = "CuaDriver";
pub const LOCAL_APP_NAME: &str = "CuaDriverLocal";
pub const RELEASE_BUNDLE_ID: &str = "com.trycua.driver";
pub const LOCAL_BUNDLE_ID: &str = "com.trycua.driver.local";

pub(crate) fn path_is_local(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    file_name == LOCAL_CLI_NAME
        || file_name == format!("{LOCAL_CLI_NAME}.exe")
        || path
            .components()
            .any(|component| component.as_os_str().to_str() == Some("CuaDriverLocal.app"))
}

/// Whether this process is the explicitly-installed source-build product.
pub fn is_local_installation() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .is_some_and(|path| path_is_local(&path))
}

pub fn cli_name() -> &'static str {
    if is_local_installation() {
        LOCAL_CLI_NAME
    } else {
        RELEASE_CLI_NAME
    }
}

pub fn state_namespace() -> &'static str {
    if is_local_installation() {
        "cua-driver-local"
    } else {
        "cua-driver"
    }
}

pub fn user_home_subdirectory() -> &'static str {
    if is_local_installation() {
        ".cua-driver-local"
    } else {
        ".cua-driver"
    }
}

#[cfg(target_os = "windows")]
pub fn uia_executable_name() -> &'static str {
    if is_local_installation() {
        "cua-driver-uia-local.exe"
    } else {
        "cua-driver-uia.exe"
    }
}

#[cfg(target_os = "windows")]
pub fn autostart_task_name() -> &'static str {
    if is_local_installation() {
        "cua-driver-local-serve"
    } else {
        "cua-driver-serve"
    }
}

pub fn app_name() -> &'static str {
    if is_local_installation() {
        LOCAL_APP_NAME
    } else {
        RELEASE_APP_NAME
    }
}

/// Path of the installed app bundle for this product.
///
/// Resolved at runtime because the bundle is not always in `/Applications`:
/// the installer falls back to `~/Applications` for users who cannot write to
/// `/Applications`. See [`resolve_app_bundle_path`] for the lookup order.
#[cfg(target_os = "macos")]
pub fn app_bundle_path() -> String {
    let current_exe = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    resolve_app_bundle_path(
        current_exe.as_deref(),
        app_name(),
        home.as_deref(),
        |path| path.is_dir(),
        || platform_macos::apps::registered_application_path(bundle_id()),
    )
    .to_string_lossy()
    .into_owned()
}

/// The `<app_name>.app` bundle that contains `executable`, if any.
///
/// `executable` must already be canonical, so a CLI symlink such as
/// `~/.local/bin/cua-driver` has been resolved into the bundle it points at.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn bundle_containing_executable(executable: &Path, app_name: &str) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    let contents_dir = macos_dir.parent()?;
    let bundle = contents_dir.parent()?;
    let expected = format!("{app_name}.app");
    (macos_dir.file_name()? == "MacOS"
        && contents_dir.file_name()? == "Contents"
        && bundle.file_name()? == expected.as_str())
    .then(|| bundle.to_path_buf())
}

/// Resolve the app bundle path, in order:
///
/// 1. the bundle that contains the running executable, so every relaunch
///    (daemon, permission host) targets the copy that is actually running;
/// 2. `/Applications/<app>.app`, then `~/Applications/<app>.app`;
/// 3. the bundle LaunchServices has registered for the bundle identifier;
/// 4. `/Applications/<app>.app` as the documented default, so error messages
///    still name a concrete location when nothing is installed.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn resolve_app_bundle_path(
    current_exe: Option<&Path>,
    app_name: &str,
    home: Option<&Path>,
    is_bundle: impl Fn(&Path) -> bool,
    registered: impl FnOnce() -> Option<PathBuf>,
) -> PathBuf {
    if let Some(bundle) = current_exe.and_then(|exe| bundle_containing_executable(exe, app_name)) {
        return bundle;
    }
    let bundle_name = format!("{app_name}.app");
    let system = Path::new("/Applications").join(&bundle_name);
    let user = home.map(|home| home.join("Applications").join(&bundle_name));
    if let Some(found) = std::iter::once(system.clone())
        .chain(user)
        .find(|candidate| is_bundle(candidate))
    {
        return found;
    }
    registered()
        .filter(|path| path.file_name() == Some(std::ffi::OsStr::new(&bundle_name)))
        .filter(|path| is_bundle(path))
        .unwrap_or(system)
}

pub fn bundle_id() -> &'static str {
    if is_local_installation() {
        LOCAL_BUNDLE_ID
    } else {
        RELEASE_BUNDLE_ID
    }
}

/// A bundled daemon already has its stable TCC responsibility identity and
/// must not disclaim it during startup.
#[cfg(target_os = "macos")]
pub fn is_executable_inside_cuadriver_app() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .is_some_and(|path| {
            path.to_str().is_some_and(|path| {
                path.contains("/CuaDriver.app/Contents/MacOS/")
                    || path.contains("/CuaDriverLocal.app/Contents/MacOS/")
            })
        })
}

/// Returns `true` when the env var is one of `1|true|yes|on`
/// (case-insensitive). Anything else, including unset, is falsy.
#[cfg(target_os = "windows")]
pub fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_identity_requires_the_explicit_local_product_name() {
        assert!(path_is_local(Path::new(
            "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"
        )));
        assert!(path_is_local(Path::new(
            "/dev/.cua-driver-local/cua-driver-local.exe"
        )));
        assert!(!path_is_local(Path::new(
            "/Applications/CuaDriver.app/Contents/MacOS/cua-driver"
        )));
        assert!(!path_is_local(Path::new("/tmp/cua-driver-local-test")));
    }

    fn no_registration() -> Option<PathBuf> {
        None
    }

    #[test]
    fn running_bundle_wins_over_every_fixed_location() {
        let exe = Path::new("/Users/alice/Applications/CuaDriver.app/Contents/MacOS/cua-driver");
        let resolved = resolve_app_bundle_path(
            Some(exe),
            RELEASE_APP_NAME,
            Some(Path::new("/Users/alice")),
            |_| true,
            || panic!("LaunchServices must not be consulted when the running bundle is known"),
        );
        assert_eq!(
            resolved,
            PathBuf::from("/Users/alice/Applications/CuaDriver.app")
        );
    }

    #[test]
    fn executable_outside_the_product_bundle_is_not_a_bundle() {
        for exe in [
            "/Users/alice/.cargo/bin/cua-driver",
            "/Applications/Other.app/Contents/MacOS/cua-driver",
            "/Applications/CuaDriver.app/Contents/Resources/cua-driver",
            "/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local",
        ] {
            assert_eq!(
                bundle_containing_executable(Path::new(exe), RELEASE_APP_NAME),
                None,
                "{exe}"
            );
        }
        assert_eq!(
            bundle_containing_executable(
                Path::new("/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"),
                LOCAL_APP_NAME
            ),
            Some(PathBuf::from("/Applications/CuaDriverLocal.app"))
        );
    }

    #[test]
    fn system_applications_is_preferred_over_the_user_folder() {
        let resolved = resolve_app_bundle_path(
            Some(Path::new("/usr/local/bin/cua-driver")),
            RELEASE_APP_NAME,
            Some(Path::new("/Users/alice")),
            |_| true,
            no_registration,
        );
        assert_eq!(resolved, PathBuf::from("/Applications/CuaDriver.app"));
    }

    #[test]
    fn user_applications_is_used_when_only_it_has_the_bundle() {
        let resolved = resolve_app_bundle_path(
            None,
            RELEASE_APP_NAME,
            Some(Path::new("/Users/alice")),
            |path| path == Path::new("/Users/alice/Applications/CuaDriver.app"),
            no_registration,
        );
        assert_eq!(
            resolved,
            PathBuf::from("/Users/alice/Applications/CuaDriver.app")
        );
    }

    #[test]
    fn launchservices_registration_is_used_only_for_an_existing_product_bundle() {
        let elsewhere = PathBuf::from("/Volumes/Tools/CuaDriver.app");
        let resolved = resolve_app_bundle_path(
            None,
            RELEASE_APP_NAME,
            Some(Path::new("/Users/alice")),
            |path| path == elsewhere.as_path(),
            || Some(elsewhere.clone()),
        );
        assert_eq!(resolved, elsewhere);

        let wrong_name = resolve_app_bundle_path(
            None,
            RELEASE_APP_NAME,
            None,
            |_| false,
            || Some(PathBuf::from("/Volumes/Tools/Renamed.app")),
        );
        assert_eq!(wrong_name, PathBuf::from("/Applications/CuaDriver.app"));
    }

    #[test]
    fn nothing_installed_falls_back_to_the_documented_default() {
        let resolved =
            resolve_app_bundle_path(None, LOCAL_APP_NAME, None, |_| false, no_registration);
        assert_eq!(resolved, PathBuf::from("/Applications/CuaDriverLocal.app"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn env_truthiness_is_strict() {
        let name = "CUA_DRIVER_RS_TEST_TRUTHY";
        for value in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            std::env::set_var(name, value);
            assert!(is_env_truthy(name), "expected truthy for {value:?}");
        }
        for value in ["0", "false", "no", "off", ""] {
            std::env::set_var(name, value);
            assert!(!is_env_truthy(name), "expected falsy for {value:?}");
        }
        std::env::remove_var(name);
    }
}
