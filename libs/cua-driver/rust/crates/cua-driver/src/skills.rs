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
//!   delete the local copy under `<HomeDir>/skills/cua-driver-rs/`.
//! - `status` — print local install state + per-agent link state.
//! - `path` — print `<HomeDir>/skills/cua-driver-rs` (the local copy).
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
//! `Skills/cua-driver-rs/` directory (one HTTP call per file — used
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
//! existing `<agent_skills>/cua-driver-rs` link — preserves dev users'
//! hand-rolled symlinks.

use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_PACK_NAME: &str = "cua-driver-rs";
const SKILL_FILES: &[&str] = &[
    "README.md",
    "SKILL.md",
    "WINDOWS.md",
    "LINUX.md",
    "WEB_APPS.md",
    "RECORDING.md",
    "TESTS.md",
];

/// Local install path for the skill pack: `<HomeDir>/skills/cua-driver-rs`.
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
        return Ok(PathBuf::from(userprofile).join(".cua-driver-rs"));
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow!("HOME not set"))?;
        return Ok(PathBuf::from(home).join(".cua-driver-rs"));
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

    let local = local_skill_dir()?;
    let already_present = local.join("SKILL.md").exists();

    if !already_present || force {
        fetch_into(&local, from_main)
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

/// Returns `Ok(true)` when a new link was created, `Ok(false)` when
/// skipped (parent dir missing, link already there, etc.).
fn link_agent(agent: Agent, local_skill_dir: &Path) -> Result<bool> {
    let parent = agent.parent_path()?;
    if !parent.exists() {
        return Ok(false);
    }
    let link = agent.link_path()?;
    if link.exists() || link.symlink_metadata().is_ok() {
        println!("  {} skill link already exists at {} (skipping)", agent.label, link.display());
        return Ok(false);
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

fn fetch_into(dest: &Path, from_main: bool) -> Result<()> {
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
        let base = "https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/Skills/cua-driver-rs";
        for f in SKILL_FILES {
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
    extract_tar_gz(&bytes, dest)?;
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

fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    // The tarball contains a `cua-driver-rs/` top-level dir matching the
    // pack name. Strip it during extraction so the .md files land
    // directly in `dest` (which is itself `<HomeDir>/skills/cua-driver-rs`).
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut components = path.components();
        if components.next().is_none() {
            continue; // empty entry
        }
        let stripped: PathBuf = components.collect();
        if stripped.as_os_str().is_empty() {
            continue;
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
    for agent in AGENTS {
        let link = match agent.link_path() {
            Ok(p) => p,
            Err(_) => continue,
        };
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
    if remove_local {
        let local = local_skill_dir()?;
        if local.exists() {
            fs::remove_dir_all(&local)?;
            println!("  ✅ removed local skill pack at {}", local.display());
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
