//! The default cua skills, bundled from `libs/cua/skills/` at build time.
//! Each is an Agent Skills folder (`<name>/SKILL.md` with `name` and
//! `description` front matter, plus optional reference files).

use crate::{Error, Result, fsutil};
use cua_home::channel::CuaChannel;
use include_dir::{Dir, include_dir};
use sha2::{Digest, Sha256};
use std::path::Path;

static BUNDLED: Dir<'_> = include_dir!("$OUT_DIR/skills");

/// One bundled skill.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Skill {
    /// Folder name (and front matter `name`) it is installed under: the
    /// bundled name for Stable (`cua-spaces`), the channel's for Local
    /// (`cua-spaces-local`, [`CuaChannel::skill_install_name`]).
    pub name: String,
    /// The bundled folder it comes from (`cua-spaces`): [`write_for`] and
    /// [`files`] take this name, whatever the channel installs it as.
    #[serde(skip)]
    pub source: String,
    /// Front matter `description`.
    pub description: String,
    /// Front matter `version`, or the cua version.
    pub version: String,
    /// Tree hash of the bundled files (same scheme as `fsutil::hash_dir`).
    pub hash: String,
    /// Number of files.
    pub files: usize,
}

/// Every bundled skill, sorted by name.
pub fn bundled() -> Vec<Skill> {
    let mut out: Vec<Skill> = BUNDLED
        .dirs()
        .filter_map(|d| {
            let name = d.path().file_name()?.to_string_lossy().to_string();
            let md = d.get_file(d.path().join("SKILL.md"))?.contents_utf8()?;
            let (fm_name, description, version) = front_matter(md)?;
            if fm_name != name {
                return None;
            }
            let files = files_of(d);
            Some(Skill {
                source: name.clone(),
                name,
                description,
                version: version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
                hash: tree_hash(d, &files),
                files: files.len(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The bundled skill `name`.
pub fn get(name: &str) -> Option<Skill> {
    bundled().into_iter().find(|s| s.name == name)
}

/// The note Local's copy of a skill opens with.
const LOCAL_NOTE: &str = "> This is the Cua Spaces Local copy of this skill. Where it says to run `cua`, run `cua-local` instead, and the MCP server is `cua-local`. The released Cua Spaces and its skills are not touched.";

/// Every bundled skill as `channel` installs it, sorted by name. Stable's
/// are the bundled ones. Local's sit beside them under `-local` names, with
/// that name in their `SKILL.md`, a note that says to use `cua-local`, and
/// the hash of that content: installing them never replaces Stable's.
pub fn bundled_for(channel: CuaChannel) -> Vec<Skill> {
    let mut out = bundled();
    if channel == CuaChannel::Stable {
        return out;
    }
    for s in &mut out {
        let d = BUNDLED.get_dir(&s.source).expect("a bundled skill");
        let name = channel.skill_install_name(&s.source);
        s.hash = hash_of(&rendered(channel, &s.source, d));
        s.name = name;
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A skill's files as `channel` installs them: (relative path, contents).
/// For Stable the bundled bytes; for Local `SKILL.md` carries the Local name
/// and note ([`localize`]).
fn rendered(channel: CuaChannel, source: &str, d: &Dir<'_>) -> Vec<(String, Vec<u8>)> {
    files_of(d)
        .into_iter()
        .map(|f| {
            let rel = rel_path(d, f);
            let bytes = if channel != CuaChannel::Stable && rel == "SKILL.md" {
                match f.contents_utf8() {
                    Some(md) => localize(md, &channel.skill_install_name(source)).into_bytes(),
                    None => f.contents().to_vec(),
                }
            } else {
                f.contents().to_vec()
            };
            (rel, bytes)
        })
        .collect()
}

/// `md` (a `SKILL.md`) for Local: the front matter's `name` becomes
/// `install_name` and the note follows the front matter.
fn localize(md: &str, install_name: &str) -> String {
    let Some(rest) = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))
    else {
        return md.to_string();
    };
    let open = &md[..md.len() - rest.len()];
    let Some(end) = rest.find("\n---") else {
        return md.to_string();
    };
    let (fm, closing) = rest.split_at(end);
    let mut renamed = false;
    let fm: Vec<String> = fm
        .split('\n')
        .map(|line| {
            let bare = line.trim_end_matches('\r');
            if !renamed && bare.starts_with("name:") {
                renamed = true;
                let cr = &line[bare.len()..];
                format!("name: {install_name}{cr}")
            } else {
                line.to_string()
            }
        })
        .collect();
    // `closing` is "\n---" and the rest of that line; the body follows it.
    let line_end = closing[1..]
        .find('\n')
        .map(|i| i + 2)
        .unwrap_or(closing.len());
    let (closing, body) = closing.split_at(line_end);
    format!("{open}{}{closing}\n{LOCAL_NOTE}\n\n{body}", fm.join("\n"))
}

/// The tree hash of `files` (the scheme of `fsutil::hash_dir`).
fn hash_of(files: &[(String, Vec<u8>)]) -> String {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|(rel, bytes)| (rel.clone(), fsutil::sha256(bytes)))
        .collect();
    entries.sort();
    let mut h = Sha256::new();
    for (rel, sum) in entries {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(sum.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

fn rel_path(d: &Dir<'_>, f: &include_dir::File<'_>) -> String {
    f.path()
        .strip_prefix(d.path())
        .unwrap_or(f.path())
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Writes the bundled skill `source` into `dest` as `channel` installs it
/// (see [`bundled_for`]); otherwise as [`write_to`].
pub fn write_for(channel: CuaChannel, source: &str, dest: &Path) -> Result<()> {
    let d = BUNDLED
        .get_dir(source)
        .ok_or_else(|| Error::InvalidArgument(format!("unknown skill {source:?}")))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    }
    for (rel, bytes) in rendered(channel, source, d) {
        let p = dest.join(&rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        std::fs::write(&p, bytes).map_err(|e| Error::io(&p, e))?;
    }
    Ok(())
}

/// Writes skill `name` into `dest` (created; files cua does not ship are
/// removed first so an update never leaves stale references behind).
pub fn write_to(name: &str, dest: &Path) -> Result<()> {
    let d = BUNDLED
        .get_dir(name)
        .ok_or_else(|| Error::InvalidArgument(format!("unknown skill {name:?}")))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    }
    for f in files_of(d) {
        let rel = f.path().strip_prefix(d.path()).unwrap_or(f.path());
        let p = dest.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        std::fs::write(&p, f.contents()).map_err(|e| Error::io(&p, e))?;
    }
    Ok(())
}

/// The files of bundled skill `name` as (`/`-separated path relative to the
/// skill folder, contents): what [`write_to`] writes, for writers that are
/// not a local filesystem (a sandbox's guest filesystem).
pub fn files(name: &str) -> Result<Vec<(String, &'static [u8])>> {
    let d = BUNDLED
        .get_dir(name)
        .ok_or_else(|| Error::InvalidArgument(format!("unknown skill {name:?}")))?;
    Ok(files_of(d)
        .into_iter()
        .map(|f| {
            let rel = f
                .path()
                .strip_prefix(d.path())
                .unwrap_or(f.path())
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            (rel, f.contents())
        })
        .collect())
}

fn files_of<'a>(d: &'a Dir<'a>) -> Vec<&'a include_dir::File<'a>> {
    let mut out = Vec::new();
    fn walk<'a>(d: &'a Dir<'a>, out: &mut Vec<&'a include_dir::File<'a>>, depth: usize) {
        if depth > 16 {
            return;
        }
        out.extend(d.files());
        for sub in d.dirs() {
            walk(sub, out, depth + 1);
        }
    }
    walk(d, &mut out, 0);
    out
}

fn tree_hash(d: &Dir<'_>, files: &[&include_dir::File<'_>]) -> String {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            let rel = f
                .path()
                .strip_prefix(d.path())
                .unwrap_or(f.path())
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            (rel, fsutil::sha256(f.contents()))
        })
        .collect();
    entries.sort();
    let mut h = Sha256::new();
    for (rel, sum) in entries {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(sum.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

/// `name`, `description` (folded `>-` blocks joined) and `version` from
/// SKILL.md front matter.
pub fn front_matter(md: &str) -> Option<(String, String, Option<String>)> {
    let rest = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let mut name = None;
    let mut description = None;
    let mut version = None;
    let mut lines = fm.lines().peekable();
    while let Some(l) = lines.next() {
        if l.starts_with(' ') || l.starts_with('\t') {
            continue;
        }
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        // Drop a trailing YAML comment (`version: 1.2 # x-release-please`).
        let v = v.split(" #").next().unwrap_or("").trim();
        let value = if matches!(v, ">-" | ">" | "|" | "|-") {
            let mut parts = Vec::new();
            while let Some(n) = lines.peek() {
                if n.starts_with(' ') || n.starts_with('\t') || n.is_empty() {
                    parts.push(n.trim().to_string());
                    lines.next();
                } else {
                    break;
                }
            }
            parts.retain(|p| !p.is_empty());
            parts.join(" ")
        } else {
            v.trim_matches('"').trim_matches('\'').to_string()
        };
        match k.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            "version" => version = Some(value),
            _ => {}
        }
    }
    Some((name?, description?, version.filter(|v| !v.is_empty())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_skills_are_bundled_and_well_formed() {
        let names: Vec<String> = bundled().into_iter().map(|s| s.name).collect();
        for want in [
            "cua-driver",
            "cua-sandboxes",
            "cua-spaces",
            "cua-volume",
            "gui-automation",
        ] {
            assert!(
                names.iter().any(|n| n == want),
                "{want} missing from {names:?}"
            );
        }
        for s in bundled() {
            assert!(!s.description.is_empty(), "{}", s.name);
            assert!(
                s.description.len() <= 1024,
                "{} description too long",
                s.name
            );
            assert!(s.files >= 1);
        }
    }

    #[test]
    fn our_skill_texts_have_no_em_dashes() {
        for name in ["cua-sandboxes", "cua-spaces", "cua-volume"] {
            let d = BUNDLED.get_dir(name).unwrap();
            for f in files_of(d) {
                let t = f.contents_utf8().unwrap();
                assert!(
                    !t.contains('\u{2014}'),
                    "{} has an em dash",
                    f.path().display()
                );
            }
        }
    }

    #[test]
    fn written_tree_hashes_like_the_bundle() {
        let d = tempfile::tempdir().unwrap();
        for s in bundled() {
            let dest = d.path().join(&s.name);
            std::fs::create_dir_all(dest.join("stale")).unwrap();
            std::fs::write(dest.join("stale/old.md"), "x").unwrap();
            write_to(&s.name, &dest).unwrap();
            assert!(!dest.join("stale").exists());
            assert_eq!(fsutil::hash_dir(&dest).unwrap(), s.hash, "{}", s.name);
        }
        assert!(write_to("nope", &d.path().join("nope")).is_err());
    }

    #[test]
    fn local_copies_have_names_of_their_own_and_hash_like_what_is_written() {
        let stable = bundled_for(CuaChannel::Stable);
        assert_eq!(stable, bundled());
        let local = bundled_for(CuaChannel::Local);
        assert_eq!(local.len(), stable.len());
        let d = tempfile::tempdir().unwrap();
        for (l, s) in local.iter().zip(&stable) {
            assert_eq!(l.name, format!("{}-local", s.name));
            assert_eq!(l.source, s.name);
            assert_ne!(l.hash, s.hash, "{}", l.name);
            // Written under the Local name, with that name in SKILL.md, and
            // the tree hashes like the skill says (so it is "not edited").
            let dest = d.path().join(&l.name);
            write_for(CuaChannel::Local, &l.source, &dest).unwrap();
            assert_eq!(fsutil::hash_dir(&dest).unwrap(), l.hash, "{}", l.name);
            let md = std::fs::read_to_string(dest.join("SKILL.md")).unwrap();
            let (name, description, _) = front_matter(&md).unwrap();
            assert_eq!(name, l.name);
            assert_eq!(description, l.description);
            assert!(md.contains(LOCAL_NOTE) && !md.contains('\u{2014}'), "{md}");
            // Stable's copy is the bundled bytes, untouched.
            let stable_dest = d.path().join(&s.name);
            write_for(CuaChannel::Stable, &s.source, &stable_dest).unwrap();
            assert_eq!(fsutil::hash_dir(&stable_dest).unwrap(), s.hash);
            // Every other file is shared byte for byte.
            assert_eq!(l.files, s.files);
        }
    }

    #[test]
    fn localize_keeps_line_endings_and_the_rest_of_the_front_matter() {
        let md = "---\r\nname: a\r\ndescription: x\r\n---\r\n# A\r\n";
        let out = localize(md, "a-local");
        assert!(
            out.starts_with("---\r\nname: a-local\r\ndescription: x\r\n---\r\n"),
            "{out:?}"
        );
        assert!(
            out.ends_with("# A\r\n") && out.contains(LOCAL_NOTE),
            "{out:?}"
        );
        assert_eq!(front_matter(&out).unwrap().0, "a-local");
        // No front matter: left as it is.
        assert_eq!(localize("# plain", "x"), "# plain");
    }

    #[test]
    fn front_matter_handles_folded_descriptions_and_comments() {
        let md = "---\nname: a\ndescription: >-\n  one\n  two\nversion: 1.2.3 # x-release\nmetadata:\n  k: v\n---\nbody";
        assert_eq!(
            front_matter(md).unwrap(),
            ("a".into(), "one two".into(), Some("1.2.3".into()))
        );
        assert!(front_matter("no front matter").is_none());
    }
}
