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
//! `--from main` first resolves the branch to an immutable commit and
//! fetches every file from that commit. `--from local --source <dir>`
//! copies a source checkout explicitly. Every source is staged, verified
//! against `skill-pack.json`, and activated with rollback protection.
//!
//! ## Agent detection
//!
//! Same agent dirs as the Swift cua-driver installer detects:
//!
//! - Claude Code: `~/.claude/skills/`
//! - Codex:       `~/.agents/skills/`
//! - OpenClaw:    `~/.openclaw/skills/`
//! - OpenCode:    `~/.config/opencode/skills/` (macOS / Linux),
//!                `%APPDATA%\opencode\skills\` (Windows)
//! - Antigravity: `~/.gemini/skills/` — shared between Antigravity CLI
//!                (`agy`) and Antigravity IDE; same dir Google Gemini CLI
//!                used before the May-2026 transition, so existing
//!                installs migrate forward unchanged.
//! - Hermes:      `~/.hermes/skills/` — NousResearch/hermes-agent.
//!                The user-level skill space Hermes resolves at agent
//!                load time (separate from the repo-bundled
//!                `hermes-agent/skills/` tree, which is read-only and
//!                version-controlled). Hermes' own `computer-use`
//!                skill teaches its wrapper vocabulary; the
//!                cua-driver pack provides the platform deep dives.
//!
//! Only acts on a given agent when its parent skills dir already
//! exists (i.e. the agent itself is installed). Never clobbers an
//! existing `<agent_skills>/cua-driver` link — preserves dev users'
//! hand-rolled symlinks.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const SKILL_PACK_NAME: &str = "cua-driver";
const MANIFEST_FILE: &str = "skill-pack.json";
const MANIFEST_SCHEMA_VERSION: u32 = 1;
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
    "BROWSER.md",
    "RECORDING.md",
    "EMBEDDING.md",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SkillPackManifest {
    schema_version: u32,
    skill_version: String,
    compatible_driver_version: String,
    source: SkillPackSource,
    files: Vec<SkillPackFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SkillPackSource {
    kind: SourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_commit: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum SourceKind {
    Release,
    Main,
    Local,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Release => "release",
            Self::Main => "main",
            Self::Local => "local",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SkillPackFile {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallSource {
    Release,
    Main,
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallOptions {
    source: InstallSource,
    force: bool,
    all_platforms: bool,
    git_commit: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct IntegrityReport {
    missing: Vec<String>,
    extra: Vec<String>,
    modified: Vec<String>,
    manifest_error: Option<String>,
}

impl IntegrityReport {
    fn is_valid(&self) -> bool {
        self.manifest_error.is_none()
            && self.missing.is_empty()
            && self.extra.is_empty()
            && self.modified.is_empty()
    }
}

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
    {
        &["LINUX.md", "MACOS.md"]
    }
    #[cfg(target_os = "linux")]
    {
        &["WINDOWS.md", "MACOS.md"]
    }
    #[cfg(target_os = "macos")]
    {
        &["WINDOWS.md", "LINUX.md"]
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        &[]
    }
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
        let userprofile =
            std::env::var("USERPROFILE").map_err(|_| anyhow!("USERPROFILE not set"))?;
        return Ok(PathBuf::from(userprofile).join(HOME_SUBDIRECTORY));
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
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
    ///
    /// `#[allow(dead_code)]`: constructed only inside `#[cfg(windows)]`
    /// AGENTS entries (OpenCode on Windows reads from `%APPDATA%`) and
    /// matched only inside `#[cfg(windows)]` arms of `parent_path`. The
    /// enum variant itself sits in cross-platform code so rustc's
    /// post-cfg-strip dead-code pass flags it on macOS/Linux even though
    /// the Windows build uses it.
    #[allow(dead_code)]
    AppData(&'static str),
}

const AGENTS: &[Agent] = &[
    Agent {
        label: "Claude Code",
        parent: AgentParent::Home(".claude/skills"),
    },
    Agent {
        label: "Codex",
        parent: AgentParent::Home(".agents/skills"),
    },
    Agent {
        label: "OpenClaw",
        parent: AgentParent::Home(".openclaw/skills"),
    },
    #[cfg(windows)]
    Agent {
        label: "OpenCode",
        parent: AgentParent::AppData("opencode/skills"),
    },
    #[cfg(not(windows))]
    Agent {
        label: "OpenCode",
        parent: AgentParent::Home(".config/opencode/skills"),
    },
    // Antigravity CLI + Antigravity IDE share the `.gemini/skills/` dir
    // (the same path Gemini CLI used pre-May-2026). Registering the
    // single shared path means both surfaces pick up the same symlink.
    Agent {
        label: "Antigravity",
        parent: AgentParent::Home(".gemini/skills"),
    },
    // Hermes (NousResearch/hermes-agent) resolves user skills from
    // `~/.hermes/skills/` at agent load time — the same directory its
    // `/skills install …` slash command and `hermes skills install`
    // CLI write to. Hermes' bundled `skills/computer-use/SKILL.md`
    // teaches the Hermes `computer_use` action vocabulary; the
    // cua-driver pack symlinked here adds the platform-specific deep
    // dives (MACOS.md / WINDOWS.md / LINUX.md / RECORDING.md /
    // BROWSER.md) that Hermes deliberately doesn't clone.
    Agent {
        label: "Hermes",
        parent: AgentParent::Home(".hermes/skills"),
    },
];

impl Agent {
    fn parent_path(&self) -> Result<PathBuf> {
        match self.parent {
            AgentParent::Home(seg) => {
                #[cfg(windows)]
                let base =
                    std::env::var("USERPROFILE").map_err(|_| anyhow!("USERPROFILE not set"))?;
                #[cfg(not(windows))]
                let base = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
                Ok(PathBuf::from(base).join(seg.replace('/', std::path::MAIN_SEPARATOR_STR)))
            }
            AgentParent::AppData(seg) => {
                #[cfg(windows)]
                let base = std::env::var("APPDATA").map_err(|_| anyhow!("APPDATA not set"))?;
                #[cfg(not(windows))]
                let base = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
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
        "update" => install(flags, true),
        "uninstall" => uninstall(flags),
        "status" => status(),
        "path" => print_path(),
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
    let options = parse_install_options(flags, force)?;

    // Sweep the legacy `cua-driver-rs`-named pack out FIRST so the
    // post-install state has exactly one skill pack at the new name.
    // Done before fetch so a fresh install on a previously-installed
    // machine doesn't leave orphan links pointing at a stale local dir.
    sweep_legacy_skill_pack();

    let local = local_skill_dir()?;
    recover_interrupted_activation(&local)?;
    let already_present = load_manifest(&local).ok().is_some_and(|manifest| {
        manifest.compatible_driver_version == env!("CARGO_PKG_VERSION")
            && audit_pack(&local, Some(&manifest)).is_valid()
    });

    if !already_present || options.force {
        fetch_into(&local, &options)
            .with_context(|| format!("failed to fetch skill pack to {}", local.display()))?;
        println!("✅ Skill pack at {}", local.display());
    } else {
        println!(
            "✅ Skill pack already at {} (use `cua-driver skills update` to refresh)",
            local.display()
        );
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
        println!("(No agent skills dirs present yet — install Claude Code / Codex / OpenClaw / OpenCode / Antigravity / Hermes then re-run.)");
    }
    Ok(())
}

fn parse_install_options(flags: &[String], update: bool) -> Result<InstallOptions> {
    let mut source_name: Option<String> = None;
    let mut source_path: Option<PathBuf> = None;
    let mut git_commit: Option<String> = None;
    let mut force = update;
    let mut all_platforms = false;
    let mut index = 0;

    while index < flags.len() {
        let flag = &flags[index];
        let mut take_value = |name: &str| -> Result<String> {
            index += 1;
            flags
                .get(index)
                .cloned()
                .ok_or_else(|| anyhow!("{name} requires a value"))
        };
        match flag.as_str() {
            "--force" => force = true,
            "--all-platforms" => all_platforms = true,
            "--from" => source_name = Some(take_value("--from")?),
            "--source" => source_path = Some(PathBuf::from(take_value("--source")?)),
            "--git-commit" => git_commit = Some(take_value("--git-commit")?),
            "--local" => {
                source_name = Some("local".to_owned());
                source_path = Some(PathBuf::from(take_value("--local")?));
            }
            _ if flag.starts_with("--from=") => {
                source_name = Some(flag["--from=".len()..].to_owned());
            }
            _ if flag.starts_with("--source=") => {
                source_path = Some(PathBuf::from(&flag["--source=".len()..]));
            }
            _ if flag.starts_with("--local=") => {
                source_name = Some("local".to_owned());
                source_path = Some(PathBuf::from(&flag["--local=".len()..]));
            }
            _ if flag.starts_with("--git-commit=") => {
                git_commit = Some(flag["--git-commit=".len()..].to_owned());
            }
            _ => bail!(
                "unknown option {flag:?}; supported options are --from <release|main|local>, \
                 --source <path>, --local <path>, --git-commit <sha>, --all-platforms, and --force"
            ),
        }
        index += 1;
    }

    let source = match source_name.as_deref().unwrap_or("release") {
        "release" => InstallSource::Release,
        "main" => InstallSource::Main,
        "local" => InstallSource::Local(source_path.clone().ok_or_else(|| {
            anyhow!("--from local requires --source <skill-directory> (or use --local <skill-directory>)")
        })?),
        other => bail!("unsupported skill source {other:?}; expected release, main, or local"),
    };

    if source_path.is_some() && !matches!(source, InstallSource::Local(_)) {
        bail!("--source is only valid with --from local");
    }
    if let Some(commit) = git_commit.as_deref() {
        validate_git_commit(commit)?;
        if !matches!(source, InstallSource::Local(_)) {
            bail!("--git-commit is only valid with a local source");
        }
    }

    force |= !matches!(source, InstallSource::Release);

    Ok(InstallOptions {
        source,
        force,
        all_platforms,
        git_commit,
    })
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
                eprintln!(
                    "  warning: could not remove legacy local pack at {}: {e}",
                    legacy_local.display()
                );
            } else {
                println!(
                    "  cleaned up legacy local pack at {}",
                    legacy_local.display()
                );
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
                    eprintln!(
                        "  warning: could not remove legacy pack at {}: {e}",
                        dir.display()
                    );
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
            eprintln!(
                "  warning: could not remove legacy {} link at {}: {e}",
                agent.label,
                legacy_link.display()
            );
        } else {
            println!(
                "  cleaned up legacy {} link at {}",
                agent.label,
                legacy_link.display()
            );
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
    let resolves = link.exists();
    if has_metadata && resolves {
        println!(
            "  {} skill link already exists at {} (skipping)",
            agent.label,
            link.display()
        );
        return Ok(false);
    }
    if has_metadata && !resolves && is_symlink_or_junction(&link) {
        // Dangling link/junction — target was removed (typical after
        // sweep_legacy_skill_pack cleaned a pre-rename pack out from
        // under it). Remove + recreate pointing at the new target.
        if let Err(e) = remove_link(&link) {
            eprintln!(
                "  warning: could not remove stale {} link at {}: {e}",
                agent.label,
                link.display()
            );
            return Ok(false);
        }
        println!(
            "  cleaned up stale {} link at {}",
            agent.label,
            link.display()
        );
    }
    make_dir_symlink(local_skill_dir, &link).with_context(|| {
        format!(
            "symlink {} -> {}",
            link.display(),
            local_skill_dir.display()
        )
    })?;
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

fn fetch_into(dest: &Path, options: &InstallOptions) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow!("skill directory has no parent"))?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".{SKILL_PACK_NAME}.staging-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&staging)?;

    let staged_result = (|| -> Result<()> {
        match &options.source {
            InstallSource::Release => fetch_release_into(&staging)?,
            InstallSource::Main => fetch_main_into(&staging)?,
            InstallSource::Local(source) => {
                copy_local_into(source, &staging, options.git_commit.clone())?
            }
        }

        let manifest = load_manifest(&staging)?;
        validate_manifest_shape(&manifest)?;
        ensure_compatible(&manifest)?;
        let report = audit_pack(&staging, Some(&manifest));
        if !report.is_valid() {
            bail!(
                "downloaded skill pack failed integrity validation: {}",
                describe_integrity(&report)
            );
        }

        apply_platform_filter(&staging, manifest, options.all_platforms)?;
        let filtered = load_manifest(&staging)?;
        let filtered_report = audit_pack(&staging, Some(&filtered));
        if !filtered_report.is_valid() {
            bail!(
                "staged skill pack failed integrity validation: {}",
                describe_integrity(&filtered_report)
            );
        }
        activate_staged(dest, &staging)
    })();

    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    staged_result
}

fn fetch_release_into(dest: &Path) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let url = format!(
        "https://github.com/trycua/cua/releases/download/cua-driver-rs-v{version}/cua-driver-rs-v{version}-skills.tar.gz"
    );
    let bytes = http_get_bytes(&url).with_context(|| format!("GET {url}"))?;
    extract_tar_gz(&bytes, dest)?;
    let manifest = load_manifest(dest)?;
    if manifest.source.kind != SourceKind::Release {
        bail!(
            "release archive declares source kind {}",
            manifest.source.kind
        );
    }
    if manifest.compatible_driver_version != version || manifest.skill_version != version {
        bail!(
            "release archive version mismatch: asset={version}, skill={}, compatible_driver={}",
            manifest.skill_version,
            manifest.compatible_driver_version
        );
    }
    Ok(())
}

fn fetch_main_into(dest: &Path) -> Result<()> {
    let commit = resolve_main_commit()?;
    let base = main_raw_base(&commit)?;
    for file in SKILL_FILES {
        let url = format!("{base}/Skills/cua-driver/{file}");
        let body = http_get_bytes(&url).with_context(|| format!("GET {url}"))?;
        fs::write(dest.join(file), body)?;
    }
    let cargo_url = format!("{base}/Cargo.toml");
    let cargo_toml = http_get_text(&cargo_url).with_context(|| format!("GET {cargo_url}"))?;
    let compatible_driver_version = parse_workspace_version(&cargo_toml)?;
    let skill_version = parse_skill_version(&fs::read_to_string(dest.join("SKILL.md"))?)?;
    write_generated_manifest(
        dest,
        skill_version,
        compatible_driver_version,
        SkillPackSource {
            kind: SourceKind::Main,
            git_commit: Some(commit),
        },
    )
}

fn main_raw_base(commit: &str) -> Result<String> {
    validate_git_commit(commit)?;
    Ok(format!(
        "https://raw.githubusercontent.com/trycua/cua/{commit}/libs/cua-driver/rust"
    ))
}

fn resolve_main_commit() -> Result<String> {
    let url = "https://api.github.com/repos/trycua/cua/commits/main";
    let body = http_get_text(url).with_context(|| format!("GET {url}"))?;
    parse_resolved_commit(&body)
}

fn parse_resolved_commit(body: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct CommitResponse {
        sha: String,
    }
    let response: CommitResponse =
        serde_json::from_str(body).context("invalid GitHub commit response")?;
    validate_git_commit(&response.sha)?;
    Ok(response.sha)
}

fn copy_local_into(source: &Path, dest: &Path, git_commit: Option<String>) -> Result<()> {
    if !source.is_dir() {
        bail!(
            "local skill source is not a directory: {}",
            source.display()
        );
    }
    copy_payload_tree(source, dest, source)?;
    let skill_version = parse_skill_version(&fs::read_to_string(dest.join("SKILL.md"))?)?;
    let compatible_driver_version = env!("CARGO_PKG_VERSION").to_owned();
    if skill_version != compatible_driver_version {
        bail!(
            "local skill version {skill_version} does not match this driver build {compatible_driver_version}"
        );
    }
    write_generated_manifest(
        dest,
        skill_version,
        compatible_driver_version,
        SkillPackSource {
            kind: SourceKind::Local,
            git_commit,
        },
    )
}

fn copy_payload_tree(root: &Path, dest: &Path, current: &Path) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let source_path = entry.path();
        let relative = source_path.strip_prefix(root)?;
        if relative == Path::new(MANIFEST_FILE) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!(
                "local skill source contains a symlink: {}",
                relative.display()
            );
        }
        let destination = dest.join(relative);
        if file_type.is_dir() {
            fs::create_dir_all(&destination)?;
            copy_payload_tree(root, dest, &source_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source_path, &destination)?;
        } else {
            bail!(
                "local skill source contains an unsupported entry: {}",
                relative.display()
            );
        }
    }
    Ok(())
}

fn http_get_text(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .header("User-Agent", "cua-driver")
        .call()
        .map_err(|e| anyhow!("HTTP error fetching {url}: {e}"))?;
    if resp.status() != 200 {
        bail!("HTTP {} fetching {url}", resp.status());
    }
    Ok(resp.into_body().read_to_string()?)
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .header("User-Agent", "cua-driver")
        .call()
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
        let out = dest.join(&stripped);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}

fn write_generated_manifest(
    directory: &Path,
    skill_version: String,
    compatible_driver_version: String,
    source: SkillPackSource,
) -> Result<()> {
    let files = collect_payload_files(directory)?
        .into_iter()
        .map(|path| {
            let hash = sha256_file(&directory.join(&path))?;
            Ok(SkillPackFile { path, sha256: hash })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = SkillPackManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        skill_version,
        compatible_driver_version,
        source,
        files,
    };
    write_manifest(directory, &manifest)
}

fn write_manifest(directory: &Path, manifest: &SkillPackManifest) -> Result<()> {
    let mut json = serde_json::to_string_pretty(manifest)?;
    json.push('\n');
    fs::write(directory.join(MANIFEST_FILE), json)?;
    Ok(())
}

fn load_manifest(directory: &Path) -> Result<SkillPackManifest> {
    let path = directory.join(MANIFEST_FILE);
    let body = fs::read_to_string(&path)
        .with_context(|| format!("missing or unreadable manifest {}", path.display()))?;
    serde_json::from_str(&body).with_context(|| format!("invalid manifest {}", path.display()))
}

fn validate_manifest_shape(manifest: &SkillPackManifest) -> Result<()> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        bail!(
            "unsupported skill-pack schema {}; expected {}",
            manifest.schema_version,
            MANIFEST_SCHEMA_VERSION
        );
    }
    semver::Version::parse(&manifest.skill_version)
        .with_context(|| format!("invalid skill version {:?}", manifest.skill_version))?;
    semver::Version::parse(&manifest.compatible_driver_version).with_context(|| {
        format!(
            "invalid compatible driver version {:?}",
            manifest.compatible_driver_version
        )
    })?;
    if manifest.source.kind == SourceKind::Main && manifest.source.git_commit.is_none() {
        bail!("main skill source must record an immutable Git commit");
    }
    if let Some(commit) = manifest.source.git_commit.as_deref() {
        validate_git_commit(commit)?;
    }
    if manifest.files.is_empty() {
        bail!("skill-pack manifest has no payload files");
    }
    let mut previous: Option<&str> = None;
    let mut has_skill = false;
    for file in &manifest.files {
        validate_relative_path(&file.path)?;
        if file.path == MANIFEST_FILE {
            bail!("manifest cannot hash itself");
        }
        if let Some(prior) = previous {
            if prior >= file.path.as_str() {
                bail!("manifest file list must be strictly sorted and unique");
            }
        }
        previous = Some(&file.path);
        has_skill |= file.path == "SKILL.md";
        if file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!("invalid SHA-256 for {:?}", file.path);
        }
    }
    if !has_skill {
        bail!("skill-pack manifest does not include SKILL.md");
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<()> {
    let candidate = Path::new(path);
    if path.is_empty()
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || path.contains('\\')
    {
        bail!("unsafe manifest path {path:?}");
    }
    Ok(())
}

fn validate_git_commit(commit: &str) -> Result<()> {
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Git commit must be a full 40-character hexadecimal SHA");
    }
    Ok(())
}

fn ensure_compatible(manifest: &SkillPackManifest) -> Result<()> {
    let running = env!("CARGO_PKG_VERSION");
    if manifest.compatible_driver_version != running {
        bail!(
            "skill pack requires cua-driver {}, but the running driver is {}; previous installation retained",
            manifest.compatible_driver_version,
            running
        );
    }
    Ok(())
}

fn apply_platform_filter(
    directory: &Path,
    mut manifest: SkillPackManifest,
    all_platforms: bool,
) -> Result<()> {
    if all_platforms {
        return Ok(());
    }
    manifest.files.retain(|file| {
        if is_excluded_platform_doc(&file.path, false) {
            let _ = fs::remove_file(directory.join(&file.path));
            false
        } else {
            true
        }
    });
    write_manifest(directory, &manifest)
}

fn audit_pack(directory: &Path, manifest: Option<&SkillPackManifest>) -> IntegrityReport {
    let Some(manifest) = manifest else {
        return IntegrityReport {
            manifest_error: Some(format!("missing or invalid {MANIFEST_FILE}")),
            ..IntegrityReport::default()
        };
    };
    if let Err(error) = validate_manifest_shape(manifest) {
        return IntegrityReport {
            manifest_error: Some(error.to_string()),
            ..IntegrityReport::default()
        };
    }

    let expected = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect::<BTreeMap<_, _>>();
    let actual = match collect_payload_files(directory) {
        Ok(files) => files.into_iter().collect::<BTreeSet<_>>(),
        Err(error) => {
            return IntegrityReport {
                manifest_error: Some(error.to_string()),
                ..IntegrityReport::default()
            };
        }
    };
    let expected_paths = expected.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected_paths
        .difference(&actual)
        .cloned()
        .collect::<Vec<_>>();
    let extra = actual
        .difference(&expected_paths)
        .cloned()
        .collect::<Vec<_>>();
    let modified = expected_paths
        .intersection(&actual)
        .filter_map(|path| match sha256_file(&directory.join(path)) {
            Ok(hash) if expected.get(path) == Some(&hash) => None,
            _ => Some(path.clone()),
        })
        .collect();
    IntegrityReport {
        missing,
        extra,
        modified,
        manifest_error: None,
    }
}

fn collect_payload_files(directory: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_payload_files_from(directory, directory, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_payload_files_from(root: &Path, current: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        if relative == Path::new(MANIFEST_FILE) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!("skill pack contains symlink {}", relative.display());
        }
        if file_type.is_dir() {
            collect_payload_files_from(root, &path, files)?;
        } else if file_type.is_file() {
            files.push(path_to_manifest_string(relative)?);
        } else {
            bail!(
                "skill pack contains unsupported entry {}",
                relative.display()
            );
        }
    }
    Ok(())
}

fn path_to_manifest_string(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("skill path is not UTF-8")),
            _ => Err(anyhow!("skill path is not relative")),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join("/"))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_skill_version(skill: &str) -> Result<String> {
    skill
        .lines()
        .find_map(|line| line.strip_prefix("version:").map(str::trim))
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("SKILL.md frontmatter has no version"))
}

fn parse_workspace_version(cargo_toml: &str) -> Result<String> {
    let mut in_workspace_package = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace_package = trimmed == "[workspace.package]";
            continue;
        }
        if in_workspace_package {
            if let Some(value) = trimmed.strip_prefix("version") {
                if let Some(value) = value.trim_start().strip_prefix('=') {
                    return Ok(value.trim().trim_matches('"').to_owned());
                }
            }
        }
    }
    bail!("workspace Cargo.toml has no [workspace.package] version")
}

fn describe_integrity(report: &IntegrityReport) -> String {
    let mut parts = Vec::new();
    if let Some(error) = &report.manifest_error {
        parts.push(format!("manifest: {error}"));
    }
    if !report.missing.is_empty() {
        parts.push(format!("missing: {}", report.missing.join(", ")));
    }
    if !report.extra.is_empty() {
        parts.push(format!("extra: {}", report.extra.join(", ")));
    }
    if !report.modified.is_empty() {
        parts.push(format!("modified: {}", report.modified.join(", ")));
    }
    if parts.is_empty() {
        "valid".to_owned()
    } else {
        parts.join("; ")
    }
}

fn backup_path(dest: &Path) -> Result<PathBuf> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow!("skill directory has no parent"))?;
    Ok(parent.join(format!(".{SKILL_PACK_NAME}.previous")))
}

fn recover_interrupted_activation(dest: &Path) -> Result<()> {
    let backup = backup_path(dest)?;
    if !backup.exists() {
        return Ok(());
    }
    if !dest.exists() {
        fs::rename(&backup, dest)
            .context("restore previous skill pack after interrupted update")?;
        return Ok(());
    }

    let dest_manifest = load_manifest(dest).ok();
    if audit_pack(dest, dest_manifest.as_ref()).is_valid() {
        fs::remove_dir_all(&backup)?;
        return Ok(());
    }
    let backup_manifest = load_manifest(&backup).ok();
    if !audit_pack(&backup, backup_manifest.as_ref()).is_valid() {
        bail!("both active and rollback skill packs are invalid; refusing to overwrite either");
    }
    fs::remove_dir_all(dest)?;
    fs::rename(&backup, dest).context("restore previous valid skill pack")
}

fn activate_staged(dest: &Path, staging: &Path) -> Result<()> {
    let backup = backup_path(dest)?;
    if backup.exists() {
        bail!(
            "stale rollback directory remains at {}; rerun the command to recover it",
            backup.display()
        );
    }
    if dest.exists() {
        fs::rename(dest, &backup).context("move active skill pack to rollback location")?;
    }
    if let Err(error) = fs::rename(staging, dest) {
        if backup.exists() {
            fs::rename(&backup, dest)
                .context("failed to activate new pack and failed to restore previous pack")?;
        }
        return Err(error).context("activate staged skill pack; previous installation restored");
    }
    if backup.exists() {
        if let Err(error) = fs::remove_dir_all(&backup) {
            eprintln!(
                "  warning: activated the verified skill pack but could not remove rollback copy at {}: {error}",
                backup.display()
            );
        }
    }
    Ok(())
}

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
                    println!(
                        "  {} link at {} is not a symlink/junction; leaving alone",
                        agent.label,
                        link.display()
                    );
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
                    println!(
                        "  ✅ removed legacy local skill pack at {}",
                        local.display()
                    );
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
    p.symlink_metadata()
        .map(|md| md.file_type().is_symlink())
        .unwrap_or(false)
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
    println!("Running driver version: {}", env!("CARGO_PKG_VERSION"));
    if local.exists() {
        println!("Local skill pack: {}", local.display());
        let manifest = load_manifest(&local);
        match manifest {
            Ok(manifest) => {
                let report = audit_pack(&local, Some(&manifest));
                let compatible = manifest.compatible_driver_version == env!("CARGO_PKG_VERSION");
                let provenance = match manifest.source.git_commit.as_deref() {
                    Some(commit) => format!("{} ({commit})", manifest.source.kind),
                    None => manifest.source.kind.to_string(),
                };
                println!("Installed skill version: {}", manifest.skill_version);
                println!(
                    "Compatible driver version: {}",
                    manifest.compatible_driver_version
                );
                println!("Source: {provenance}");
                println!(
                    "Integrity: {}",
                    if report.is_valid() {
                        "valid ✅"
                    } else {
                        "invalid ❌"
                    }
                );
                println!(
                    "Compatibility: {}",
                    if compatible {
                        "compatible ✅"
                    } else {
                        "incompatible ❌"
                    }
                );
                println!("Missing files: {}", display_file_list(&report.missing));
                println!("Extra/obsolete files: {}", display_file_list(&report.extra));
                println!("Modified files: {}", display_file_list(&report.modified));
                if let Some(error) = report.manifest_error {
                    println!("Manifest error: {error}");
                }
            }
            Err(error) => {
                let skill_version = fs::read_to_string(local.join("SKILL.md"))
                    .ok()
                    .and_then(|skill| parse_skill_version(&skill).ok())
                    .unwrap_or_else(|| "unknown".to_owned());
                let untracked = collect_payload_files(&local).unwrap_or_default();
                println!("Installed skill version: {skill_version}");
                println!("Compatible driver version: unknown");
                println!("Source: unknown");
                println!("Integrity: invalid ❌");
                println!("Compatibility: unknown");
                println!("Missing files: {MANIFEST_FILE}");
                println!("Extra/obsolete files: {}", display_file_list(&untracked));
                println!("Modified files: unknown");
                println!("Manifest error: {error}");
            }
        }
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
            println!(
                "  {} — agent dir not present ({})",
                agent.label,
                parent.display()
            );
            continue;
        }
        if !link.exists() && link.symlink_metadata().is_err() {
            println!("  {} — not linked ({})", agent.label, link.display());
        } else if is_symlink_or_junction(&link) {
            let target = fs::read_link(&link).ok();
            match target {
                Some(t) => println!(
                    "  {} — ✅ linked: {} → {}",
                    agent.label,
                    link.display(),
                    t.display()
                ),
                None => println!("  {} — ✅ linked: {}", agent.label, link.display()),
            }
        } else {
            println!(
                "  {} — non-symlink path at {} (left alone)",
                agent.label,
                link.display()
            );
        }
    }
    Ok(())
}

fn display_file_list(files: &[String]) -> String {
    if files.is_empty() {
        "none".to_owned()
    } else {
        files.join(", ")
    }
}

fn print_path() -> Result<()> {
    let local = local_skill_dir()?;
    println!("{}", local.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        activate_staged, apply_platform_filter, audit_pack, copy_local_into, extract_tar_gz,
        fetch_into, load_manifest, main_raw_base, parse_install_options, parse_resolved_commit,
        recover_interrupted_activation, write_generated_manifest, InstallOptions, InstallSource,
        SkillPackSource, SourceKind, MANIFEST_FILE, SKILL_FILES,
    };
    use std::path::PathBuf;
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

    fn write_valid_pack(directory: &std::path::Path, marker: &str) {
        std::fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nversion: {}\n---\n{marker}\n",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .unwrap();
        std::fs::write(directory.join("README.md"), marker).unwrap();
        write_generated_manifest(
            directory,
            env!("CARGO_PKG_VERSION").to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
            SkillPackSource {
                kind: SourceKind::Local,
                git_commit: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn from_main_manifest_matches_canonical_markdown_files() {
        let skill_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Skills/cua-driver");
        let mut canonical = std::fs::read_dir(&skill_dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", skill_dir.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("md"))
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        canonical.sort();

        let mut manifest = SKILL_FILES
            .iter()
            .map(|file| (*file).to_owned())
            .collect::<Vec<_>>();
        manifest.sort();

        assert_eq!(
            manifest, canonical,
            "SKILL_FILES must include every canonical Markdown file"
        );
    }

    #[test]
    fn extract_flat_tarball_v_0_2_20_plus() {
        // Post-fix shape: one wrapper dir, files directly under it.
        //   cua-driver-rs-v0.2.20-skills/SKILL.md
        //   cua-driver-rs-v0.2.20-skills/WINDOWS.md
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/SKILL.md", b"flat-skill"),
            ("cua-driver-rs-v0.2.20-skills/WINDOWS.md", b"flat-win"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path()).unwrap();

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
            (
                "cua-driver-rs-v0.2.18-skills/cua-driver-rs/SKILL.md",
                b"legacy-skill",
            ),
            (
                "cua-driver-rs-v0.2.18-skills/cua-driver-rs/WINDOWS.md",
                b"legacy-win",
            ),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path()).unwrap();

        let s = std::fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert_eq!(s, "legacy-skill");
        let w = std::fs::read_to_string(dest.path().join("WINDOWS.md")).unwrap();
        assert_eq!(w, "legacy-win");
        assert!(
            !dest.path().join("cua-driver-rs").exists(),
            "nested cua-driver-rs/ dir should have been stripped"
        );
    }

    #[test]
    fn extract_double_wrap_new_name_also_strips() {
        // Interim shape (v0.2.19, briefly): inner dir is `cua-driver/`.
        let bytes = build_tarball(&[(
            "cua-driver-rs-v0.2.19-skills/cua-driver/SKILL.md",
            b"interim-skill",
        )]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path()).unwrap();
        let s = std::fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert_eq!(s, "interim-skill");
        assert!(!dest.path().join("cua-driver").exists());
    }

    #[test]
    fn extract_preserves_subdirs_inside_pack() {
        // If a future skill pack adds a real subdir (e.g. `examples/`),
        // it must NOT be stripped — only the unambiguous pack-name
        // wrappers are.
        let bytes = build_tarball(&[("cua-driver-rs-v0.2.20-skills/examples/click.md", b"sample")]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path()).unwrap();
        let s = std::fs::read_to_string(dest.path().join("examples/click.md")).unwrap();
        assert_eq!(s, "sample");
    }

    #[test]
    fn extraction_keeps_complete_archive_for_pre_filter_validation() {
        // Integrity is checked against the complete source manifest before
        // host-only filtering derives the installed manifest.
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/README.md", b"r"),
            ("cua-driver-rs-v0.2.20-skills/SKILL.md", b"s"),
            ("cua-driver-rs-v0.2.20-skills/WINDOWS.md", b"w"),
            ("cua-driver-rs-v0.2.20-skills/MACOS.md", b"m"),
            ("cua-driver-rs-v0.2.20-skills/LINUX.md", b"l"),
            ("cua-driver-rs-v0.2.20-skills/RECORDING.md", b"R"),
            ("cua-driver-rs-v0.2.20-skills/BROWSER.md", b"B"),
            ("cua-driver-rs-v0.2.20-skills/EMBEDDING.md", b"E"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path()).unwrap();
        // README + SKILL + cross-platform docs are present.
        for f in [
            "README.md",
            "SKILL.md",
            "RECORDING.md",
            "BROWSER.md",
            "EMBEDDING.md",
        ] {
            assert!(
                dest.path().join(f).exists(),
                "{f} should be present after per-host extraction"
            );
        }
        for f in ["WINDOWS.md", "MACOS.md", "LINUX.md"] {
            assert!(
                dest.path().join(f).exists(),
                "archive extraction must validate before filtering {f}"
            );
        }
    }

    #[test]
    fn extraction_keeps_every_platform_doc() {
        let bytes = build_tarball(&[
            ("cua-driver-rs-v0.2.20-skills/WINDOWS.md", b"w"),
            ("cua-driver-rs-v0.2.20-skills/MACOS.md", b"m"),
            ("cua-driver-rs-v0.2.20-skills/LINUX.md", b"l"),
        ]);
        let dest = tempdir().unwrap();
        extract_tar_gz(&bytes, dest.path()).unwrap();
        for f in ["WINDOWS.md", "MACOS.md", "LINUX.md"] {
            assert!(
                dest.path().join(f).exists(),
                "--all-platforms should keep {f}"
            );
        }
    }

    #[test]
    fn host_filter_rewrites_manifest_without_breaking_integrity() {
        let pack = tempdir().unwrap();
        for file in ["WINDOWS.md", "MACOS.md", "LINUX.md"] {
            std::fs::write(pack.path().join(file), file).unwrap();
        }
        std::fs::write(
            pack.path().join("SKILL.md"),
            format!("---\nversion: {}\n---\n", env!("CARGO_PKG_VERSION")),
        )
        .unwrap();
        write_generated_manifest(
            pack.path(),
            env!("CARGO_PKG_VERSION").to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
            SkillPackSource {
                kind: SourceKind::Local,
                git_commit: None,
            },
        )
        .unwrap();

        let manifest = load_manifest(pack.path()).unwrap();
        apply_platform_filter(pack.path(), manifest, false).unwrap();
        let filtered = load_manifest(pack.path()).unwrap();
        assert!(audit_pack(pack.path(), Some(&filtered)).is_valid());
        assert_eq!(
            filtered
                .files
                .iter()
                .filter(|file| ["WINDOWS.md", "MACOS.md", "LINUX.md"].contains(&file.path.as_str()))
                .count(),
            1
        );
    }

    #[test]
    fn parses_consistent_install_and_update_sources() {
        let local = parse_install_options(
            &["--from".into(), "local".into(), "--source=pack".into()],
            false,
        )
        .unwrap();
        assert_eq!(local.source, InstallSource::Local(PathBuf::from("pack")));
        assert!(local.force);

        let release = parse_install_options(&[], false).unwrap();
        assert_eq!(release.source, InstallSource::Release);
        assert!(!release.force);

        let main = parse_install_options(&["--from=main".into()], true).unwrap();
        assert_eq!(main.source, InstallSource::Main);
        assert!(main.force);

        assert!(
            parse_install_options(&["--from".into(), "branch-name".into()], false)
                .unwrap_err()
                .to_string()
                .contains("unsupported skill source")
        );
    }

    #[test]
    fn main_commit_resolution_requires_and_records_an_immutable_sha() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            parse_resolved_commit(&format!(r#"{{"sha":"{sha}"}}"#)).unwrap(),
            sha
        );
        assert!(main_raw_base(sha).unwrap().contains(sha));
        assert!(!main_raw_base(sha).unwrap().contains("/main/"));
        assert!(parse_resolved_commit(r#"{"sha":"main"}"#).is_err());
    }

    #[test]
    fn explicit_local_source_is_copied_and_attributed() {
        let source = tempdir().unwrap();
        std::fs::write(
            source.path().join("SKILL.md"),
            format!("---\nversion: {}\n---\n", env!("CARGO_PKG_VERSION")),
        )
        .unwrap();
        std::fs::write(source.path().join("README.md"), "local").unwrap();
        let destination = tempdir().unwrap();
        let commit = "0123456789abcdef0123456789abcdef01234567".to_owned();
        copy_local_into(source.path(), destination.path(), Some(commit.clone())).unwrap();

        let manifest = load_manifest(destination.path()).unwrap();
        assert_eq!(manifest.source.kind, SourceKind::Local);
        assert_eq!(manifest.source.git_commit.as_deref(), Some(commit.as_str()));
        assert!(audit_pack(destination.path(), Some(&manifest)).is_valid());
    }

    #[test]
    fn audit_reports_corrupt_incomplete_and_obsolete_content() {
        let pack = tempdir().unwrap();
        write_valid_pack(pack.path(), "original");
        let manifest = load_manifest(pack.path()).unwrap();

        std::fs::write(pack.path().join("README.md"), "modified").unwrap();
        std::fs::remove_file(pack.path().join("SKILL.md")).unwrap();
        std::fs::write(pack.path().join("OBSOLETE.md"), "old").unwrap();
        let report = audit_pack(pack.path(), Some(&manifest));
        assert_eq!(report.missing, vec!["SKILL.md"]);
        assert_eq!(report.extra, vec!["OBSOLETE.md"]);
        assert_eq!(report.modified, vec!["README.md"]);
    }

    #[test]
    fn audit_rejects_incomplete_manifest() {
        let pack = tempdir().unwrap();
        std::fs::write(pack.path().join("SKILL.md"), "content").unwrap();
        std::fs::write(pack.path().join(MANIFEST_FILE), "{}").unwrap();
        assert!(!audit_pack(pack.path(), load_manifest(pack.path()).ok().as_ref()).is_valid());
    }

    #[test]
    fn compatibility_mismatch_is_visible_without_changing_hash_integrity() {
        let pack = tempdir().unwrap();
        write_valid_pack(pack.path(), "valid");
        let mut manifest = load_manifest(pack.path()).unwrap();
        manifest.compatible_driver_version = "999.0.0".to_owned();
        super::write_manifest(pack.path(), &manifest).unwrap();

        let reloaded = load_manifest(pack.path()).unwrap();
        assert!(audit_pack(pack.path(), Some(&reloaded)).is_valid());
        assert_ne!(
            reloaded.compatible_driver_version,
            env!("CARGO_PKG_VERSION")
        );
        assert!(super::ensure_compatible(&reloaded).is_err());
    }

    #[test]
    fn failed_activation_restores_previous_valid_pack() {
        let root = tempdir().unwrap();
        let active = root.path().join("cua-driver");
        std::fs::create_dir(&active).unwrap();
        write_valid_pack(&active, "previous");
        let missing_stage = root.path().join("missing-stage");

        assert!(activate_staged(&active, &missing_stage).is_err());
        assert_eq!(
            std::fs::read_to_string(active.join("README.md")).unwrap(),
            "previous"
        );
        let manifest = load_manifest(&active).unwrap();
        assert!(audit_pack(&active, Some(&manifest)).is_valid());
    }

    #[test]
    fn incompatible_update_retains_previous_valid_pack() {
        let root = tempdir().unwrap();
        let active = root.path().join("cua-driver");
        std::fs::create_dir(&active).unwrap();
        write_valid_pack(&active, "previous");
        let incompatible = root.path().join("incompatible");
        std::fs::create_dir(&incompatible).unwrap();
        std::fs::write(
            incompatible.join("SKILL.md"),
            "---\nversion: 999.0.0\n---\n",
        )
        .unwrap();

        let options = InstallOptions {
            source: InstallSource::Local(incompatible),
            force: true,
            all_platforms: true,
            git_commit: None,
        };
        assert!(fetch_into(&active, &options).is_err());
        assert_eq!(
            std::fs::read_to_string(active.join("README.md")).unwrap(),
            "previous"
        );
        let manifest = load_manifest(&active).unwrap();
        assert!(audit_pack(&active, Some(&manifest)).is_valid());
    }

    #[test]
    fn interrupted_activation_recovers_previous_pack() {
        let root = tempdir().unwrap();
        let active = root.path().join("cua-driver");
        std::fs::create_dir(&active).unwrap();
        write_valid_pack(&active, "previous");
        let backup = root.path().join(".cua-driver.previous");
        std::fs::rename(&active, &backup).unwrap();

        recover_interrupted_activation(&active).unwrap();
        assert_eq!(
            std::fs::read_to_string(active.join("README.md")).unwrap(),
            "previous"
        );
        assert!(!backup.exists());
    }
}
