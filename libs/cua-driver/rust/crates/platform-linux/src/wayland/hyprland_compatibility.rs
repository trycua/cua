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
}
