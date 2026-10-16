//! `cua connect {detect,setup,status,remove,update}` (formerly `cua agents`) and the agent onboarding
//! that follows `cua auth login`, on `cua-agent-setup`.

use crate::util::{self, internal, line};
use clap::{Args, Subcommand};
use cua_agent_setup::{
    AgentSetup, AgentStatus, Change, ConnectionReport, ConnectionRow, ConnectionState,
    DRIVER_MCP_SERVER_NAME, ImportState, McpServer, McpState, Outcome, Parts, ProblemAction,
    RemoveOptions, SignedInState, StatusOptions, Target, connection::ConnectParts,
};
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
  cua connect detect")]
    Detect,
    /// Install the default cua skills and configure the cua MCP server.
    #[command(after_help = "Examples:
  # Every installed agent, asking before each change
  cua connect setup
  # Claude Code and Codex, no prompts
  cua connect setup --agents claude,codex -y
  # Hermes Agent (Nous Research): ~/.hermes/skills and ~/.hermes/config.yaml
  cua connect setup --agents hermes -y
  # Only the MCP server entry
  cua connect setup --mcp-only
  # Background computer-use: the cua-driver skill and MCP server
  cua connect setup --cua-driver --agents all -y
  # Also copy each agent's sign-in to Keyvault on This Mac (Codex's
  # ChatGPT sign-in, Claude Code's `claude setup-token`, Hermes' key)
  cua connect setup --agents codex,claude --sign-in -y")]
    Setup(SetupCmdArgs),
    /// Whether each agent is really connected: installed and its version,
    /// signed in, Cua's MCP entry present and answering, and its sign-in
    /// copied to Keyvault. The same rows as the app and the
    /// `connect_status` MCP tool.
    #[command(after_help = "Examples:
  cua connect status
  cua connect status --json")]
    Status,
    /// Remove the Cua Spaces skills and MCP entries cua added (nothing else).
    #[command(after_help = "Examples:
  # See what would be removed, change nothing
  cua connect remove --dry-run
  cua connect remove
  cua connect remove --agents codex --skills-only -y
  # Cua Driver's skill and MCP server too (they stay otherwise)
  cua connect remove --include-cua-driver
  # Also delete the sign-in Connect copied to Keyvault
  cua connect remove --agents codex --sign-in -y")]
    Remove(RemoveArgs),
    /// Refresh installed cua skills and managed MCP entries to this version.
    #[command(after_help = "Examples:
  cua connect update")]
    Update,
}

/// Flags shared by `cua connect setup` and `cua auth login` onboarding.
#[derive(Args, Debug, Clone, Default)]
pub struct SetupArgs {
    /// Agents: `all` (installed), `none`, or ids such as `claude,codex,hermes`
    /// (`cua connect detect` lists every supported agent).
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

/// `cua connect setup` flags: the shared ones plus `--cua-driver`.
#[derive(Args, Debug, Clone, Default)]
pub struct SetupCmdArgs {
    #[command(flatten)]
    pub base: SetupArgs,
    /// Set up background computer-use instead: the cua-driver skill and
    /// the cua-driver MCP server (`cua-driver mcp`). --mcp-command then
    /// names the cua-driver binary.
    #[arg(long)]
    pub cua_driver: bool,
    /// Also make a sign-in for each agent's runs in your Spaces and keep it
    /// in Keyvault on This Mac: Claude Code's `claude setup-token` (one
    /// browser approval) and Codex's own device sign-in (a link and a code
    /// to enter there, in a folder Cua owns); Hermes uses the providers
    /// connected in Cua Spaces (Settings, Connectors).
    /// The agent's own sign-in files are never read or changed.
    #[arg(long, conflicts_with = "cua_driver")]
    pub sign_in: bool,
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
    /// Also remove Cua Driver's skill and MCP server (`cua-driver`). Without
    /// it only Cua Spaces' own entries go: the `cua` server (`cua-local` for
    /// Local) and the Cua Spaces skills.
    #[arg(long)]
    pub include_cua_driver: bool,
    /// List exactly what would be removed and change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Also delete the sign-ins Connect copied to Keyvault for these agents.
    #[arg(long)]
    pub sign_in: bool,
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

    /// One answer, lowercased and trimmed, or `None` when the input ended
    /// before an answer came (Ctrl-D, a closed terminal). Running out of
    /// input is never an answer: callers must not treat `None` as yes, no
    /// or never.
    fn read(&self, out: &mut dyn Write, q: &str) -> Option<String> {
        // Keys typed before the question was on screen (an extra Enter at
        // an earlier prompt, a press while the browser sign-in ran) are not
        // answers to it.
        discard_typeahead();
        let _ = write!(out, "{q} ");
        let _ = out.flush();
        let mut s = String::new();
        // Bounded: one line.
        match std::io::stdin().lock().read_line(&mut s) {
            Ok(0) | Err(_) => {
                line(out, "");
                None
            }
            Ok(_) => Some(s.trim().to_ascii_lowercase()),
        }
    }

    /// `[Y/n]` (default yes) or `[y/N]`. Enter takes the default; with no
    /// answer at all (the input ended) the step is skipped, with a line
    /// saying so.
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
                None => {
                    line(out, NO_ANSWER);
                    return false;
                }
                Some("") => return default,
                Some("y" | "yes") => return true,
                Some("n" | "no") => return false,
                Some(_) => line(out, "Please answer y or n."),
            }
        }
        default
    }

    /// `[y/n/never]`.
    pub fn yes_no_never(&self, out: &mut dyn Write, q: &str) -> Choice {
        if self.yes {
            return Choice::Yes;
        }
        for _ in 0..5 {
            match self.read(out, &format!("{q} [y/n/never]")).as_deref() {
                None => return Choice::NoAnswer,
                Some("y" | "yes") => return Choice::Yes,
                Some("n" | "no" | "") => return Choice::No,
                Some("never") => return Choice::Never,
                Some(_) => line(out, "Please answer y, n or never."),
            }
        }
        Choice::No
    }
}

/// The answer to a `[y/n/never]` question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
    /// Do not ask again.
    Never,
    /// The input ended before anyone answered. Not a "no": nothing is
    /// recorded, and the caller says what to run later.
    NoAnswer,
}

const NO_ANSWER: &str = "No answer (the input ended), so cua skipped this step.";

/// Throws away input waiting on the terminal (see [`Prompt::read`]).
fn discard_typeahead() {
    #[cfg(unix)]
    // SAFETY: tcflush only acts on the descriptor; it fails harmlessly when
    // stdin is not a terminal.
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

/// The MCP server agents should launch.
pub fn mcp_server(setup: &AgentSetup, command: Option<&str>) -> McpServer {
    // The channel's own server (`cua`, or `cua-local` when this is the Local
    // CLI), launching the channel's own binary.
    let channel = setup.env().channel;
    if let Some(c) = command {
        return McpServer::for_channel(channel, c);
    }
    if setup.env().which(channel.cli_name()).is_some() {
        return McpServer::default_for(setup.env());
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| channel.cli_name().into());
    McpServer::for_channel(channel, exe)
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

/// `cua connect detect`.
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

/// `cua connect status`: the connection rows (CONTRACT §4).
pub fn cmd_status(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let report = connection_report(&setup);
    if json {
        util::json_line(out, &status_json(&setup, &report));
        return Ok(0);
    }
    print_connections(&report, out);
    Ok(0)
}

/// `cua connect status --json`: the connection rows, with each agent's
/// earlier status fields (`cua_configured`, `cua_managed`, skills, ...)
/// beside them, and `skills` and `state_file` as before, so scripts that read
/// the earlier output keep working. The earlier fields are kept for
/// compatibility only (CUA-2341 removes them).
pub fn status_json(setup: &AgentSetup, report: &ConnectionReport) -> serde_json::Value {
    let mut v = serde_json::to_value(report).unwrap_or_default();
    let earlier = setup.status();
    if let Some(rows) = v
        .get_mut("agents")
        .and_then(serde_json::Value::as_array_mut)
    {
        for row in rows {
            let id = row["id"].as_str().unwrap_or_default().to_string();
            let Some(old) = earlier.iter().find(|a| a.id == id) else {
                continue;
            };
            let (Some(row), Ok(serde_json::Value::Object(old))) =
                (row.as_object_mut(), serde_json::to_value(old))
            else {
                continue;
            };
            for (k, x) in old {
                row.entry(k).or_insert(x);
            }
        }
    }
    v["skills"] = serde_json::json!(setup.bundled_skills());
    v["state_file"] = serde_json::json!(setup.state_path());
    v
}

/// The rows `cua connect status` and the `connect_status` MCP tool show:
/// this CLI's own cua is the expected MCP server.
pub fn connection_report(setup: &AgentSetup) -> ConnectionReport {
    let opts = StatusOptions {
        expected: Some(mcp_server(setup, None)),
        refresh: true,
        ..Default::default()
    };
    let vault = cua_sdk::sign_in_vault();
    setup.connection_status(&opts, vault.as_deref())
}

fn state_word(r: &ConnectionRow) -> &'static str {
    match r.state {
        ConnectionState::Connected => "connected",
        ConnectionState::Attention => "needs you",
        ConnectionState::NotConnected => "not connected",
        ConnectionState::NotInstalled => "not installed",
    }
}

fn print_connections(report: &ConnectionReport, out: &mut dyn Write) {
    let rows: Vec<Vec<String>> = report
        .agents
        .iter()
        .map(|r| {
            let signed = match r.signed_in {
                SignedInState::Yes => "yes",
                SignedInState::No => "no",
                SignedInState::Unknown => "unknown",
                SignedInState::Expired => "expired",
            };
            let mcp = match r.mcp {
                McpState::Ok => "ok",
                McpState::Missing => "-",
                McpState::Broken => "broken",
                McpState::Stale => "other cua",
                McpState::Unsupported => "n/a",
            };
            let vault = match r.signin_import {
                ImportState::Ok => "copied",
                ImportState::Expired => "expired",
                ImportState::Missing => "-",
                ImportState::Unsupported => "n/a",
            };
            vec![
                r.name.clone(),
                state_word(r).into(),
                r.version.clone().unwrap_or_else(|| "-".into()),
                if r.installed {
                    signed.into()
                } else {
                    "-".into()
                },
                mcp.into(),
                vault.into(),
            ]
        })
        .collect();
    util::table(
        out,
        &["AGENT", "STATE", "VERSION", "SIGNED IN", "MCP", "KEYVAULT"],
        &rows,
    );
    for r in &report.agents {
        if let Some(p) = &r.problem {
            let fix = match p.action {
                ProblemAction::Reconnect => {
                    format!(" (cua connect setup --agents {} --sign-in)", r.id)
                }
                ProblemAction::SignIn => String::new(),
                ProblemAction::Disconnect => {
                    format!(" (cua connect remove --agents {} --sign-in)", r.id)
                }
                ProblemAction::Install | ProblemAction::None => String::new(),
            };
            line(out, format!("{}: {}{fix}", r.name, p.line));
        }
    }
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

/// The setup flow shared by `cua connect setup` and login onboarding.
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

    line(out, "AI coding agents on this computer:");
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
            "Restart your agents to load the cua MCP server. Undo with `cua connect remove`.",
        );
    }
    if sum.failures > 0 {
        line(out, format!("{} step(s) failed; see above.", sum.failures));
        return Ok(1);
    }
    Ok(0)
}

/// `cua connect setup`.
pub fn cmd_setup(args: &SetupCmdArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let setup = AgentSetup::from_env();
    let prompt = Prompt::new(args.base.yes || json);
    if args.cua_driver {
        return run_driver_setup(&setup, &args.base, &prompt, json, out);
    }
    if args.sign_in {
        return run_connect(&setup, &args.base, json, out);
    }
    run_setup(&setup, &args.base, &prompt, json, out)
}

/// `cua connect setup --sign-in`: the app's Connect, the same core call:
/// skills, MCP and the sign-in copy per agent.
fn run_connect(
    setup: &AgentSetup,
    args: &SetupArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let ids = match args.agents.as_deref() {
        Some(sel) => setup.select(sel).map_err(err)?,
        None => setup.select("all").map_err(err)?,
    };
    let server = mcp_server(setup, args.mcp_command.as_deref());
    let vault = cua_sdk::sign_in_vault();
    let parts = ConnectParts {
        skills: !args.mcp_only,
        mcp: !args.skills_only,
        sign_in: true,
    };
    // A sign-in with a code (Codex): the link and the code go to stderr, so
    // `--json` stays one line on stdout.
    let mut show = |agent: &str, d: &cua_agent_setup::signin::DeviceSignIn| {
        let name = cua_agent_setup::registry::find(agent)
            .map(cua_agent_setup::connection::display_name)
            .unwrap_or(agent);
        eprintln!(
            "  {name}: open {} and enter the code {} (it works once and expires in 15 minutes).",
            d.url, d.code
        );
    };
    let res = setup
        .connect_with(
            &ids,
            &server,
            parts,
            vault.as_deref(),
            cua_agent_setup::signin::SETUP_TOKEN_WAIT,
            Some(&mut show),
        )
        .map_err(err)?;
    let failed = res.iter().any(|r| !r.tools_ok);
    if json {
        util::json_line(out, &serde_json::json!({"agents": res}));
        return Ok(i32::from(failed));
    }
    for r in &res {
        let name = cua_agent_setup::registry::find(&r.agent)
            .map(cua_agent_setup::connection::display_name)
            .unwrap_or_default();
        match (&r.tools_line, &r.sign_in) {
            (Some(l), _) => line(out, format!("  ✗ {name}: {l}")),
            (None, Some(s)) if s.imported.is_some() => line(
                out,
                format!(
                    "  ✓ {name}: connected; {} copied to Keyvault",
                    s.imported
                        .as_ref()
                        .map(|i| i.label.as_str())
                        .unwrap_or_default()
                ),
            ),
            (None, Some(s)) => line(
                out,
                format!("  ✓ {name}: {}", s.line.clone().unwrap_or_default()),
            ),
            (None, None) => line(out, format!("  ✓ {name}: connected")),
        }
    }
    if !res.is_empty() {
        line(
            out,
            "Restart your agents to load the cua MCP server. Undo with `cua connect remove --sign-in`.",
        );
    }
    Ok(i32::from(failed))
}

/// The cua-driver server agents launch: `--mcp-command`, else the
/// installed cua-driver.
pub fn driver_server(setup: &AgentSetup, command: Option<&str>) -> McpServer {
    match command {
        Some(c) => McpServer::driver(c),
        None => McpServer::driver_for(setup.env()),
    }
}

/// `cua connect setup --cua-driver`: background computer-use for the
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
            "Skipped. Run `cua connect setup --cua-driver` any time.",
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
    // Nothing is written for a cua-driver that is missing or doesn't answer
    // MCP (the rows above say why).
    if outcomes
        .iter()
        .any(|o| o.failed() && o.target == Target::Mcp && o.item == DRIVER_MCP_SERVER_NAME)
    {
        let how = if cfg!(windows) {
            "& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Only cua-driver"
        } else {
            "curl -fsSL https://cua.ai/install.sh | sh -s -- --only cua-driver"
        };
        line(
            out,
            format!("cua-driver is not installed or not working. Install it with: {how}"),
        );
    }
    line(
        out,
        "Restart your agents to load cua-driver. Undo with `cua connect remove`.",
    );
    if failures > 0 {
        line(out, format!("{failures} step(s) failed; see above."));
        return Ok(1);
    }
    Ok(0)
}

/// `cua connect remove`.
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
    let opts = RemoveOptions {
        include_cua_driver: args.include_cua_driver,
        dry_run: args.dry_run,
    };
    if !args.dry_run {
        let prompt = Prompt::new(args.yes || json);
        let question = if args.include_cua_driver {
            "Remove the Cua Spaces and Cua Driver skills and MCP entries that cua added?"
        } else {
            "Remove the Cua Spaces skills and MCP entries that cua added?"
        };
        if !prompt.confirm(out, question, false) {
            if !stdin_is_tty() && !args.yes {
                line(out, "Pass --yes to remove without a prompt.");
            }
            return Ok(1);
        }
    }
    let vault = cua_sdk::sign_in_vault();
    let (res, removed) = setup
        .disconnect(&ids, parts, opts, args.sign_in, vault.as_deref())
        .map_err(err)?;
    let failed = res.iter().any(Outcome::failed);
    if json {
        util::json_line(
            out,
            &serde_json::json!({"dry_run": args.dry_run, "outcomes": res, "removed_sign_ins": removed}),
        );
        return Ok(fail_line(&res.iter().collect::<Vec<_>>()));
    }
    for env in &removed {
        line(out, format!("  ✓ sign-in {env}: deleted from Keyvault"));
    }
    let touched: Vec<&Outcome> = res
        .iter()
        .filter(|o| !(o.change == Change::Skipped && o.detail == "not configured by cua"))
        .collect();
    if args.dry_run {
        line(out, "Dry run: nothing is changed.");
    }
    if touched.is_empty() && removed.is_empty() {
        line(out, "Nothing to remove.");
    }
    for o in touched {
        let what = match o.target {
            Target::Skill => format!("skill {}", o.item),
            Target::Mcp => format!("MCP server {}", o.item),
        };
        let place = tilde(&setup, &o.path);
        if o.change == Change::Skipped && o.detail == cua_agent_setup::KEPT_CUA_DRIVER {
            line(out, format!("  - {what}: {place} kept ({})", o.detail));
            continue;
        }
        if args.dry_run && o.change == Change::Removed {
            if o.detail == "already gone" {
                line(out, format!("  - {what}: {place} already gone"));
            } else if o.detail == "restored the previous entry" {
                line(
                    out,
                    format!("  would restore the previous entry of {what}: {place}"),
                );
            } else {
                line(out, format!("  would remove {what}: {place}"));
            }
            continue;
        }
        let detail = if o.detail.is_empty() || o.detail == "removed" {
            String::new()
        } else {
            format!(" ({})", o.detail)
        };
        line(
            out,
            format!("  {} {what}: {place} {}{detail}", mark(o), change_word(o)),
        );
    }
    Ok(i32::from(failed))
}

/// `cua connect update`.
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
            "Let your AI coding agents (like Claude Code and Codex) use cua?",
        ) {
            Choice::Yes => {}
            Choice::No => {
                line(out, "Skipped. Run `cua connect setup` any time.");
                return Ok(0);
            }
            Choice::NoAnswer => {
                line(
                    out,
                    "No answer (the input ended), so your agents are unchanged. Run `cua connect setup` when you are ready.",
                );
                return Ok(0);
            }
            Choice::Never => {
                cua_agent_setup::config::set_onboarding_declined(setup.env(), true)
                    .map_err(|e| internal(e.to_string()))?;
                line(
                    out,
                    "OK, cua will not ask again (saved in ~/.cua/config). Run `cua connect setup` any time.",
                );
                return Ok(0);
            }
        }
    }
    let prompt = Prompt::new(args.yes);
    run_setup(&setup, args, &prompt, false, out)
}
