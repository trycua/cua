//! The build context: which files a `COPY` / `ADD` copies, where they land,
//! and what their contents hash to (the cache key of the layer).
//!
//! Semantics follow Docker's: sources are paths (or globs) relative to the
//! context directory and may not leave it; `.dockerignore` (patterns anchored
//! at the context root, `!` re-includes, `**` crosses directories) removes
//! files from the context; a directory source copies its contents; a file
//! lands at `dest` when `dest` does not end in `/` and there is one source,
//! else inside `dest`; symlinks are copied as symlinks; `ADD` unpacks a local
//! tar archive into `dest`.

use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Most files one `COPY` may copy (a mistaken `/` must not be unbounded).
pub const MAX_COPY_ENTRIES: usize = 200_000;

/// One thing a `COPY` creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CopyEntry {
    /// Absolute path in the image.
    pub dest: String,
    /// `chmod`-style permission bits.
    pub mode: u32,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EntryKind {
    File {
        host: PathBuf,
        sha256: String,
        size: u64,
    },
    Dir,
    Symlink {
        target: String,
    },
    /// A local tar archive unpacked into `dest` (a directory).
    Archive {
        host: PathBuf,
        sha256: String,
        size: u64,
    },
}

impl CopyEntry {
    /// The entry as stable text for the layer's cache key.
    pub fn key_text(&self) -> String {
        match &self.kind {
            EntryKind::File { sha256, size, .. } => {
                format!("f {} {:o} {size} {sha256}", self.dest, self.mode)
            }
            EntryKind::Dir => format!("d {} {:o}", self.dest, self.mode),
            EntryKind::Symlink { target } => format!("l {} {target}", self.dest),
            EntryKind::Archive { sha256, size, .. } => {
                format!("a {} {size} {sha256}", self.dest)
            }
        }
    }
}

/// Patterns of `<context>/.dockerignore`, as Docker reads them.
pub(super) struct DockerIgnore(Option<Gitignore>);

impl DockerIgnore {
    pub fn load(context: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(context.join(".dockerignore")) else {
            return Self(None);
        };
        Self(Self::parse(context, &text))
    }

    pub fn parse(context: &Path, text: &str) -> Option<Gitignore> {
        let mut b = GitignoreBuilder::new(context);
        let mut any = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (neg, pat) = match line.strip_prefix('!') {
                Some(p) => (true, p.trim()),
                None => (false, line),
            };
            // Docker anchors every pattern at the context root.
            let pat = pat.trim_start_matches('/');
            let pat = pat.strip_prefix("./").unwrap_or(pat);
            if pat.is_empty() {
                continue;
            }
            let line = format!("{}/{pat}", if neg { "!" } else { "" });
            if b.add_line(None, &line).is_ok() {
                any = true;
            }
        }
        if !any {
            return None;
        }
        b.build().ok()
    }

    /// Whether `rel` (a context-relative path) is excluded, itself or by
    /// an excluded parent directory.
    pub fn excludes(&self, context: &Path, rel: &str, is_dir: bool) -> bool {
        match &self.0 {
            Some(g) => g
                .matched_path_or_any_parents(context.join(rel), is_dir)
                .is_ignore(),
            None => false,
        }
    }
}

fn file_sha(path: &Path) -> Result<(String, u64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)
        .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(h.finalize()), size))
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    if meta.is_dir() { 0o755 } else { 0o644 }
}

/// Whether `name` is a tar archive `ADD` unpacks.
pub(super) fn is_archive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tbz", ".tar.xz", ".txz", ".tar.zst",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

fn clean_source(src: &str) -> Result<String> {
    let s = src.trim_start_matches("./").trim_start_matches('/');
    let mut parts: Vec<&str> = Vec::new();
    for c in Path::new(s).components() {
        match c {
            Component::Normal(p) => parts.push(p.to_str().unwrap_or("")),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::invalid(format!(
                    "COPY source {src:?} is outside the build context"
                )));
            }
            _ => {}
        }
    }
    Ok(parts.join("/"))
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// The context's paths under `rel` (itself first), not following symlinks.
fn walk(context: &Path, rel: &str, ignore: &DockerIgnore, out: &mut Vec<String>) -> Result<()> {
    let abs = if rel.is_empty() {
        context.to_path_buf()
    } else {
        context.join(rel)
    };
    let mut entries: Vec<_> = std::fs::read_dir(&abs)
        .map_err(|e| Error::invalid(format!("{}: {e}", abs.display())))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let ft = e.file_type()?;
        let is_dir = ft.is_dir();
        if ignore.excludes(context, &child, is_dir) {
            continue;
        }
        out.push(child.clone());
        if out.len() > MAX_COPY_ENTRIES {
            return Err(Error::invalid(format!(
                "COPY would copy more than {MAX_COPY_ENTRIES} files; narrow the source or add a .dockerignore"
            )));
        }
        if is_dir {
            walk(context, &child, ignore, out)?;
        }
    }
    Ok(())
}

fn join_dest(dest: &str, rel: &str) -> String {
    if rel.is_empty() {
        dest.trim_end_matches('/').to_string()
    } else {
        format!("{}/{}", dest.trim_end_matches('/'), rel)
    }
}

fn entry_for(context: &Path, rel: &str, dest: String) -> Result<CopyEntry> {
    let abs = context.join(rel);
    let meta = std::fs::symlink_metadata(&abs)
        .map_err(|e| Error::invalid(format!("{}: {e}", abs.display())))?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        let target = std::fs::read_link(&abs)?.to_string_lossy().to_string();
        return Ok(CopyEntry {
            dest,
            mode: 0o777,
            kind: EntryKind::Symlink { target },
        });
    }
    if ft.is_dir() {
        return Ok(CopyEntry {
            dest,
            mode: mode_of(&meta),
            kind: EntryKind::Dir,
        });
    }
    if !ft.is_file() {
        return Err(Error::invalid(format!(
            "{}: not a regular file, directory or symlink",
            abs.display()
        )));
    }
    let (sha256, size) = file_sha(&abs)?;
    Ok(CopyEntry {
        dest,
        mode: mode_of(&meta),
        kind: EntryKind::File {
            host: abs,
            sha256,
            size,
        },
    })
}

/// What `sources` (context-relative paths or globs) copy to `dest` (absolute
/// in the image), sorted. `unpack` is `ADD`.
pub(super) fn plan_copy(
    context: Option<&Path>,
    sources: &[String],
    dest: &str,
    unpack: bool,
) -> Result<Vec<CopyEntry>> {
    let verb = if unpack { "ADD" } else { "COPY" };
    let context = context.ok_or_else(|| {
        Error::invalid(format!(
            "{verb} {} needs a build context: pass the directory it is relative to",
            sources.join(" ")
        ))
    })?;
    let ignore = DockerIgnore::load(context);
    // Everything in the context, for globs.
    let mut universe: Option<Vec<String>> = None;
    // (matched relative path, is it a literal source)
    let mut matched: Vec<String> = Vec::new();
    for src in sources {
        let rel = clean_source(src)?;
        if has_glob(&rel) {
            if universe.is_none() {
                let mut all = Vec::new();
                walk(context, "", &ignore, &mut all)?;
                universe = Some(all);
            }
            let glob = globset::GlobBuilder::new(&rel)
                .literal_separator(true)
                .build()
                .map_err(|e| Error::invalid(format!("{verb} source {src:?}: {e}")))?
                .compile_matcher();
            let before = matched.len();
            for p in universe.as_ref().expect("walked") {
                if glob.is_match(p) {
                    matched.push(p.clone());
                }
            }
            if matched.len() == before {
                return Err(Error::invalid(format!(
                    "{verb} {src:?}: no files match in the build context (or .dockerignore excludes them)"
                )));
            }
        } else {
            let abs = context.join(&rel);
            let exists = std::fs::symlink_metadata(&abs).is_ok();
            let is_dir = abs.is_dir();
            if !exists || (!rel.is_empty() && ignore.excludes(context, &rel, is_dir)) {
                return Err(Error::invalid(format!(
                    "{verb} {src:?}: not found in the build context (or excluded by .dockerignore)"
                )));
            }
            matched.push(rel);
        }
    }
    let multi = matched.len() > 1;
    let into_dir = dest.ends_with('/') || multi;
    let mut out: Vec<CopyEntry> = Vec::new();
    for rel in &matched {
        let abs = context.join(rel);
        let meta = std::fs::symlink_metadata(&abs)
            .map_err(|e| Error::invalid(format!("{}: {e}", abs.display())))?;
        if meta.is_dir() {
            // A directory copies its contents into `dest`.
            out.push(CopyEntry {
                dest: join_dest(dest, ""),
                mode: mode_of(&meta),
                kind: EntryKind::Dir,
            });
            let mut inner = Vec::new();
            walk(context, rel, &ignore, &mut inner)?;
            for p in inner {
                let within = p.strip_prefix(rel).unwrap_or(&p).trim_start_matches('/');
                let within = if rel.is_empty() { p.as_str() } else { within };
                out.push(entry_for(context, &p, join_dest(dest, within))?);
            }
        } else if unpack && is_archive(rel) && meta.is_file() {
            let (sha256, size) = file_sha(&abs)?;
            out.push(CopyEntry {
                dest: join_dest(dest, ""),
                mode: 0o755,
                kind: EntryKind::Archive {
                    host: abs,
                    sha256,
                    size,
                },
            });
        } else {
            let base = rel.rsplit('/').next().unwrap_or(rel);
            let target = if into_dir {
                join_dest(dest, base)
            } else {
                dest.to_string()
            };
            out.push(entry_for(context, rel, target)?);
        }
        if out.len() > MAX_COPY_ENTRIES {
            return Err(Error::invalid(format!(
                "{verb} would copy more than {MAX_COPY_ENTRIES} files"
            )));
        }
    }
    // Later sources overwrite earlier ones at the same path.
    let mut seen = std::collections::BTreeMap::new();
    for (i, e) in out.iter().enumerate() {
        if !matches!(e.kind, EntryKind::Dir) {
            seen.insert(e.dest.clone(), i);
        }
    }
    let mut keep: Vec<CopyEntry> = out
        .into_iter()
        .enumerate()
        .filter(|(i, e)| matches!(e.kind, EntryKind::Dir) || seen.get(&e.dest) == Some(i))
        .map(|(_, e)| e)
        .collect();
    keep.sort_by(|a, b| a.dest.cmp(&b.dest));
    keep.dedup_by(|a, b| a.dest == b.dest && matches!(a.kind, EntryKind::Dir) && a.kind == b.kind);
    Ok(keep)
}

/// The entries as a tar stream rooted at `/` (paths without the leading
/// slash, owned by root, parents created by the extraction), plus the list
/// of paths to `chown` / `chmod` (NUL-separated). `Archive` entries are not
/// included (they travel as their own file).
pub(super) fn pack(entries: &[CopyEntry]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut tar = tar::Builder::new(Vec::new());
    tar.mode(tar::HeaderMode::Deterministic);
    let mut paths = Vec::new();
    for e in entries {
        let name = e.dest.trim_start_matches('/');
        if name.is_empty() {
            continue;
        }
        match &e.kind {
            EntryKind::Dir => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(e.mode);
                h.set_size(0);
                h.set_uid(0);
                h.set_gid(0);
                h.set_mtime(0);
                tar.append_data(&mut h, format!("{name}/"), std::io::empty())?;
            }
            EntryKind::File { host, sha256, size } => {
                let (now, len) = file_sha(host)?;
                if &now != sha256 || len != *size {
                    return Err(Error::invalid(format!(
                        "{} changed while the build was running",
                        host.display()
                    )));
                }
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(e.mode);
                h.set_size(*size);
                h.set_uid(0);
                h.set_gid(0);
                h.set_mtime(0);
                tar.append_data(&mut h, name, std::fs::File::open(host)?)?;
            }
            EntryKind::Symlink { target } => {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_mode(0o777);
                h.set_size(0);
                h.set_uid(0);
                h.set_gid(0);
                h.set_mtime(0);
                tar.append_link(&mut h, name, target)?;
            }
            EntryKind::Archive { .. } => continue,
        }
        paths.extend_from_slice(e.dest.as_bytes());
        paths.push(0);
    }
    Ok((tar.into_inner()?, paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let w = |p: &str, c: &str| {
            let f = d.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        };
        w("hello.txt", "hi\n");
        w("app/main.py", "print(1)\n");
        w("app/lib/util.py", "x = 1\n");
        w("app/notes.md", "n\n");
        w("docs/a.md", "a\n");
        w("docs/sub/b.md", "b\n");
        d
    }

    fn dests(e: &[CopyEntry]) -> Vec<String> {
        e.iter().map(|e| e.dest.clone()).collect()
    }

    #[test]
    fn a_file_lands_at_dest_or_in_it() {
        let d = tree();
        let e = plan_copy(Some(d.path()), &["hello.txt".into()], "/srv/app/", false).unwrap();
        assert_eq!(dests(&e), ["/srv/app/hello.txt"]);
        let e = plan_copy(Some(d.path()), &["hello.txt".into()], "/srv/h.txt", false).unwrap();
        assert_eq!(dests(&e), ["/srv/h.txt"]);
        let e = plan_copy(Some(d.path()), &["./hello.txt".into()], "/srv", false).unwrap();
        assert_eq!(dests(&e), ["/srv"]);
    }

    #[test]
    fn a_directory_copies_its_contents() {
        let d = tree();
        let e = plan_copy(Some(d.path()), &["app".into()], "/opt/app", false).unwrap();
        assert_eq!(
            dests(&e),
            [
                "/opt/app",
                "/opt/app/lib",
                "/opt/app/lib/util.py",
                "/opt/app/main.py",
                "/opt/app/notes.md"
            ]
        );
        let all = plan_copy(Some(d.path()), &[".".into()], "/w", false).unwrap();
        assert!(dests(&all).contains(&"/w/app/lib/util.py".to_string()));
        assert!(dests(&all).contains(&"/w/hello.txt".to_string()));
    }

    #[test]
    fn globs_and_several_sources() {
        let d = tree();
        let e = plan_copy(Some(d.path()), &["docs/*.md".into()], "/d/", false).unwrap();
        assert_eq!(dests(&e), ["/d/a.md"]);
        let e = plan_copy(
            Some(d.path()),
            &["hello.txt".into(), "app/main.py".into()],
            "/d",
            false,
        )
        .unwrap();
        assert_eq!(dests(&e), ["/d/hello.txt", "/d/main.py"]);
        assert!(plan_copy(Some(d.path()), &["*.nope".into()], "/d/", false).is_err());
    }

    #[test]
    fn dockerignore_is_anchored_at_the_root() {
        let d = tree();
        std::fs::write(
            d.path().join(".dockerignore"),
            "*.md\n!docs/a.md\napp/lib\n",
        )
        .unwrap();
        let e = plan_copy(Some(d.path()), &[".".into()], "/w", false).unwrap();
        let got = dests(&e);
        // `*.md` matches at the root only; `app/notes.md` stays.
        assert!(got.contains(&"/w/app/notes.md".to_string()), "{got:?}");
        assert!(got.contains(&"/w/docs/a.md".to_string()), "{got:?}");
        assert!(!got.contains(&"/w/app/lib/util.py".to_string()), "{got:?}");
        std::fs::write(d.path().join(".dockerignore"), "**/*.md\n").unwrap();
        let e = plan_copy(Some(d.path()), &[".".into()], "/w", false).unwrap();
        assert!(!dests(&e).iter().any(|p| p.ends_with(".md")));
        // An excluded literal source is not found.
        let err = plan_copy(Some(d.path()), &["docs/a.md".into()], "/w/", false).unwrap_err();
        assert!(
            err.to_string().contains("excluded by .dockerignore"),
            "{err}"
        );
    }

    #[test]
    fn sources_cannot_leave_the_context() {
        let d = tree();
        let e = plan_copy(Some(d.path()), &["../x".into()], "/w", false).unwrap_err();
        assert!(e.to_string().contains("outside the build context"), "{e}");
        assert!(plan_copy(None, &["a".into()], "/w", false).is_err());
        assert!(plan_copy(Some(d.path()), &["missing".into()], "/w", false).is_err());
    }

    #[test]
    fn content_changes_change_the_key_text() {
        let d = tree();
        let a = plan_copy(Some(d.path()), &["hello.txt".into()], "/h", false).unwrap();
        std::fs::write(d.path().join("hello.txt"), "changed\n").unwrap();
        let b = plan_copy(Some(d.path()), &["hello.txt".into()], "/h", false).unwrap();
        assert_ne!(a[0].key_text(), b[0].key_text());
        let c = plan_copy(Some(d.path()), &["hello.txt".into()], "/h", false).unwrap();
        assert_eq!(b[0].key_text(), c[0].key_text());
    }

    #[test]
    fn add_unpacks_local_archives() {
        let d = tree();
        std::fs::write(d.path().join("x.tar.gz"), b"not really").unwrap();
        let e = plan_copy(Some(d.path()), &["x.tar.gz".into()], "/opt/x", true).unwrap();
        assert!(matches!(e[0].kind, EntryKind::Archive { .. }));
        // COPY never unpacks.
        let e = plan_copy(Some(d.path()), &["x.tar.gz".into()], "/opt/x/", false).unwrap();
        assert!(matches!(e[0].kind, EntryKind::File { .. }));
    }

    #[test]
    fn packs_a_rooted_tar_owned_by_root() {
        let d = tree();
        let e = plan_copy(Some(d.path()), &["app".into()], "/opt/app", false).unwrap();
        let (tarball, paths) = pack(&e).unwrap();
        let mut ar = tar::Archive::new(&tarball[..]);
        let names: Vec<String> = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert!(names.contains(&"opt/app/main.py".to_string()), "{names:?}");
        assert!(names.contains(&"opt/app/lib/".to_string()), "{names:?}");
        let mut ar = tar::Archive::new(&tarball[..]);
        for e in ar.entries().unwrap() {
            let e = e.unwrap();
            assert_eq!(e.header().uid().unwrap(), 0);
        }
        assert!(paths.starts_with(b"/opt/app\0"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_copied_not_followed() {
        let d = tree();
        std::os::unix::fs::symlink("hello.txt", d.path().join("link")).unwrap();
        let e = plan_copy(Some(d.path()), &["link".into()], "/l", false).unwrap();
        assert_eq!(
            e[0].kind,
            EntryKind::Symlink {
                target: "hello.txt".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn modes_are_the_sources() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree();
        std::fs::set_permissions(
            d.path().join("hello.txt"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        let e = plan_copy(Some(d.path()), &["hello.txt".into()], "/h", false).unwrap();
        assert_eq!(e[0].mode, 0o750);
    }
}
