//! Path normalization and reparse-point resolution for the Windows isolated
//! browser installation check.
//!
//! A trusted browser candidate such as
//! `C:\Program Files\Google\Chrome\Application\chrome.exe` may reach its files
//! through directory junctions or symbolic links, for example when an
//! administrator relocates `Application` to another volume. Rejecting every
//! redirect refuses such installs as "not installed"; following redirects
//! without checking them would let a writable link target replace the browser.
//!
//! [`resolve_installation`] walks the candidate one component at a time. Each
//! link must keep the candidate's relative layout: a link reached at
//! `<root>\Google\Chrome\Application` must point to some
//! `<other root>\Google\Chrome\Application`. The walk returns the final real
//! executable and every object whose modification could change what that
//! path opens: each link object itself, each real directory and the
//! executable, and each new root a link moves to. The caller requires the
//! browser launch token to be denied write access to all of them, then
//! cross-checks the result against the operating system's own resolution.
//!
//! The module is pure. Filesystem access goes through [`InstallationFs`], so
//! the unit tests run on any host.

/// Upper bound on links followed for one candidate.
pub(crate) const MAX_REPARSE_HOPS: usize = 8;

/// Convert a Windows path to its plain Win32 form.
///
/// `\\?\C:\x` becomes `C:\x`, `\\?\UNC\server\share\x` becomes
/// `\\server\share\x`, and the NT form `\??\C:\x` stored in junction data
/// becomes `C:\x`. A path without a verbatim prefix is returned unchanged.
/// Verbatim forms with no plain equivalent, such as `\\?\Volume{...}\` or
/// `\\?\GLOBALROOT\`, return `None` so callers fail closed.
pub(crate) fn plain_windows_path(path: &str) -> Option<String> {
    let verbatim = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\??\"));
    let Some(rest) = verbatim else {
        return Some(path.to_owned());
    };
    // Verbatim paths are passed to the filesystem without normalization, so
    // `/` is an ordinary character there; such a path has no plain form.
    if rest.contains('/') {
        return None;
    }
    if let Some(unc) = strip_prefix_ignore_ascii_case(rest, r"UNC\") {
        let mut parts = unc.splitn(3, '\\');
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        if server.is_empty() || share.is_empty() {
            return None;
        }
        return Some(format!(r"\\{unc}"));
    }
    let bytes = rest.as_bytes();
    let is_drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'\\');
    is_drive.then(|| rest.to_owned())
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &value[prefix.len()..])
}

/// Split an absolute local drive path into `["C:", component, ...]`.
///
/// Network, device, volume-GUID, relative, and drive-relative paths return
/// `None`, as do paths with `.` or `..` components, so a redirect can never
/// leave the local drive namespace or escape its claimed layout.
fn drive_components(path: &str) -> Option<Vec<String>> {
    let plain = plain_windows_path(path)?;
    let bytes = plain.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return None;
    }
    let mut components = vec![plain[..2].to_ascii_uppercase()];
    let mut parts = plain[3..].split(['\\', '/']).peekable();
    while let Some(part) = parts.next() {
        if part.is_empty() {
            // Only a single trailing separator is tolerated.
            if parts.peek().is_none() {
                break;
            }
            return None;
        }
        if part == "." || part == ".." {
            return None;
        }
        components.push(part.to_owned());
    }
    Some(components)
}

fn join_components(components: &[String]) -> String {
    match components {
        [] => String::new(),
        [drive] => format!(r"{drive}\"),
        [drive, rest @ ..] => format!(r"{drive}\{}", rest.join(r"\")),
    }
}

fn same_component(left: &str, right: &str) -> bool {
    left == right || left.to_lowercase() == right.to_lowercase()
}

fn same_components(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same_component(left, right))
}

/// What a path names, without following a link at its final component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallationEntry {
    Missing,
    File,
    Directory,
    /// A junction or symbolic link and its stored target.
    Link(String),
}

/// Filesystem queries the resolution needs.
pub(crate) trait InstallationFs {
    /// Classify `path` without following a link at its final component.
    fn entry(&self, path: &str) -> Result<InstallationEntry, String>;
    /// Fully resolve `path` as the operating system would open it.
    fn canonical(&self, path: &str) -> Result<String, String>;
}

/// One object whose write access the launch token must be denied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProbeTarget {
    pub path: String,
    pub directory: bool,
    /// Probe the link object itself rather than its target.
    pub link: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedInstallation {
    /// The real executable path in plain drive form.
    pub executable: String,
    /// Objects to probe, in walk order.
    pub probes: Vec<ProbeTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallationResolution {
    /// The candidate does not exist.
    Missing,
    /// The candidate exists but cannot be proven to be a protected
    /// installation; the reason explains why.
    Untrusted(String),
    Resolved(ResolvedInstallation),
}

/// Resolve `executable` below `trusted_root`, following each link only when
/// it keeps the executable's relative layout. See the module documentation.
pub(crate) fn resolve_installation(
    executable: &str,
    trusted_root: &str,
    fs: &impl InstallationFs,
) -> InstallationResolution {
    use InstallationResolution::{Missing, Resolved, Untrusted};

    let Some(root) = drive_components(trusted_root) else {
        return Untrusted(format!(
            "trusted installation root {trusted_root} is not a local drive path"
        ));
    };
    let Some(candidate) = drive_components(executable) else {
        return Untrusted(format!("{executable} is not a local drive path"));
    };
    if candidate.len() <= root.len() || !same_components(&candidate[..root.len()], &root) {
        return Untrusted(format!("{executable} is outside {trusted_root}"));
    }
    let tail = &candidate[root.len()..];

    // `base` is the real directory that currently stands in for the trusted
    // root; the path being examined is `base` plus the first `depth` tail
    // components.
    let mut base = root;
    let mut probes = Vec::new();
    let mut hops = 0;
    for depth in 0..=tail.len() {
        let directory = depth < tail.len();
        loop {
            let mut current = base.clone();
            current.extend_from_slice(&tail[..depth]);
            let current_path = join_components(&current);
            let entry = match fs.entry(&current_path) {
                Ok(entry) => entry,
                Err(error) => {
                    return Untrusted(format!("could not inspect {current_path}: {error}"))
                }
            };
            match entry {
                InstallationEntry::Missing => return Missing,
                InstallationEntry::Directory if directory => {
                    probes.push(ProbeTarget {
                        path: current_path,
                        directory: true,
                        link: false,
                    });
                    break;
                }
                InstallationEntry::File if !directory => {
                    probes.push(ProbeTarget {
                        path: current_path,
                        directory: false,
                        link: false,
                    });
                    break;
                }
                // A file where a directory belongs, or a directory where the
                // executable belongs: the candidate is not installed.
                InstallationEntry::File | InstallationEntry::Directory => return Missing,
                InstallationEntry::Link(target) => {
                    hops += 1;
                    if hops > MAX_REPARSE_HOPS {
                        return Untrusted(format!(
                            "{executable} passes through more than {MAX_REPARSE_HOPS} links"
                        ));
                    }
                    probes.push(ProbeTarget {
                        path: current_path.clone(),
                        directory,
                        link: true,
                    });
                    let Some(target_components) = drive_components(&target) else {
                        return Untrusted(format!(
                            "{current_path} links to {target}, which is not an absolute local \
                             drive path"
                        ));
                    };
                    let kept = &tail[..depth];
                    let split = target_components.len().saturating_sub(kept.len());
                    if split == 0 || !same_components(&target_components[split..], kept) {
                        return Untrusted(format!(
                            "{current_path} links to {target}, which does not keep the \
                             installation layout below {trusted_root}"
                        ));
                    }
                    base = target_components[..split].to_vec();
                    // The new root and the directories between it and the
                    // link target are part of the real installation chain.
                    for prefix in 0..depth {
                        let mut ancestor = base.clone();
                        ancestor.extend_from_slice(&tail[..prefix]);
                        probes.push(ProbeTarget {
                            path: join_components(&ancestor),
                            directory: true,
                            link: false,
                        });
                    }
                    // Re-examine the target itself, which may be another link.
                }
            }
        }
    }

    let mut resolved = base;
    resolved.extend_from_slice(tail);
    let resolved_path = join_components(&resolved);
    // The walk sees links only at the components it visits. A link anywhere
    // else (above the trusted root or above a link's new root) would make the
    // operating system open a different file, so require both views to agree.
    match fs.canonical(executable).map(|path| drive_components(&path)) {
        Ok(Some(canonical)) if same_components(&canonical, &resolved) => {
            Resolved(ResolvedInstallation {
                executable: resolved_path,
                probes,
            })
        }
        Ok(_) => Untrusted(format!(
            "{executable} resolves somewhere other than {resolved_path}"
        )),
        Err(error) => Untrusted(format!("could not resolve {executable}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn plain_windows_path_strips_verbatim_prefixes() {
        assert_eq!(
            plain_windows_path(r"\\?\C:\Program Files\Google\Chrome\Application\chrome.exe")
                .as_deref(),
            Some(r"C:\Program Files\Google\Chrome\Application\chrome.exe")
        );
        assert_eq!(
            plain_windows_path(r"\\?\UNC\server\share\Chrome\chrome.exe").as_deref(),
            Some(r"\\server\share\Chrome\chrome.exe")
        );
        assert_eq!(
            plain_windows_path(r"\\?\unc\server\share").as_deref(),
            Some(r"\\server\share")
        );
        assert_eq!(
            plain_windows_path(r"\??\E:\Program Files\Google").as_deref(),
            Some(r"E:\Program Files\Google")
        );
        assert_eq!(plain_windows_path(r"\\?\D:").as_deref(), Some(r"D:"));
        assert_eq!(
            plain_windows_path(r"C:\Program Files").as_deref(),
            Some(r"C:\Program Files")
        );
    }

    #[test]
    fn plain_windows_path_refuses_verbatim_forms_without_a_plain_equivalent() {
        for path in [
            r"\\?\Volume{0b1f7c33-5a3c-4c1e-9d7a-3f1d2a6b8c11}\Program Files",
            r"\\?\GLOBALROOT\Device\HarddiskVolume3\Program Files",
            r"\\?\UNC\server",
            r"\\?\UNC\\share",
            r"\\?\C:/Program Files",
            r"\\?\CC:\x",
        ] {
            assert_eq!(plain_windows_path(path), None, "{path}");
        }
    }

    #[test]
    fn drive_components_accepts_only_absolute_local_drive_paths() {
        assert_eq!(
            drive_components(r"\\?\c:\Program Files\"),
            Some(vec!["C:".to_owned(), "Program Files".to_owned()])
        );
        assert_eq!(
            drive_components(r"E:/Program Files/Google"),
            Some(vec![
                "E:".to_owned(),
                "Program Files".to_owned(),
                "Google".to_owned()
            ])
        );
        for path in [
            r"\\server\share\Program Files",
            r"\\?\UNC\server\share\Program Files",
            r"Program Files\Google",
            r"C:Program Files",
            r"C:\Program Files\..\Users",
            r"C:\Program Files\.\Google",
            r"C:\Program Files\\Google",
            r"\\.\C:\Program Files",
        ] {
            assert_eq!(drive_components(path), None, "{path}");
        }
    }

    /// In-memory filesystem: lowercase path -> entry. `canonical` follows the
    /// links the same way the operating system does, at every component.
    #[derive(Default)]
    struct FakeFs {
        entries: HashMap<String, InstallationEntry>,
    }

    impl FakeFs {
        fn dir(mut self, path: &str) -> Self {
            self.entries
                .insert(path.to_lowercase(), InstallationEntry::Directory);
            self
        }
        fn file(mut self, path: &str) -> Self {
            self.entries
                .insert(path.to_lowercase(), InstallationEntry::File);
            self
        }
        fn link(mut self, path: &str, target: &str) -> Self {
            self.entries.insert(
                path.to_lowercase(),
                InstallationEntry::Link(target.to_owned()),
            );
            self
        }
        /// Directories for `path` and each of its ancestors.
        fn tree(mut self, path: &str) -> Self {
            let components = drive_components(path).unwrap();
            for end in 1..=components.len() {
                self = self.dir(&join_components(&components[..end]));
            }
            self
        }
    }

    impl InstallationFs for FakeFs {
        fn entry(&self, path: &str) -> Result<InstallationEntry, String> {
            // Resolve links in the parent, as a real lookup would.
            let components = drive_components(path).ok_or("bad path")?;
            if components.len() > 1 {
                let parent = join_components(&components[..components.len() - 1]);
                let parent = plain_windows_path(&self.canonical(&parent)?).ok_or("bad parent")?;
                let real = format!(
                    r"{}\{}",
                    parent.trim_end_matches('\\'),
                    components.last().unwrap()
                );
                if real.to_lowercase() != path.to_lowercase() {
                    return self.entry(&real);
                }
            }
            Ok(self
                .entries
                .get(&path.to_lowercase())
                .cloned()
                .unwrap_or(InstallationEntry::Missing))
        }

        fn canonical(&self, path: &str) -> Result<String, String> {
            self.resolve(path, 0)
                .map(|resolved| format!(r"\\?\{resolved}"))
        }
    }

    impl FakeFs {
        /// Follow links at every component, including inside link targets.
        fn resolve(&self, path: &str, depth: usize) -> Result<String, String> {
            if depth > 32 {
                return Err("link loop".to_owned());
            }
            let components = drive_components(path).ok_or("bad path")?;
            let mut resolved = vec![components[0].clone()];
            for component in &components[1..] {
                resolved.push(component.clone());
                if let Some(InstallationEntry::Link(target)) =
                    self.entries.get(&join_components(&resolved).to_lowercase())
                {
                    let target = self.resolve(target, depth + 1)?;
                    resolved = drive_components(&target).ok_or("bad target")?;
                }
            }
            Ok(join_components(&resolved))
        }
    }

    const ROOT: &str = r"C:\Program Files";
    const CHROME: &str = r"C:\Program Files\Google\Chrome\Application\chrome.exe";

    fn resolved(resolution: InstallationResolution) -> ResolvedInstallation {
        match resolution {
            InstallationResolution::Resolved(resolved) => resolved,
            other => panic!("expected a resolved installation, got {other:?}"),
        }
    }

    fn untrusted_reason(resolution: InstallationResolution) -> String {
        match resolution {
            InstallationResolution::Untrusted(reason) => reason,
            other => panic!("expected an untrusted installation, got {other:?}"),
        }
    }

    fn probe(path: &str, directory: bool, link: bool) -> ProbeTarget {
        ProbeTarget {
            path: path.to_owned(),
            directory,
            link,
        }
    }

    #[test]
    fn plain_installation_probes_the_executable_and_each_directory_up_to_the_root() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome\Application")
            .file(CHROME);
        let resolved = resolved(resolve_installation(CHROME, ROOT, &fs));
        assert_eq!(resolved.executable, CHROME);
        assert_eq!(
            resolved.probes,
            vec![
                probe(r"C:\Program Files", true, false),
                probe(r"C:\Program Files\Google", true, false),
                probe(r"C:\Program Files\Google\Chrome", true, false),
                probe(r"C:\Program Files\Google\Chrome\Application", true, false),
                probe(CHROME, false, false),
            ]
        );
    }

    #[test]
    fn verbatim_paths_resolve_to_plain_drive_paths() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome\Application")
            .file(CHROME);
        let resolved = resolved(resolve_installation(
            &format!(r"\\?\{CHROME}"),
            &format!(r"\\?\{ROOT}"),
            &fs,
        ));
        assert_eq!(resolved.executable, CHROME);
        assert!(resolved
            .probes
            .iter()
            .all(|probe| !probe.path.starts_with(r"\\")));
    }

    #[test]
    fn in_tree_junction_resolves_to_the_relocated_installation() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome")
            .link(
                r"C:\Program Files\Google\Chrome\Application",
                r"\\?\E:\Program Files\Google\Chrome\Application",
            )
            .tree(r"E:\Program Files\Google\Chrome\Application")
            .file(r"E:\Program Files\Google\Chrome\Application\chrome.exe");
        let resolved = resolved(resolve_installation(CHROME, ROOT, &fs));
        assert_eq!(
            resolved.executable,
            r"E:\Program Files\Google\Chrome\Application\chrome.exe"
        );
        assert_eq!(
            resolved.probes,
            vec![
                probe(r"C:\Program Files", true, false),
                probe(r"C:\Program Files\Google", true, false),
                probe(r"C:\Program Files\Google\Chrome", true, false),
                // The junction object itself, then the real chain it reaches.
                probe(r"C:\Program Files\Google\Chrome\Application", true, true),
                probe(r"E:\Program Files", true, false),
                probe(r"E:\Program Files\Google", true, false),
                probe(r"E:\Program Files\Google\Chrome", true, false),
                probe(r"E:\Program Files\Google\Chrome\Application", true, false),
                probe(
                    r"E:\Program Files\Google\Chrome\Application\chrome.exe",
                    false,
                    false
                ),
            ]
        );
    }

    #[test]
    fn every_link_in_a_chain_and_each_new_root_are_probed() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google")
            .link(
                r"C:\Program Files\Google\Chrome",
                r"\??\E:\Apps\Google\Chrome",
            )
            .tree(r"E:\Apps\Google")
            .link(r"E:\Apps\Google\Chrome", r"F:\Program Files\Google\Chrome")
            .tree(r"F:\Program Files\Google\Chrome\Application")
            .link(
                r"F:\Program Files\Google\Chrome\Application\chrome.exe",
                r"G:\Program Files\Google\Chrome\Application\chrome.exe",
            )
            .tree(r"G:\Program Files\Google\Chrome\Application")
            .file(r"G:\Program Files\Google\Chrome\Application\chrome.exe");
        let resolved = resolved(resolve_installation(CHROME, ROOT, &fs));
        assert_eq!(
            resolved.executable,
            r"G:\Program Files\Google\Chrome\Application\chrome.exe"
        );
        let links: Vec<_> = resolved
            .probes
            .iter()
            .filter(|probe| probe.link)
            .map(|probe| probe.path.as_str())
            .collect();
        assert_eq!(
            links,
            [
                r"C:\Program Files\Google\Chrome",
                r"E:\Apps\Google\Chrome",
                r"F:\Program Files\Google\Chrome\Application\chrome.exe",
            ]
        );
        for directory in [
            r"E:\Apps",
            r"E:\Apps\Google",
            r"F:\Program Files",
            r"F:\Program Files\Google",
            r"G:\Program Files\Google\Chrome\Application",
        ] {
            assert!(
                resolved.probes.contains(&probe(directory, true, false)),
                "{directory} must be probed: {:?}",
                resolved.probes
            );
        }
        // The file link is probed as a file object.
        assert!(resolved.probes.contains(&probe(
            r"F:\Program Files\Google\Chrome\Application\chrome.exe",
            false,
            true
        )));
    }

    #[test]
    fn junctioned_trusted_root_resolves_with_its_target_probed() {
        let fs = FakeFs::default()
            .tree(r"C:\")
            .link(r"C:\Program Files", r"\??\E:\Program Files")
            .tree(r"E:\Program Files\Google\Chrome\Application")
            .file(r"E:\Program Files\Google\Chrome\Application\chrome.exe");
        let resolved = resolved(resolve_installation(CHROME, ROOT, &fs));
        assert_eq!(resolved.probes[0], probe(r"C:\Program Files", true, true));
        assert_eq!(resolved.probes[1], probe(r"E:\Program Files", true, false));
    }

    #[test]
    fn link_that_changes_the_installation_layout_is_untrusted() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome")
            .link(
                r"C:\Program Files\Google\Chrome\Application",
                r"\??\C:\Users\Public\Downloads",
            )
            .tree(r"C:\Users\Public\Downloads")
            .file(r"C:\Users\Public\Downloads\chrome.exe");
        let reason = untrusted_reason(resolve_installation(CHROME, ROOT, &fs));
        assert!(
            reason.contains("does not keep the installation layout"),
            "{reason}"
        );

        // The file name itself must be kept too.
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome\Application")
            .link(
                CHROME,
                r"C:\Program Files\Google\Chrome\Application\other.exe",
            )
            .file(r"C:\Program Files\Google\Chrome\Application\other.exe");
        untrusted_reason(resolve_installation(CHROME, ROOT, &fs));
    }

    #[test]
    fn link_to_a_drive_root_keeps_the_writable_root_in_the_probe_set() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files")
            .link(r"C:\Program Files\Google", r"\??\D:\Google")
            .tree(r"D:\Google\Chrome\Application")
            .file(r"D:\Google\Chrome\Application\chrome.exe");
        let resolved = resolved(resolve_installation(CHROME, ROOT, &fs));
        assert!(resolved.probes.contains(&probe(r"D:\", true, false)));
    }

    #[test]
    fn link_to_a_network_volume_or_relative_target_is_untrusted() {
        for target in [
            r"\\?\UNC\server\share\Google\Chrome\Application",
            r"\\server\share\Google\Chrome\Application",
            r"\??\Volume{0b1f7c33-5a3c-4c1e-9d7a-3f1d2a6b8c11}\Google\Chrome\Application",
            r"..\..\..\Elsewhere\Google\Chrome\Application",
            r"Google\Chrome\Application",
        ] {
            let fs = FakeFs::default()
                .tree(r"C:\Program Files\Google\Chrome")
                .link(r"C:\Program Files\Google\Chrome\Application", target);
            let reason = untrusted_reason(resolve_installation(CHROME, ROOT, &fs));
            assert!(
                reason.contains("not an absolute local drive path"),
                "{target}: {reason}"
            );
        }
    }

    #[test]
    fn link_loops_are_bounded() {
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome")
            .link(
                r"C:\Program Files\Google\Chrome\Application",
                r"D:\Program Files\Google\Chrome\Application",
            )
            .tree(r"D:\Program Files\Google\Chrome")
            .link(
                r"D:\Program Files\Google\Chrome\Application",
                r"C:\Program Files\Google\Chrome\Application",
            );
        let reason = untrusted_reason(resolve_installation(CHROME, ROOT, &fs));
        assert!(reason.contains("more than"), "{reason}");
    }

    #[test]
    fn unseen_link_above_a_root_is_caught_by_the_canonical_cross_check() {
        // `E:\Program Files` is itself a junction. The walk only visits the
        // components below the new root, so the operating system's
        // resolution must veto the result.
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome")
            .link(
                r"C:\Program Files\Google\Chrome\Application",
                r"E:\Program Files\Google\Chrome\Application",
            )
            .tree(r"E:\")
            .link(r"E:\Program Files", r"F:\Writable")
            .tree(r"F:\Writable\Google\Chrome\Application")
            .file(r"F:\Writable\Google\Chrome\Application\chrome.exe");
        let reason = untrusted_reason(resolve_installation(CHROME, ROOT, &fs));
        assert!(reason.contains("resolves somewhere other than"), "{reason}");
    }

    #[test]
    fn candidate_outside_the_trusted_root_is_untrusted_and_absent_one_is_missing() {
        let fs = FakeFs::default()
            .tree(r"D:\UserControlled")
            .file(r"D:\UserControlled\chrome.exe");
        untrusted_reason(resolve_installation(
            r"D:\UserControlled\chrome.exe",
            ROOT,
            &fs,
        ));
        untrusted_reason(resolve_installation(ROOT, ROOT, &fs));

        let fs = FakeFs::default().tree(r"C:\Program Files\Google");
        assert_eq!(
            resolve_installation(CHROME, ROOT, &fs),
            InstallationResolution::Missing
        );
        let fs = FakeFs::default()
            .tree(r"C:\Program Files\Google\Chrome\Application")
            .tree(CHROME);
        assert_eq!(
            resolve_installation(CHROME, ROOT, &fs),
            InstallationResolution::Missing
        );
    }

    #[test]
    fn inspection_errors_fail_closed() {
        struct Failing;
        impl InstallationFs for Failing {
            fn entry(&self, _: &str) -> Result<InstallationEntry, String> {
                Err("access denied".to_owned())
            }
            fn canonical(&self, _: &str) -> Result<String, String> {
                Err("access denied".to_owned())
            }
        }
        untrusted_reason(resolve_installation(CHROME, ROOT, &Failing));
    }
}
