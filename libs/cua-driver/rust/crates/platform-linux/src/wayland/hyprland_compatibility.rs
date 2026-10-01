//! Compatibility admission for the initial native Hyprland input package.
//! This is not a security boundary against same-user code. Unknown package
//! versions refuse until the native operation matrix is qualified for them.

use std::io::Read;
use std::path::{Path, PathBuf};

struct QualifiedPackage {
    name: &'static str,
    version: &'static str,
    executable: &'static str,
}

const PACKAGES: &[QualifiedPackage] = &[
    QualifiedPackage {
        name: "libreoffice-fresh",
        version: "26.2.5-3",
        executable: "/usr/lib/libreoffice/program/soffice.bin",
    },
    QualifiedPackage {
        name: "inkscape",
        version: "1.4.4-6",
        executable: "/usr/bin/inkscape",
    },
];

/// Local opt-in (default OFF): `CUA_HYPRLAND_QUALIFY_ANY_APP=1` admits every
/// target process to the background plugin route, skipping the exact package
/// gate below. Only the literal value `1` enables it.
///
/// Why upstream gates this (see `hyprland-plugin/protocol/cua-input-v3.md`
/// "Compatibility and build gates", `host-authority-boundary.md`, and
/// `docs/production-proof.md`): background input arrives on a second, unfocused
/// `wl_seat` (`Cua-Agent*`). Whether a toolkit honours events from a non-primary
/// seat, keeps per-seat state apart, and applies them to the intended widget
/// was only proven, with native traces and app-effect checks, for the exact
/// Calc 26.2.5-3 and Inkscape 1.4.4-6 builds. Chromium/Electron and XWayland
/// raw background input are explicitly unqualified (XWayland has no multi-seat
/// path at all), and newer builds of the qualified apps were never re-proven.
/// The replies are `effect:"unverifiable"`, so an unqualified app can drop the
/// events, act on the wrong widget or toolkit-global state, or leave a press
/// half-applied while the call still looks successful. Callers must verify
/// the app effect themselves (screenshot / AT-SPI) after every action. The
/// gate is compatibility, not security: the plugin still enforces the exact
/// live surface, geometry, primary/agent conflicts, desktop state and keymap.
pub(super) const QUALIFY_ANY_APP_ENV: &str = "CUA_HYPRLAND_QUALIFY_ANY_APP";

fn qualify_any_app() -> bool {
    std::env::var(QUALIFY_ANY_APP_ENV).as_deref() == Ok("1")
}

fn field<'a>(description: &'a str, name: &str) -> Option<&'a str> {
    let mut lines = description.lines();
    while let Some(line) = lines.next() {
        if line == name {
            return lines.next().filter(|value| !value.is_empty());
        }
    }
    None
}

fn read_bounded(path: &Path) -> Result<String, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "client_qualification_unavailable")?;
    let mut content = String::new();
    file.take(1024 * 1024 + 1)
        .read_to_string(&mut content)
        .map_err(|_| "client_qualification_unavailable")?;
    if content.len() > 1024 * 1024 {
        return Err("client_qualification_unavailable");
    }
    Ok(content)
}

pub(super) fn qualify(pid: u32) -> Result<(), &'static str> {
    if qualify_any_app() {
        // Still require a live process; the plugin binds the exact surface.
        return std::fs::read_link(format!("/proc/{pid}/exe"))
            .map(|_| ())
            .map_err(|_| "client_qualification_unavailable");
    }
    let process_exe = PathBuf::from(format!("/proc/{pid}/exe"));
    let executable =
        std::fs::read_link(&process_exe).map_err(|_| "client_qualification_unavailable")?;
    let package = PACKAGES
        .iter()
        .find(|package| {
            std::fs::canonicalize(package.executable).ok().as_ref() == Some(&executable)
        })
        .ok_or("client_not_qualified")?;
    let directory = PathBuf::from("/var/lib/pacman/local")
        .join(format!("{}-{}", package.name, package.version));
    let description = read_bounded(&directory.join("desc"))?;
    if field(&description, "%NAME%") != Some(package.name)
        || field(&description, "%VERSION%") != Some(package.version)
    {
        return Err("client_not_qualified");
    }
    let files = read_bounded(&directory.join("files"))?;
    if !files
        .lines()
        .any(|line| line == package.executable.trim_start_matches('/'))
    {
        return Err("client_not_qualified");
    }
    // Detect replacement between initial identification and admission. The
    // compositor independently verifies its exact live target at dispatch.
    if std::fs::read_link(process_exe).ok().as_ref() != Some(&executable) {
        return Err("client_qualification_unavailable");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpm_identity_requires_exact_package_and_version_fields() {
        let description = "%NAME%\ninkscape\n\n%VERSION%\n1.4.4-6\n\n";
        assert_eq!(field(description, "%NAME%"), Some("inkscape"));
        assert_eq!(field(description, "%VERSION%"), Some("1.4.4-6"));
        assert_eq!(field("%VERSION%\n", "%VERSION%"), None);
        assert_eq!(field("%VERSION%\n\n", "%VERSION%"), None);
        assert_eq!(field("prefix%NAME%\ninkscape", "%NAME%"), None);
    }

    #[test]
    fn nonexistent_process_cannot_be_qualified() {
        assert!(qualify(u32::MAX).is_err());
    }

    #[test]
    fn qualify_any_app_is_opt_in_and_still_requires_a_live_process() {
        // Env mutation is process-global; keep every case in one test.
        let previous = std::env::var_os(QUALIFY_ANY_APP_ENV);
        let own = std::process::id();
        std::env::remove_var(QUALIFY_ANY_APP_ENV);
        // The test binary is not a qualified package build.
        assert_eq!(qualify(own), Err("client_not_qualified"));
        for value in ["0", "true", "yes", ""] {
            std::env::set_var(QUALIFY_ANY_APP_ENV, value);
            assert_eq!(qualify(own), Err("client_not_qualified"), "{value:?}");
        }
        std::env::set_var(QUALIFY_ANY_APP_ENV, "1");
        assert_eq!(qualify(own), Ok(()));
        assert!(qualify(u32::MAX).is_err());
        match previous {
            Some(value) => std::env::set_var(QUALIFY_ANY_APP_ENV, value),
            None => std::env::remove_var(QUALIFY_ANY_APP_ENV),
        }
    }
}
