//! `cua agents {detect,setup,status,remove,update}` and the agent onboarding
//! that follows `cua auth login`, on `cua-agent-setup`.

use crate::util::{self, internal, line};
use clap::{Args, Subcommand};
use cua_agent_setup::{AgentSetup, AgentStatus, Change, McpServer, Outcome, Parts, Target};
use cua_sdk::CuaError;
use std::{
    collections::BTreeSet,
    io::{BufRead, Write},
    path::Path,
};

#[derive(Subcommand, Debug)]
pub enum AgentsCmd {
    /// List supported AI coding agents and which are installed (read-only).
    #[command(after_help = "Examples:
  cua agents detect")]
    Detect,
    /// Install the default cua skills and configure the cua MCP server.
    #[command(after_help = "Examples:
  # Every installed agent, asking before each change
  cua agents setup
  # Claude Code and Codex, no prompts
  cua agents setup --agents claude,codex -y
  # Hermes Agent (Nous Research): ~/.hermes/skills and ~/.hermes/config.yaml
  cua agents setup --agents hermes -y
  # Only the MCP server entry
  cua agents setup --mcp-only
  # Background computer-use: the cua-driver skill and MCP server
  cua agents setup --cua-driver --agents all -y")]
    Setup(SetupCmdArgs),
    /// What cua configured for each agent.
    #[command(after_help = "Examples:
  cua agents status")]
    Status,
    /// Remove the skills and MCP entries cua added (nothing else).
    #[command(after_help = "Examples:
  cua agents remove
  cua agents remove --agents codex --skills-only -y")]
    Remove(RemoveArgs),
    /// Refresh installed cua skills and managed MCP entries to this version.
    #[command(after_help = "Examples:
  cua agents update")]
    Update,
}

/// Flags shared by `cua agents setup` and `cua auth login` onboarding.
#[derive(Args, Debug, Clone, Default)]
pub struct SetupArgs {
    /// Agents: `all` (installed), `none`, or ids such as `claude,codex,hermes`
    /// (`cua agents detect` lists every supported agent).
    #[arg(long)]
    pub agents: Option<String>,
    /// Only install skills.
    #[arg(long, visible_alias = "no-mcp", conflicts_with = "mcp_only")]
    pub skills_only: bool,
    /// Only configure the MCP server.
    #[arg(long, visible_alias = "no-skills")]
    pub mcp_only: bool,
    /// Answer yes to every prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Replace skill folders cua did not write (the old one is backed up).
    #[arg(long)]
    pub force: bool,
    /// Command agents launch for the MCP server (default: the `cua` on
    /// PATH, else this binary).
    #[arg(long)]
    pub mcp_command: Option<String>,
}

/// `cua agents setup` flags: the shared ones plus `--cua-driver`.
#[derive(Args, Debug, Clone, Default)]
pub struct SetupCmdArgs {
    #[command(flatten)]
    pub base: SetupArgs,
    /// Set up background computer-use instead: the cua-driver skill and
    /// the cua-driver MCP server (`cua-driver mcp`). --mcp-command then
    /// names the cua-driver binary.
    #[arg(long)]
    pub cua_driver: bool,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Agents: `all` (default) or ids such as `claude,codex`.
    #[arg(long, default_value = "all")]
    pub agents: String,
    /// Only remove skills.
    #[arg(long, conflicts_with = "mcp_only")]
    pub skills_only: bool,
    /// Only remove the MCP entry.
    #[arg(long)]
    pub mcp_only: bool,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

fn err(e: cua_agent_setup::Error) -> CuaError {
    match e {
        cua_agent_setup::Error::InvalidArgument(m) => CuaError::InvalidArgument(m),
        other => CuaError::Internal(other.to_string()),
    }
}

/// `~/x` for paths under the home directory. The part after `~/` always
/// uses `/`, so Windows shows `~/.codex/config.toml` rather than a mix like
/// `~/.codex\config.toml` (PowerShell reads either separator).
fn tilde(setup: &AgentSetup, p: &Path) -> String {
    match p.strip_prefix(&setup.env().home) {
        Ok(rel) => {
            let parts: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("~/{}", parts.join("/"))
        }
        Err(_) => p.display().to_string(),
    }
}

/// Prompts need only a terminal on stdin: `install.sh` runs
/// `cua auth login </dev/tty` with stdout possibly piped.
fn stdin_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Line-based prompts on the terminal (stdout for the question, stdin for
/// the answer). Non-interactive runs never prompt.
pub struct Prompt {
    interactive: bool,
    yes: bool,
}

impl Prompt {
    pub fn new(yes: bool) -> Self {
        Prompt {
            interactive: stdin_is_tty(),
            yes,
        }
    }

    fn read(&self, out: &mut dyn Write, q: &str) -> Option<String> {
        let _ = write!(out, "{q} ");
        let _ = out.flush();
        let mut s = String::new();
        // Bounded: one line.
        match std::io::stdin().lock().read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s.trim().to_ascii_lowercase()),
        }
    }

    /// `[Y/n]` (default yes) or `[y/N]`.
    pub fn confirm(&self, out: &mut dyn Write, q: &str, default: bool) -> bool {
        if self.yes {
            return true;
        }
        if !self.interactive {
            return default;
        }
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        // Re-ask a bounded number of times on nonsense.
        for _ in 0..5 {
            match self.read(out, &format!("{q} {hint}")).as_deref() {
                None => return default,
                Some("") => return default,
                Some("y" | "yes") => return true,
                Some("n" | "no") => return false,
                Some(_) => line(out, "Please answer y or n."),
            }
        }
        default
    }

    /// `[y/n/never]`; `None` means never.
    pub fn yes_no_never(&self, out: &mut dyn Write, q: &str) -> Option<bool> {
        if self.yes {
            return Some(true);
        }
        for _ in 0..5 {
            match self.read(out, &format!("{q} [y/n/never]")).as_deref() {
                None => return Some(false),
                Some("y" | "yes") => return Some(true),
                Some("n" | "no" | "") => return Some(false),
                Some("never") => return None,
                Some(_) => line(out, "Please answer y, n or never."),
            }
        }
        Some(false)
    }
}

/// The MCP server agents should launch.
pub fn mcp_server(setup: &AgentSetup, command: Option<&str>) -> McpServer {
    if let Some(c) = command {
        return McpServer::new(c);
    }
    if setup.env().which("cua").is_some() {
        return McpServer::default_for(setup.env());
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "cua".into());
    McpServer::new(exe)
}

fn print_agents(setup: &AgentSetup, statuses: &[AgentStatus], out: &mut dyn Write) {
    let width = statuses
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    for a in statuses {
        let mark = if a.installed { "✓" } else { "✗" };
        let mut parts = Vec::new();
        if a.installed {
            if let Some(d) = &a.skills_dir {
                parts.push(format!("skills {}", tilde(setup, d)));
            }
            match &a.mcp_config {
                Some(c) => {
                    let state = if a.cua_configured {
                        " (cua configured)"
                    } else {
                        ""
                    };
                    parts.push(format!("mcp {}{state}", tilde(setup, c)));
                }
                None => parts.push("no MCP support".into()),
            }
            if let Some(e) = &a.error {
                parts.push(format!("error: {e}"));
            }
        } else {
            parts.push("not found".into());
        }
        line(
            out,
            format!("  {mark} {:<width$}  {}", a.name, parts.join(", ")),
        );
    }
}

/// `cua agents detect`.
pub fn cmd_detect(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let st = setup.detect();
    if json {
        util::json_line(
            out,
            &serde_json::json!({"agents": st, "skills": setup.bundled_skills()}),
        );
        return Ok(0);
    }
    line(out, "AI coding agents:");
    print_agents(&setup, &st, out);
    Ok(0)
}

/// `cua agents status`.
pub fn cmd_status(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let st = setup.status();
    if json {
        util::json_line(
            out,
            &serde_json::json!({
                "agents": st,
                "skills": setup.bundled_skills(),
                "state_file": setup.state_path(),
            }),
        );
        return Ok(0);
    }
    let bundled = setup.bundled_skills();
    let rows: Vec<Vec<String>> = st
        .iter()
        .filter(|a| a.installed || a.cua_configured)
        .map(|a| {
            let skills = if a.skills_dir.is_none() {
                "n/a".to_string()
            } else {
                let mut s = format!("{}/{}", a.skills_installed.len(), bundled.len());
                if !a.skills_outdated.is_empty() {
                    s.push_str(&format!(" ({} outdated)", a.skills_outdated.len()));
                }
                s
            };
            let mcp = match (&a.mcp_config, a.cua_configured, &a.error) {
                (None, _, _) => "n/a".to_string(),
                (_, _, Some(_)) => "error".into(),
                (_, true, _) if a.cua_managed => "configured".into(),
                (_, true, _) => "configured (not by cua agents)".into(),
                _ => "-".into(),
            };
            vec![a.name.clone(), skills, mcp]
        })
        .collect();
    if rows.is_empty() {
        line(out, "No AI coding agents found. See `cua agents detect`.");
        return Ok(0);
    }
    util::table(out, &["AGENT", "SKILLS", "MCP"], &rows);
    Ok(0)
}

/// Exit code for `--json` runs: 1 with the first failure as the last
/// stderr line (the GUI installer shows that line).
fn fail_line(outcomes: &[&Outcome]) -> i32 {
    let failed: Vec<&&Outcome> = outcomes.iter().filter(|o| o.failed()).collect();
    match failed.first() {
        None => 0,
        Some(o) => {
            eprintln!(
                "{} of {} steps failed; first: {}",
                failed.len(),
                outcomes.len(),
                o.detail
            );
            1
        }
    }
}

fn change_word(o: &Outcome) -> &'static str {
    match o.change {
        Change::Created if o.target == Target::Mcp => "added",
        Change::Created => "installed",
        Change::Updated => "updated",
        Change::Unchanged => "up to date",
        Change::Removed => "removed",
        Change::Skipped => "skipped",
        Change::Failed => "failed",
    }
}

fn mark(o: &Outcome) -> &'static str {
    match o.change {
        Change::Failed => "✗",
        Change::Skipped => "-",
        _ => "✓",
    }
}

/// Totals for the summary line.
#[derive(Default)]
struct Summary {
    skills_ok: usize,
    skill_locations: BTreeSet<std::path::PathBuf>,
    mcp_ok: Vec<String>,
    failures: usize,
}

/// The setup flow shared by `cua agents setup` and login onboarding.
/// Returns the process exit code (1 when anything failed).
pub fn run_setup(
    setup: &AgentSetup,
    args: &SetupArgs,
    prompt: &Prompt,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let statuses = setup.detect();
    let ids = match args.agents.as_deref() {
        Some(sel) => setup.select(sel).map_err(err)?,
        None => statuses
            .iter()
            .filter(|a| a.installed)
            .map(|a| a.id.clone())
            .collect(),
    };
    let do_skills = !args.mcp_only;
    let do_mcp = !args.skills_only;
    if json {
        let skills = if do_skills {
            setup.install_skills(&ids, &[], args.force).map_err(err)?
        } else {
            vec![]
        };
        let mcp = if do_mcp {
            let server = mcp_server(setup, args.mcp_command.as_deref());
            let with_mcp: Vec<String> = ids
                .iter()
                .filter(|id| {
                    statuses
                        .iter()
                        .any(|a| &a.id == *id && a.mcp_config.is_some())
                })
                .cloned()
                .collect();
            setup.configure_mcp(&with_mcp, &server).map_err(err)?
        } else {
            vec![]
        };
        let outcomes: Vec<&Outcome> = skills.iter().chain(&mcp).collect();
        util::json_line(
            out,
            &serde_json::json!({"agents": ids, "outcomes": outcomes}),
        );
        return Ok(fail_line(&outcomes));
    }

    line(out, "AI coding agents on this machine:");
    print_agents(setup, &statuses, out);
    if ids.is_empty() {
        line(
            out,
            "No agents selected. Install one, or pass --agents (for example --agents claude,codex).",
        );
        return Ok(0);
    }
    let names: Vec<String> = ids
        .iter()
        .filter_map(|id| cua_agent_setup::registry::find(id).map(|a| a.name.to_string()))
        .collect();
    line(out, format!("Setting up: {}", names.join(", ")));
    let mut sum = Summary::default();

    if do_skills {
        let skills = setup.bundled_skills();
        let n = skills.len();
        if prompt.confirm(out, &format!("Install {n} default cua skills?"), true) {
            for (i, s) in skills.iter().enumerate() {
                let res = setup
                    .install_skills(&ids, std::slice::from_ref(&s.name), args.force)
                    .map_err(err)?;
                let res: Vec<&Outcome> = res
                    .iter()
                    .filter(|o| o.target == Target::Skill && !o.item.is_empty())
                    .collect();
                let failed: Vec<&&Outcome> = res
                    .iter()
                    .filter(|o| o.failed() || o.change == Change::Skipped)
                    .collect();
                let ok = res.len() - failed.len();
                for o in &res {
                    if !o.failed() && o.change != Change::Skipped {
                        sum.skill_locations
                            .insert(o.path.parent().unwrap_or(&o.path).to_path_buf());
                    }
                }
                if failed.is_empty() {
                    sum.skills_ok += 1;
                    let words: BTreeSet<&str> = res.iter().map(|o| change_word(o)).collect();
                    line(
                        out,
                        format!(
                            "  [{}/{n}] ✓ {} ({}, {} location{})",
                            i + 1,
                            s.name,
                            words.into_iter().collect::<Vec<_>>().join(", "),
                            ok,
                            if ok == 1 { "" } else { "s" }
                        ),
                    );
                } else {
                    line(
                        out,
                        format!("  [{}/{n}] {} {}", i + 1, mark(failed[0]), s.name),
                    );
                    for o in failed {
                        if o.failed() {
                            sum.failures += 1;
                        }
                        line(
                            out,
                            format!("        {}: {}", tilde(setup, &o.path), o.detail),
                        );
                    }
                }
            }
        }
    }

    if do_mcp {
        let with_mcp: Vec<String> = ids
            .iter()
            .filter(|id| {
                statuses
                    .iter()
                    .any(|a| &a.id == *id && a.mcp_config.is_some())
            })
            .cloned()
            .collect();
        if !with_mcp.is_empty() && prompt.confirm(out, "Configure cua MCP server connection?", true)
        {
            let server = mcp_server(setup, args.mcp_command.as_deref());
            line(
                out,
                format!("  Server: {} {}", server.command, server.args.join(" ")),
            );
            for o in setup.configure_mcp(&with_mcp, &server).map_err(err)? {
                let name = cua_agent_setup::registry::find(&o.agents[0])
                    .map(|a| a.name)
                    .unwrap_or_default();
                if o.failed() {
                    sum.failures += 1;
                    line(out, format!("  ✗ {name}: {}", o.detail));
                } else {
                    sum.mcp_ok.push(name.to_string());
                    let bak = o
                        .backup
                        .as_deref()
                        .map(|b| format!(", backup {}", tilde(setup, b)))
                        .unwrap_or_default();
                    line(
                        out,
                        format!(
                            "  ✓ {name}: {} ({}{bak})",
                            tilde(setup, &o.path),
                            change_word(&o)
                        ),
                    );
                }
            }
        }
    }

    let mut parts = Vec::new();
    if do_skills {
        parts.push(format!(
            "{} skill{} in {} location{}",
            sum.skills_ok,
            if sum.skills_ok == 1 { "" } else { "s" },
            sum.skill_locations.len(),
            if sum.skill_locations.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if do_mcp {
        parts.push(format!(
            "MCP configured for {} agent{}",
            sum.mcp_ok.len(),
            if sum.mcp_ok.len() == 1 { "" } else { "s" }
        ));
    }
    line(out, format!("Done: {}.", parts.join("; ")));
    if !sum.mcp_ok.is_empty() {
        line(
            out,
            "Restart your agents to load the cua MCP server. Undo with `cua agents remove`.",
        );
    }
    if sum.failures > 0 {
        line(out, format!("{} step(s) failed; see above.", sum.failures));
        return Ok(1);
    }
    Ok(0)
}

/// `cua agents setup`.
pub fn cmd_setup(args: &SetupCmdArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let prompt = Prompt::new(args.base.yes || json);
    if args.cua_driver {
        return run_driver_setup(&setup, &args.base, &prompt, json, out);
    }
    run_setup(&setup, &args.base, &prompt, json, out)
}

/// The cua-driver server agents launch: `--mcp-command`, else the
/// installed cua-driver.
pub fn driver_server(setup: &AgentSetup, command: Option<&str>) -> McpServer {
    match command {
        Some(c) => McpServer::driver(c),
        None => McpServer::driver_for(setup.env()),
    }
}

/// `cua agents setup --cua-driver`: background computer-use for the
/// selected agents (the cua-driver skill and the `cua-driver mcp` server).
/// The installers' `cua-driver` item runs this; the Spaces onboarding card
/// runs the same engine call ([`AgentSetup::setup_cua_driver`]).
pub fn run_driver_setup(
    setup: &AgentSetup,
    args: &SetupArgs,
    prompt: &Prompt,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let ids = match args.agents.as_deref() {
        Some(sel) => setup.select(sel).map_err(err)?,
        None => setup.select("all").map_err(err)?,
    };
    let parts = Parts {
        skills: !args.mcp_only,
        mcp: !args.skills_only,
    };
    let server = driver_server(setup, args.mcp_command.as_deref());
    if json {
        let outcomes = setup
            .setup_cua_driver(&ids, &server, parts, args.force)
            .map_err(err)?;
        util::json_line(
            out,
            &serde_json::json!({"agents": ids, "server": server, "outcomes": outcomes}),
        );
        return Ok(fail_line(&outcomes.iter().collect::<Vec<_>>()));
    }
    if ids.is_empty() {
        line(
            out,
            "No AI coding agents found for cua-driver. Install one, or pass --agents (for example --agents claude,codex).",
        );
        return Ok(0);
    }
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|id| cua_agent_setup::registry::find(id).map(|a| a.name))
        .collect();
    line(
        out,
        format!(
            "Background computer-use (cua-driver) for: {}",
            names.join(", ")
        ),
    );
    if !prompt.confirm(out, "Install the cua-driver skill and MCP server?", true) {
        line(
            out,
            "Skipped. Run `cua agents setup --cua-driver` any time.",
        );
        return Ok(0);
    }
    let outcomes = setup
        .setup_cua_driver(&ids, &server, parts, args.force)
        .map_err(err)?;
    let mut failures = 0;
    for o in &outcomes {
        if o.target == Target::Skill && o.item.is_empty() {
            continue;
        }
        if o.failed() {
            failures += 1;
        }
        let what = match o.target {
            Target::Skill => format!("skill {}", o.item),
            Target::Mcp => format!("MCP server {}", o.item),
        };
        let detail = if o.failed() || o.change == Change::Skipped {
            format!(" ({})", o.detail)
        } else {
            String::new()
        };
        line(
            out,
            format!(
                "  {} {what}: {} {}{detail}",
                mark(o),
                tilde(setup, &o.path),
                change_word(o)
            ),
        );
    }
    if parts.mcp && server.command == "cua-driver" {
        let how = if cfg!(windows) {
            "& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Only cua-driver"
        } else {
            "curl -fsSL https://cua.ai/install.sh | sh -s -- --only cua-driver"
        };
        line(
            out,
            format!("cua-driver is not installed yet. Install it with: {how}"),
        );
    }
    line(
        out,
        "Restart your agents to load cua-driver. Undo with `cua agents remove`.",
    );
    if failures > 0 {
        line(out, format!("{failures} step(s) failed; see above."));
        return Ok(1);
    }
    Ok(0)
}

/// `cua agents remove`.
pub fn cmd_remove(args: &RemoveArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let ids: Vec<String> = if args.agents.trim() == "all" {
        cua_agent_setup::AGENTS
            .iter()
            .map(|a| a.id.to_string())
            .collect()
    } else {
        setup.select(&args.agents).map_err(err)?
    };
    let parts = Parts {
        skills: !args.mcp_only,
        mcp: !args.skills_only,
    };
    let prompt = Prompt::new(args.yes || json);
    if !prompt.confirm(
        out,
        "Remove the cua skills and MCP entries that cua added?",
        false,
    ) {
        if !stdin_is_tty() && !args.yes {
            line(out, "Pass --yes to remove without a prompt.");
        }
        return Ok(1);
    }
    let res = setup.remove(&ids, parts).map_err(err)?;
    let failed = res.iter().any(Outcome::failed);
    if json {
        util::json_line(out, &serde_json::json!({"outcomes": res}));
        return Ok(fail_line(&res.iter().collect::<Vec<_>>()));
    }
    let touched: Vec<&Outcome> = res
        .iter()
        .filter(|o| !(o.change == Change::Skipped && o.detail == "not configured by cua"))
        .collect();
    if touched.is_empty() {
        line(out, "Nothing to remove.");
    }
    for o in touched {
        let what = match o.target {
            Target::Skill => format!("skill {}", o.item),
            Target::Mcp => format!("MCP server {}", o.item),
        };
        let detail = if o.detail.is_empty() || o.detail == "removed" {
            String::new()
        } else {
            format!(" ({})", o.detail)
        };
        line(
            out,
            format!(
                "  {} {what}: {} {}{detail}",
                mark(o),
                tilde(&setup, &o.path),
                change_word(o)
            ),
        );
    }
    Ok(i32::from(failed))
}

/// `cua agents update`.
pub fn cmd_update(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let server = mcp_server(&setup, None);
    let res = setup.update(Some(&server)).map_err(err)?;
    let failed = res.iter().any(Outcome::failed);
    if json {
        util::json_line(out, &serde_json::json!({"outcomes": res}));
        return Ok(fail_line(&res.iter().collect::<Vec<_>>()));
    }
    let changed: Vec<&Outcome> = res
        .iter()
        .filter(|o| o.change != Change::Unchanged)
        .collect();
    if changed.is_empty() {
        line(out, "Everything cua installed is up to date.");
    }
    for o in changed {
        line(
            out,
            format!(
                "  {} {} {}: {}",
                mark(o),
                o.item,
                change_word(o),
                tilde(&setup, &o.path)
            ),
        );
    }
    Ok(i32::from(failed))
}

/// Onboarding after a successful `cua auth login`.
pub fn after_login(
    args: &SetupArgs,
    no_onboarding: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let interactive = stdin_is_tty();
    if no_onboarding || args.agents.as_deref() == Some("none") {
        return Ok(0);
    }
    // Scripts opt in with --agents (or --yes); interactive users are asked.
    if !interactive && args.agents.is_none() && !args.yes {
        return Ok(0);
    }
    let explicit = args.agents.is_some() || args.yes;
    if !explicit {
        if cua_agent_setup::config::onboarding_declined(setup.env()) {
            return Ok(0);
        }
        let prompt = Prompt::new(false);
        line(out, "");
        match prompt.yes_no_never(
            out,
            "Configure cua skills and the cua MCP server for your AI coding agent(s)?",
        ) {
            Some(true) => {}
            Some(false) => {
                line(out, "Skipped. Run `cua agents setup` any time.");
                return Ok(0);
            }
            None => {
                cua_agent_setup::config::set_onboarding_declined(setup.env(), true)
                    .map_err(|e| internal(e.to_string()))?;
                line(
                    out,
                    "OK, cua will not ask again (saved in ~/.cua/config). Run `cua agents setup` any time.",
                );
                return Ok(0);
            }
        }
    }
    let prompt = Prompt::new(args.yes);
    run_setup(&setup, args, &prompt, false, out)
}
