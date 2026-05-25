//! `cua-driver skills {install|update|uninstall|status|path}` —
//! agent skill-pack management.
//!
//! The install scripts intentionally do NOT touch the user's
//! `~/.claude/skills/` (etc.) directories — too invasive for an
//! `irm | iex` / `curl | sh` one-liner that the user might be running
//! just to try the binary. This verb is the opt-in path: fetch the
//! versioned skill pack from a matching GitHub release and symlink it
//! into each detected agent's skills dir.
//!
//! ## Subcommands
//!
//! - `install` — fetch + place + symlink (idempotent: re-run is a no-op).
//! - `update` — same as `install --force`: re-fetch even if local copy
//!   already exists, refreshes content.
//! - `uninstall [--all]` — remove the agent symlinks. With `--all`, also
//!   delete the local copy under `<HomeDir>/skills/cua-driver/` (and the
//!   pre-rename `cua-driver-rs/` location if present).
//! - `status` — print local install state + per-agent link state.
//! - `path` — print `<HomeDir>/skills/cua-driver` (the local copy).
//!
//! Default install drops only the host platform's deep-dive .md
//! (WINDOWS.md / MACOS.md / LINUX.md — whichever matches). Pass
//! `--all-platforms` to keep all three (useful when assisting users
//! across OSes from one machine).
//!
//! ## Fetch source
//!
//! The default fetch URL is the versioned release asset matched to the
//! binary's own version: `cua-driver-rs-v<v>-skills.tar.gz` from
//! `https://github.com/trycua/cua/releases/...`. This pins the skill
//! content to the binary release so an agent loading the doc knows
//! every example matches the daemon it'll talk to.
//!
//! `--from <tag>` lets the user pin a different release tag.
//! `--from main` fetches the latest from the `main` branch via the
//! `Skills/cua-driver/` directory (one HTTP call per file — used
//! for bleeding-edge dev validation; not the default).
//!
//! ## Agent detection
//!
//! Same four agent dirs as the Swift cua-driver installer detects:
//!
//! - Claude Code: `~/.claude/skills/`
//! - Codex:       `~/.agents/skills/`
//! - OpenClaw:    `~/.openclaw/skills/`
//! - OpenCode:    `~/.config/opencode/skills/` (macOS / Linux),
//!                `%APPDATA%\opencode\skills\` (Windows)
//!
//! Only acts on a given agent when its parent skills dir already
//! exists (i.e. the agent itself is installed). Never clobbers an
//! existing `<agent_skills>/cua-driver` link — preserves dev users'
//! hand-rolled symlinks.

use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_PACK_NAME: &str = "cua-driver";
/// Pre-rename name. The skill pack used to install as `cua-driver-rs`
/// (when the Rust port lived at `libs/cua-driver-rs/`). On install /
/// uninstall we sweep this name out of every agent skills dir and the
/// local stage so a user who had the old skill installed ends up with
/// exactly one pack, named consistently with the rest of the binary.
const LEGACY_SKILL_PACK_NAME: &str = "cua-driver-rs";
const SKILL_FILES: &[&str] = &[
    "README.md",
    "SKILL.md",
    "WINDOWS.md",
    "MACOS.md",
    "LINUX.md",
    "WEB_APPS.md",
    "RECORDING.md",
    "TESTS.md",
];

/// Per-host filter: returns the platform-specific docs that should NOT
/// land in the local stage. The skill pack ships docs for all three
/// platforms in the same tarball, but a Windows user has no need for
/// LINUX.md / MACOS.md and vice-versa. SKILL.md still references the
/// matching platform doc by name, so the LLM sees one specific deep
/// dive without two extra files of unused noise.
///
/// Override with `cua-driver skills install --all-platforms` to keep
/// the full set (useful when assisting users across OSes from one
/// machine).
fn excluded_platform_docs(all_platforms: bool) -> &'static [&'static str] {
    if all_platforms {
        return &[];
    }
    #[cfg(target_os = "windows")]
    { &["LINUX.md", "MACOS.md"] }
    #[cfg(target_os = "linux")]
    { &["WINDOWS.md", "MACOS.md"] }
    #[cfg(target_os = "macos")]
    { &["WINDOWS.md", "LINUX.md"] }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    { &[] }
}

/// True when the basename matches one of the excluded platform docs.
fn is_excluded_platform_doc(basename: &str, all_platforms: bool) -> bool {
    let excluded = excluded_platform_docs(all_platforms);
    excluded.iter().any(|f| basename.eq_ignore_ascii_case(f))
}

/// Package-home subdir name (matches `telemetry::HOME_SUBDIRECTORY`).
/// Pre-v0.2.16 this was `.cua-driver-rs`; the rename to `.cua-driver/` is
/// the same rename the binary install path went through, kept in lock-
/// step so all on-disk state lives under one dot-folder.
const HOME_SUBDIRECTORY: &str = ".cua-driver";

/// Legacy subdir name. `sweep_legacy_skill_pack` removes any skill-pack
/// artifacts that landed here before the rename.
const LEGACY_HOME_SUBDIRECTORY: &str = ".cua-driver-rs";

/// Local install path for the skill pack: `<HomeDir>/skills/cua-driver`.
fn local_skill_dir() -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(home.join("skills").join(SKILL_PACK_NAME))
}

/// `<HomeDir>` resolved from the same env override `serve.rs` uses,
/// falling back to platform conventions.
fn home_dir() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("CUA_DRIVER_RS_HOME") {
        return Ok(PathBuf::from(h));
    }
    #[cfg(windows)]
    {
        let userprofile = std::env::var("USERPROFILE")
            .map_err(|_| anyhow!("USERPROFILE not set"))?;
        return Ok(PathBuf::from(userprofile).join(HOME_SUBDIRECTORY));
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow!("HOME not set"))?;
        return Ok(PathBuf::from(home).join(HOME_SUBDIRECTORY));
    }
}

/// The pre-rename home (`~/.cua-driver-rs/`) if it exists on disk.
/// `None` when CUA_DRIVER_RS_HOME is set (the env var overrides both
/// the new and legacy defaults — caller is on their own).
fn legacy_home_dir() -> Option<PathBuf> {
    if std::env::var("CUA_DRIVER_RS_HOME").is_ok() {
        return None;
    }
    #[cfg(windows)]
    let base = std::env::var("USERPROFILE").ok()?;
    #[cfg(not(windows))]
    let base = std::env::var("HOME").ok()?;
    let p = PathBuf::from(base).join(LEGACY_HOME_SUBDIRECTORY);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Agent {
    label: &'static str,
    /// User-relative parent skills dir, expanded at runtime per OS.
    parent: AgentParent,
}

#[derive(Debug, Clone, Copy)]
enum AgentParent {
    /// `<HOME or USERPROFILE>/<segment>`.
    Home(&'static str),
    /// `<APPDATA>/<segment>` (Windows roaming app config).
    AppData(&'static str),
}

const AGENTS: &[Agent] = &[
    Agent { label: "Claude Code", parent: AgentParent::Home(".claude/skills") },
    Agent { label: "Codex",       parent: AgentParent::Home(".agents/skills") },
    Agent { label: "OpenClaw",    parent: AgentParent::Home(".openclaw/skills") },
    #[cfg(windows)]
    Agent { label: "OpenCode",    parent: AgentParent::AppData("opencode/skills") },
    #[cfg(not(windows))]
    Agent { label: "OpenCode",    parent: AgentParent::Home(".config/opencode/skills") },
];

impl Agent {
    fn parent_path(&self) -> Result<PathBuf> {
        match self.parent {
            AgentParent::Home(seg) => {
                #[cfg(windows)]
                let base = std::env::var("USERPROFILE")
                    .map_err(|_| anyhow!("USERPROFILE not set"))?;
                #[cfg(not(windows))]
                let base = std::env::var("HOME")
                    .map_err(|_| anyhow!("HOME not set"))?;
                Ok(PathBuf::from(base).join(seg.replace('/', std::path::MAIN_SEPARATOR_STR)))
            }
            AgentParent::AppData(seg) => {
                #[cfg(windows)]
                let base = std::env::var("APPDATA")
                    .map_err(|_| anyhow!("APPDATA not set"))?;
                #[cfg(not(windows))]
                let base = std::env::var("HOME")
                    .map_err(|_| anyhow!("HOME not set"))?;
                Ok(PathBuf::from(base).join(seg.replace('/', std::path::MAIN_SEPARATOR_STR)))
            }
        }
    }
    fn link_path(&self) -> Result<PathBuf> {
        Ok(self.parent_path()?.join(SKILL_PACK_NAME))
    }
}

// ── Public dispatcher ─────────────────────────────────────────────────────

pub fn run(subcommand: &str, flags: &[String]) {
    let result = match subcommand {
        "install" => install(flags, false),
        "update"  => install(flags, true),
        "uninstall" => uninstall(flags),
        "status"  => status(),
        "path"    => print_path(),
        other => {
            eprintln!("Unknown skills subcommand: {other:?}");
            eprintln!("Usage: cua-driver skills {{install|update|uninstall|status|path}}");
            std::process::exit(64);
        }
    };
    match result {
        Ok(()) => {}
        Err(e) => {
            eprintln!("cua-driver skills {subcommand}: {e}");
            std::process::exit(1);
        }
    }
}

// ── install / update ──────────────────────────────────────────────────────

fn install(flags: &[String], force: bool) -> Result<()> {
    let from_main = flags.iter().any(|f| f == "--from=main")
        || (flags.iter().any(|f| f == "--from")
            && flags.iter().zip(flags.iter().skip(1)).any(|(a, b)| a == "--from" && b == "main"));
    let force = force || flags.iter().any(|f| f == "--force");
    // `--all-platforms` opts INTO keeping LINUX.md / MACOS.md / WINDOWS.md
    // for every host. Default is host-only — only the matching platform's
    // doc is kept, the other two are skipped during fetch.
    let all_platforms = flags.iter().any(|f| f == "--all-platforms");

    // Sweep the legacy `cua-driver-rs`-named pack out FIRST so the
    // post-install state has exactly one skill pack at the new name.
    // Done before fetch so a fresh install on a previously-installed
    // machine doesn't leave orphan links pointing at a stale local dir.
    sweep_legacy_skill_pack();

    let local = local_skill_dir()?;
    let already_present = local.join("SKILL.md").exists();

    if !already_present || force {
        fetch_into(&local, from_main, all_platforms)
            .with_context(|| format!("failed to fetch skill pack to {}", local.display()))?;
        println!("✅ Skill pack at {}", local.display());
    } else {
        println!("✅ Skill pack already at {} (use `cua-driver skills update` to refresh)", local.display());
    }

    let mut linked_any = false;
    for agent in AGENTS {
        match link_agent(*agent, &local) {
            Ok(true) => linked_any = true,
            Ok(false) => {}
            Err(e) => eprintln!("  warning: failed to link {}: {e}", agent.label),
        }
    }
    if !linked_any {
        println!("(No agent skills dirs present yet — install Claude Code / Codex / OpenClaw / OpenCode then re-run.)");
    }
    Ok(())
}

/// Best-effort removal of any pre-rename skill pack. Three legacy
/// locations are swept:
///   1. `<HomeDir>/skills/cua-driver-rs/`       — old pack NAME under new home dir
///   2. `<LegacyHomeDir>/skills/cua-driver/`    — new pack NAME under old home dir
///   3. `<LegacyHomeDir>/skills/cua-driver-rs/` — old pack NAME under old home dir
/// Plus every `<agent_skills>/cua-driver-rs` symlink/junction.
///
/// Then attempts to remove the empty `<LegacyHomeDir>/skills/` and
/// `<LegacyHomeDir>/` themselves so the dot-folder doesn't linger.
/// Only when those dirs are actually empty — never blows away a
/// legacy install that still has packages/ alongside.
///
/// Runs at the start of `install` / `update` so a user who had any
/// flavour of the legacy layout installed gets cleanly migrated
/// without having to run `skills uninstall` first.
///
/// Silent on failure — this is a UX nicety, not a correctness boundary.
/// The new pack still installs even if a stale junction can't be cleaned.
fn sweep_legacy_skill_pack() {
    // (1) Old pack NAME under new home dir.
    if let Ok(home) = home_dir() {
        let legacy_local = home.join("skills").join(LEGACY_SKILL_PACK_NAME);
        if legacy_local.exists() {
            if let Err(e) = fs::remove_dir_all(&legacy_local) {
                eprintln!("  warning: could not remove legacy local pack at {}: {e}",
                    legacy_local.display());
            } else {
                println!("  cleaned up legacy local pack at {}", legacy_local.display());
            }
        }
    }
    // (2) + (3) Any pack name under the pre-rename home dir, then try
    // to remove the empty skills/ + home/ dirs themselves.
    if let Some(legacy_home) = legacy_home_dir() {
        let legacy_skills_dir = legacy_home.join("skills");
        for name in [SKILL_PACK_NAME, LEGACY_SKILL_PACK_NAME] {
            let dir = legacy_skills_dir.join(name);
            if dir.exists() {
                if let Err(e) = fs::remove_dir_all(&dir) {
                    eprintln!("  warning: could not remove legacy pack at {}: {e}",
                        dir.display());
                } else {
                    println!("  cleaned up legacy local pack at {}", dir.display());
                }
            }
        }
        // remove_dir refuses to delete non-empty dirs — safe to ignore
        // errors here, and intentional: a legacy install that still has
        // packages/ alongside the (now-emptied) skills/ keeps its
        // dot-folder.
        let _ = fs::remove_dir(&legacy_skills_dir);
        let _ = fs::remove_dir(&legacy_home);
    }
    // Agent links named `<parent>/cua-driver-rs`.
    for agent in AGENTS {
        let parent = match agent.parent_path() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let legacy_link = parent.join(LEGACY_SKILL_PACK_NAME);
        if !parent.exists() || !legacy_link.symlink_metadata().is_ok() {
            continue;
        }
        if !is_symlink_or_junction(&legacy_link) {
            // Real directory — don't clobber user-managed content.
            continue;
        }
        if let Err(e) = remove_link(&legacy_link) {
            eprintln!("  warning: could not remove legacy {} link at {}: {e}",
                agent.label, legacy_link.display());
        } else {
            println!("  cleaned up legacy {} link at {}", agent.label, legacy_link.display());
        }
    }
}

/// Returns `Ok(true)` when a new link was created, `Ok(false)` when
/// skipped (parent dir missing, link already there, etc.).
fn link_agent(agent: Agent, local_skill_dir: &Path) -> Result<bool> {
    let parent = agent.parent_path()?;
    if !parent.exists() {
        return Ok(false);
    }
    let link = agent.link_path()?;
    // Four states for `link`:
    //   1. doesn't exist at all                 → create
    //   2. exists + resolves                    → already linked (skip)
    //   3. exists as link/junction but target dangling → remove + recreate
    //   4. exists as a real directory           → user-managed, leave alone
    //
    // `Path::exists()` follows symlinks, so it returns false for a
    // dangling link even though `symlink_metadata` succeeds — that's
    // the signature of case 3. We then check `is_symlink_or_junction`
    // before deleting, so we never touch a real user directory.
    let has_metadata = link.symlink_metadata().is_ok();
    let resolves    = link.exists();
    if has_metadata && resolves {
        println!("  {} skill link already exists at {} (skipping)", agent.label, link.display());
        return Ok(false);
    }
    if has_metadata && !resolves && is_symlink_or_junction(&link) {
        // Dangling link/junction — target was removed (typical after
        // sweep_legacy_skill_pack cleaned a pre-rename pack out from
        // under it). Remove + recreate pointing at the new target.
        if let Err(e) = remove_link(&link) {
            eprintln!("  warning: could not remove stale {} link at {}: {e}",
                agent.label, link.display());
            return Ok(false);
        }
        println!("  cleaned up stale {} link at {}", agent.label, link.display());
    }
    make_dir_symlink(local_skill_dir, &link)
        .with_context(|| format!("symlink {} -> {}", link.display(), local_skill_dir.display()))?;
    println!("  ✅ linked {} skill at {}", agent.label, link.display());
    Ok(true)
}

#[cfg(windows)]
fn make_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    // NTFS directory junction: works without admin/Developer Mode,
    // unlike `std::os::windows::fs::symlink_dir`. Shell out to cmd's
    // `mklink /J` — the canonical way to create a junction from a
    // standard user token.
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("mklink /J exited with {:?}", status.code());
    }
    Ok(())
}

#[cfg(not(windows))]
fn make_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

// ── fetch ──────────────────────────────────────────────────────────────────

fn fetch_into(dest: &Path, from_main: bool, all_platforms: bool) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    // Wipe stale content so an update is a clean replace, not a merge.
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;

    if from_main {
        // Per-file raw GitHub fetch — used for bleeding-edge dev validation.
        let base = "https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/rust/Skills/cua-driver";
        for f in SKILL_FILES {
            if is_excluded_platform_doc(f, all_platforms) {
                continue;
            }
            let url = format!("{base}/{f}");
            let body = http_get_text(&url)
                .with_context(|| format!("GET {url}"))?;
            fs::write(dest.join(f), body)?;
        }
        return Ok(());
    }

    // Versioned release asset.
    let version = env!("CARGO_PKG_VERSION");
    let url = format!(
        "https://github.com/trycua/cua/releases/download/cua-driver-rs-v{version}/cua-driver-rs-v{version}-skills.tar.gz"
    );
    let bytes = http_get_bytes(&url)
        .with_context(|| format!("GET {url}"))?;
    extract_tar_gz(&bytes, dest, all_platforms)?;
    Ok(())
}

fn http_get_text(url: &str) -> Result<String> {
    let resp = ureq::get(url).call()
        .map_err(|e| anyhow!("HTTP error fetching {url}: {e}"))?;
    if resp.status() != 200 {
        bail!("HTTP {} fetching {url}", resp.status());
    }
    Ok(resp.into_body().read_to_string()?)
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url).call()
        .map_err(|e| anyhow!("HTTP error fetching {url}: {e}"))?;
    if resp.status() != 200 {
        bail!("HTTP {} fetching {url}", resp.status());
    }
    let mut body = resp.into_body();
    let mut buf = Vec::new();
    body.as_reader().read_to_end(&mut buf)?;
    Ok(buf)
}

fn extract_tar_gz(bytes: &[u8], dest: &Path, all_platforms: bool) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    // Tarball shape across versions:
    //   v0.2.18 and earlier: cua-driver-rs-v<v>-skills/cua-driver-rs/<file>
    //   v0.2.19 (briefly):   cua-driver-rs-v<v>-skills/cua-driver/<file>
    //   v0.2.20+:            cua-driver-rs-v<v>-skills/<file>     (CD workflow now flattens)
    //
    // Strip the outer staging dir always; additionally strip a SECOND
    // wrapping dir IF it's named `cua-driver` or `cua-driver-rs` — that
    // covers the legacy double-wrap without losing files in the
    // (now-canonical) single-wrap shape.
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut components = path.components();
        if components.next().is_none() {
            continue; // empty entry
        }
        // Peek the next component; if it's an unambiguous skill-pack
        // wrapper, drop it too.
        let mut peek = components.clone();
        if let Some(next) = peek.next() {
            let name = next.as_os_str();
            if name == "cua-driver" || name == "cua-driver-rs" {
                components.next();
            }
        }
        let stripped: PathBuf = components.collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        // Per-host filter: skip the other platforms' .md files unless
        // the user opted into the full set with --all-platforms.
        if let Some(basename) = stripped.file_name().and_then(|s| s.to_str()) {
            if is_excluded_platform_doc(basename, all_platforms) {
                continue;
            }
        }
        let out = dest.join(&stripped);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}

use std::io::Read;

// ── uninstall ──────────────────────────────────────────────────────────────

fn uninstall(flags: &[String]) -> Result<()> {
    let remove_local = flags.iter().any(|f| f == "--all");
    let mut removed_any = false;
    // Try BOTH the current name and the legacy `cua-driver-rs` name so a
    // user who installed under the old name and then `skills uninstall`s
    // ends up clean. Same symlink/junction safety check applies to each.
    for name in [SKILL_PACK_NAME, LEGACY_SKILL_PACK_NAME] {
        for agent in AGENTS {
            let parent = match agent.parent_path() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let link = parent.join(name);
            if link.symlink_metadata().is_ok() {
                // Only remove if it's a symlink/junction we manage. If a
                // user replaced it with a real dir, leave it alone.
                if is_symlink_or_junction(&link) {
                    remove_link(&link)?;
                    println!("  ✅ removed {} link at {}", agent.label, link.display());
                    removed_any = true;
                } else {
                    println!("  {} link at {} is not a symlink/junction; leaving alone", agent.label, link.display());
                }
            }
        }
    }
    if remove_local {
        // Local stage at the current name + any legacy stage from before
        // the rename. Both are owned by the installer; safe to delete.
        if let Ok(home) = home_dir() {
            for name in [SKILL_PACK_NAME, LEGACY_SKILL_PACK_NAME] {
                let local = home.join("skills").join(name);
                if local.exists() {
                    fs::remove_dir_all(&local)?;
                    println!("  ✅ removed local skill pack at {}", local.display());
                }
            }
        }
        // Also clean up any pre-rename home (`~/.cua-driver-rs/`) that
        // might still hold a skill pack from before the
        // `.cua-driver-rs/` → `.cua-driver/` migration. Remove the empty
        // skills/ and home/ dirs only if nothing else lives under them.
        if let Some(legacy_home) = legacy_home_dir() {
            let legacy_skills_dir = legacy_home.join("skills");
            for name in [SKILL_PACK_NAME, LEGACY_SKILL_PACK_NAME] {
                let local = legacy_skills_dir.join(name);
                if local.exists() {
                    fs::remove_dir_all(&local)?;
                    println!("  ✅ removed legacy local skill pack at {}", local.display());
                }
            }
            let _ = fs::remove_dir(&legacy_skills_dir);
            let _ = fs::remove_dir(&legacy_home);
        }
    }
    if !removed_any {
        println!("(no agent skill links found)");
    }
    Ok(())
}

#[cfg(windows)]
fn is_symlink_or_junction(p: &Path) -> bool {
    if let Ok(md) = p.symlink_metadata() {
        // Both symlinks and junctions have the reparse-point attribute.
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    false
}

#[cfg(not(windows))]
fn is_symlink_or_junction(p: &Path) -> bool {
    p.symlink_metadata().map(|md| md.file_type().is_symlink()).unwrap_or(false)
}

#[cfg(windows)]
fn remove_link(p: &Path) -> Result<()> {
    // For a junction (which appears as a directory) we use rmdir;
    // for a file symlink std::fs::remove_file would work, but
    // junctions are dir-shaped so std::fs::remove_dir is correct.
    fs::remove_dir(p).map_err(Into::into)
}

#[cfg(not(windows))]
fn remove_link(p: &Path) -> Result<()> {
    fs::remove_file(p).map_err(Into::into)
}

// ── status ─────────────────────────────────────────────────────────────────

fn status() -> Result<()> {
    let local = local_skill_dir()?;
    if local.exists() && local.join("SKILL.md").exists() {
        println!("Local skill pack: {} ✅", local.display());
    } else {
        println!("Local skill pack: not installed (`cua-driver skills install` to fetch)");
    }
    println!();
    println!("Agent links:");
    for agent in AGENTS {
        let parent = match agent.parent_path() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let link = parent.join(SKILL_PACK_NAME);
        let parent_exists = parent.exists();
        if !parent_exists {
            println!("  {} — agent dir not present ({})", agent.label, parent.display());
            continue;
        }
        if !link.exists() && link.symlink_metadata().is_err() {
            println!("  {} — not linked ({})", agent.label, link.display());
        } else if is_symlink_or_junction(&link) {
            let target = fs::read_link(&link).ok();
            match target {
                Some(t) => println!("  {} — ✅ linked: {} → {}", agent.label, link.display(), t.display()),
                None    => println!("  {} — ✅ linked: {}", agent.label, link.display()),
            }
        } else {
            println!("  {} — non-symlink path at {} (left alone)", agent.label, link.display());
        }
    }
    Ok(())
}

fn print_path() -> Result<()> {
    let local = local_skill_dir()?;
    println!("{}", local.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_tar_gz;
    use tempfile::tempdir;

    /// Build a gzipped tarball with the entries given as
    /// `(path, contents)` pairs. Returns the raw `.tar.gz` bytes.
    fn build_tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz_buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut gz_buf, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            for (path, contents) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(contents.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, path, &contents[..]).unwrap();
            }
            tar.finish().unwrap();
        }
        gz_buf
    }

    #[test]
    fn extract_flat_tarball_v_0_2_20_plus() {
        // Post-fix shape: one wrapper dir, files directly under it.
        //   cua-driver-rs-v0.2.20-skills/SKILL.md
        //   cua-driver-rs-v0.2.20-skills/WINDOWS.md
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/SKILL.md",   b"flat-skill"),
            ("cua-driver-rs-v0.2.20-skills/WINDOWS.md", b"flat-win"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path(), true).unwrap();

        let s = std::fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert_eq!(s, "flat-skill");
        let w = std::fs::read_to_string(dest.path().join("WINDOWS.md")).unwrap();
        assert_eq!(w, "flat-win");
        // No nested wrapper dir created.
        assert!(!dest.path().join("cua-driver").exists());
        assert!(!dest.path().join("cua-driver-rs").exists());
    }

    #[test]
    fn extract_legacy_tarball_v_0_2_18_double_wrap_old_name() {
        // v0.2.18 and earlier shape:
        //   cua-driver-rs-v0.2.18-skills/cua-driver-rs/SKILL.md
        // Both wrappers must be stripped or the user ends up with a
        // nested cua-driver-rs/ dir (the bug this fixes).
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.18-skills/cua-driver-rs/SKILL.md",   b"legacy-skill"),
            ("cua-driver-rs-v0.2.18-skills/cua-driver-rs/WINDOWS.md", b"legacy-win"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path(), true).unwrap();

        let s = std::fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert_eq!(s, "legacy-skill");
        let w = std::fs::read_to_string(dest.path().join("WINDOWS.md")).unwrap();
        assert_eq!(w, "legacy-win");
        assert!(!dest.path().join("cua-driver-rs").exists(),
            "nested cua-driver-rs/ dir should have been stripped");
    }

    #[test]
    fn extract_double_wrap_new_name_also_strips() {
        // Interim shape (v0.2.19, briefly): inner dir is `cua-driver/`.
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.19-skills/cua-driver/SKILL.md", b"interim-skill"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path(), true).unwrap();
        let s = std::fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert_eq!(s, "interim-skill");
        assert!(!dest.path().join("cua-driver").exists());
    }

    #[test]
    fn extract_preserves_subdirs_inside_pack() {
        // If a future skill pack adds a real subdir (e.g. `examples/`),
        // it must NOT be stripped — only the unambiguous pack-name
        // wrappers are.
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/examples/click.md", b"sample"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path(), true).unwrap();
        let s = std::fs::read_to_string(dest.path().join("examples/click.md")).unwrap();
        assert_eq!(s, "sample");
    }

    #[test]
    fn extract_per_host_filter_drops_other_platform_docs() {
        // The skill pack ships docs for all three platforms but a given
        // host only needs one. all_platforms=false means the other two
        // platform docs get skipped during extraction. README + SKILL +
        // platform-agnostic docs are always kept.
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/README.md",   b"r"),
            ("cua-driver-rs-v0.2.20-skills/SKILL.md",    b"s"),
            ("cua-driver-rs-v0.2.20-skills/WINDOWS.md",  b"w"),
            ("cua-driver-rs-v0.2.20-skills/MACOS.md",    b"m"),
            ("cua-driver-rs-v0.2.20-skills/LINUX.md",    b"l"),
            ("cua-driver-rs-v0.2.20-skills/RECORDING.md",b"R"),
            ("cua-driver-rs-v0.2.20-skills/WEB_APPS.md", b"W"),
            ("cua-driver-rs-v0.2.20-skills/TESTS.md",    b"T"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path(), /*all_platforms=*/ false).unwrap();
        // README + SKILL + cross-platform docs ALWAYS present.
        for f in ["README.md", "SKILL.md", "RECORDING.md", "WEB_APPS.md", "TESTS.md"] {
            assert!(dest.path().join(f).exists(),
                "{f} should be present after per-host extraction");
        }
        // Exactly one platform doc should land — whichever matches this
        // test's compile target. The other two must be absent.
        #[cfg(target_os = "windows")]
        let expected_present = "WINDOWS.md";
        #[cfg(target_os = "linux")]
        let expected_present = "LINUX.md";
        #[cfg(target_os = "macos")]
        let expected_present = "MACOS.md";
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        let expected_present = "";
        for f in ["WINDOWS.md", "MACOS.md", "LINUX.md"] {
            let exists = dest.path().join(f).exists();
            if f == expected_present {
                assert!(exists, "{f} (host doc) should be present");
            } else if !expected_present.is_empty() {
                assert!(!exists, "{f} (non-host doc) should NOT be present");
            }
        }
    }

    #[test]
    fn extract_all_platforms_flag_keeps_every_platform_doc() {
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/WINDOWS.md", b"w"),
            ("cua-driver-rs-v0.2.20-skills/MACOS.md",   b"m"),
            ("cua-driver-rs-v0.2.20-skills/LINUX.md",   b"l"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path(), /*all_platforms=*/ true).unwrap();
        for f in ["WINDOWS.md", "MACOS.md", "LINUX.md"] {
            assert!(dest.path().join(f).exists(),
                "--all-platforms should keep {f}");
        }
    }
}
