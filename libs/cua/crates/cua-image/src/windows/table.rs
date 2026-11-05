//! The Windows version table: short ids keyed like the other OS aliases
//! (`windows:<id>` is `ghcr.io/trycua/windows:<id>`), the media each builds
//! from, and what each host architecture can run.

use crate::error::{ImageError, Result};

/// Guest CPU architecture of a Windows build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowsArch {
    Amd64,
    Arm64,
}

impl WindowsArch {
    /// The catalog and OCI spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amd64 => "amd64",
            Self::Arm64 => "arm64",
        }
    }

    /// `amd64`/`x86_64` and `arm64`/`aarch64`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "amd64" | "x86_64" | "x64" => Some(Self::Amd64),
            "arm64" | "aarch64" => Some(Self::Arm64),
            _ => None,
        }
    }

    /// The architecture of this host.
    pub fn host() -> Self {
        if cfg!(target_arch = "aarch64") {
            Self::Arm64
        } else {
            Self::Amd64
        }
    }

    /// The directory name virtio-win uses for this architecture.
    pub fn virtio_dir(self) -> &'static str {
        match self {
            Self::Amd64 => "amd64",
            Self::Arm64 => "ARM64",
        }
    }
}

/// One Windows version cua can build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsVersion {
    /// The tag: `windows:<id>`.
    pub id: &'static str,
    /// Words that also name it (`windows:server-2022`, `windows:11-enterprise`).
    pub aliases: &'static [&'static str],
    /// Pretty name, as the pickers show it.
    pub name: &'static str,
    /// Client (Windows 11, 10) or Server.
    pub server: bool,
    /// The edition the answer file installs.
    pub edition: &'static str,
    /// The Evaluation Center page slug the ISO is found on.
    pub eval_slug: &'static str,
    /// A known go.microsoft.com link id, used when the page cannot be read.
    pub eval_linkid: Option<&'static str>,
    /// Architectures Microsoft publishes evaluation media for.
    pub eval_arch: &'static [WindowsArch],
    /// Architectures cua can build with your own ISO.
    pub iso_arch: &'static [WindowsArch],
    /// Disk size in GiB.
    pub disk_gb: u32,
    /// Days the evaluation runs before it needs a licence.
    pub eval_days: u32,
    /// Whether CI publishes a prebuilt image under this tag (pulled, not built).
    pub published: bool,
}

use WindowsArch::{Amd64, Arm64};

const VERSIONS: &[WindowsVersion] = &[
    WindowsVersion {
        id: "11",
        aliases: &[
            "win11",
            "11e",
            "11-enterprise",
            "11-enterprise-eval",
            "11-eval",
            "windows11",
        ],
        name: "Windows 11 Enterprise Evaluation",
        server: false,
        edition: "Windows 11 Enterprise Evaluation",
        eval_slug: "windows-11-enterprise",
        eval_linkid: Some("2382600"),
        // Microsoft's evaluation centre has no ARM64 Windows 11 ISO: on an
        // Apple silicon Mac you bring the ARM64 ISO (`--iso`).
        eval_arch: &[Amd64],
        iso_arch: &[Amd64, Arm64],
        disk_gb: 64,
        eval_days: 90,
        published: false,
    },
    WindowsVersion {
        id: "10",
        aliases: &[
            "win10",
            "10e",
            "10-enterprise",
            "10-enterprise-eval",
            "windows10",
        ],
        name: "Windows 10 Enterprise Evaluation",
        server: false,
        edition: "Windows 10 Enterprise Evaluation",
        eval_slug: "windows-10-enterprise",
        eval_linkid: None,
        eval_arch: &[Amd64],
        iso_arch: &[Amd64],
        disk_gb: 64,
        eval_days: 90,
        published: false,
    },
    WindowsVersion {
        id: "2025",
        aliases: &["server-2025", "server2025", "ws2025", "win2025"],
        name: "Windows Server 2025 Evaluation",
        server: true,
        edition: "Windows Server 2025 Standard Evaluation (Desktop Experience)",
        eval_slug: "windows-server-2025",
        eval_linkid: Some("2345730"),
        eval_arch: &[Amd64],
        iso_arch: &[Amd64],
        disk_gb: 64,
        eval_days: 180,
        published: false,
    },
    WindowsVersion {
        id: "2022",
        aliases: &["server-2022", "server2022", "ws2022", "win2022"],
        name: "Windows Server 2022",
        server: true,
        edition: "Windows Server 2022 Standard Evaluation (Desktop Experience)",
        eval_slug: "windows-server-2022",
        eval_linkid: None,
        eval_arch: &[Amd64],
        iso_arch: &[Amd64],
        disk_gb: 64,
        eval_days: 180,
        // The default: `ghcr.io/trycua/windows:2022` is pulled, not built.
        published: true,
    },
    WindowsVersion {
        id: "2019",
        aliases: &["server-2019", "server2019", "ws2019", "win2019"],
        name: "Windows Server 2019 Evaluation",
        server: true,
        edition: "Windows Server 2019 SERVERDATACENTEREVAL",
        eval_slug: "windows-server-2019",
        eval_linkid: None,
        eval_arch: &[Amd64],
        iso_arch: &[Amd64],
        disk_gb: 64,
        eval_days: 180,
        published: false,
    },
];

/// Every version, newest client first.
pub fn versions() -> &'static [WindowsVersion] {
    VERSIONS
}

/// The tag a version word names (`server-2022` is `2022`), or the word
/// unchanged when it is not a Windows version (a custom tag passes through).
pub fn normalize_version(word: &str) -> String {
    let w = word.trim().to_ascii_lowercase();
    VERSIONS
        .iter()
        .find(|v| v.id == w || v.aliases.contains(&w.as_str()))
        .map(|v| v.id.to_string())
        .unwrap_or_else(|| word.trim().to_string())
}

/// The version a word or tag names.
pub fn lookup(word: &str) -> Option<&'static WindowsVersion> {
    let id = normalize_version(word);
    VERSIONS.iter().find(|v| v.id == id)
}

impl WindowsVersion {
    /// Whether this version is built locally rather than pulled.
    pub fn builds_locally(&self) -> bool {
        !self.published
    }

    /// Whether `arch` can be built at all (with a user ISO if need be).
    pub fn supports(&self, arch: WindowsArch) -> bool {
        self.iso_arch.contains(&arch)
    }

    /// Whether Microsoft's evaluation media can be downloaded for `arch`.
    pub fn has_eval_media(&self, arch: WindowsArch) -> bool {
        self.eval_arch.contains(&arch)
    }

    /// Checks that `arch` is buildable, with the way out in the message.
    pub fn check_arch(&self, arch: WindowsArch) -> Result<()> {
        if self.supports(arch) {
            return Ok(());
        }
        Err(ImageError::Reference(
            format!("windows:{}", self.id),
            format!(
                "{} has no {} build; it runs on {} hosts",
                self.name,
                arch.as_str(),
                self.iso_arch
                    .iter()
                    .map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_normalise_to_the_tag() {
        for (word, id) in [
            ("11", "11"),
            ("Win11", "11"),
            ("11-enterprise-eval", "11"),
            ("server-2022", "2022"),
            ("WS2025", "2025"),
            ("2019", "2019"),
        ] {
            assert_eq!(normalize_version(word), id, "{word}");
        }
        // Not a Windows version: unchanged, so a custom tag still passes through.
        assert_eq!(normalize_version("2022-slim"), "2022-slim");
        assert!(lookup("2022-slim").is_none());
    }

    #[test]
    fn ids_and_aliases_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for v in versions() {
            assert!(seen.insert(v.id), "{}", v.id);
            for a in v.aliases {
                assert!(seen.insert(a), "{a}");
            }
        }
    }

    #[test]
    fn the_default_is_published_and_the_rest_build_locally() {
        assert!(!lookup("2022").unwrap().builds_locally());
        assert!(lookup("11").unwrap().builds_locally());
        assert!(lookup("2025").unwrap().builds_locally());
    }

    #[test]
    fn arm_windows_11_needs_your_iso() {
        let v = lookup("11").unwrap();
        assert!(v.supports(WindowsArch::Arm64));
        assert!(!v.has_eval_media(WindowsArch::Arm64));
        assert!(v.has_eval_media(WindowsArch::Amd64));
        let s = lookup("2025").unwrap();
        let e = s.check_arch(WindowsArch::Arm64).unwrap_err().to_string();
        assert!(e.contains("amd64"), "{e}");
    }

    #[test]
    fn arch_words() {
        assert_eq!(WindowsArch::parse("aarch64"), Some(WindowsArch::Arm64));
        assert_eq!(WindowsArch::parse("x86_64"), Some(WindowsArch::Amd64));
        assert_eq!(WindowsArch::parse("riscv"), None);
    }
}
