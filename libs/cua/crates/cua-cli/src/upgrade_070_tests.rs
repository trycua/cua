// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every `cua agent`, `cua volume`, `cua daemon` and `cua mcp` command, alias
//! and flag that the released cua 0.7.0 (the `cua` in Cua Spaces 0.13.1)
//! accepted is still accepted (CUA-2155). The list was recorded from that
//! binary's `--help` (`tests/fixtures/upgrade/0.7.0/cli-tree.txt`, see the
//! README next to it); a removed or renamed command or flag fails here.

use super::Cli;
use clap::{CommandFactory, Parser};

const TREE: &str = include_str!("../tests/fixtures/upgrade/0.7.0/cli-tree.txt");
const MCP_TOOLS: &str = include_str!("../tests/fixtures/upgrade/0.7.0/mcp-tools.txt");

/// The subcommand at `path` (names or aliases), from the built tree.
fn find<'a>(root: &'a clap::Command, path: &str) -> Option<&'a clap::Command> {
    let mut cmd = root;
    for part in path.split_whitespace() {
        cmd = cmd.find_subcommand(part)?;
    }
    Some(cmd)
}

fn has_flag(cmd: &clap::Command, flag: &str) -> bool {
    cmd.get_arguments().any(|a| match flag.strip_prefix("--") {
        Some(long) => {
            a.get_long() == Some(long) || a.get_all_aliases().is_some_and(|al| al.contains(&long))
        }
        None => {
            let c = flag.trim_start_matches('-').chars().next();
            c.is_some()
                && (a.get_short() == c
                    || a.get_all_short_aliases()
                        .is_some_and(|al| al.contains(&c.unwrap())))
        }
    })
}

#[test]
fn every_070_agent_command_alias_and_flag_still_parses() {
    let mut root = Cli::command();
    root.build();
    let mut missing = Vec::new();
    let mut checked = 0;
    for line in TREE
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        checked += 1;
        if let Some((path, flag)) = line.split_once(" :: ") {
            match find(&root, path) {
                Some(cmd) if has_flag(cmd, flag) => {}
                Some(_) => missing.push(format!("cua {path} {flag}")),
                None => missing.push(format!("cua {path} (for {flag})")),
            }
        } else if let Some((path, alias)) = line.split_once(" = ") {
            // The 0.7.0 alias still reaches a command.
            let (parent, _) = path.rsplit_once(' ').unwrap_or(("", path));
            let via_alias = if parent.is_empty() {
                root.find_subcommand(alias).is_some()
            } else {
                find(&root, parent)
                    .and_then(|p| p.find_subcommand(alias))
                    .is_some()
            };
            if !via_alias {
                missing.push(format!("cua {parent} {alias} (alias of {path})"));
            }
        } else if find(&root, line).is_none() {
            missing.push(format!("cua {line}"));
        }
    }
    assert!(
        checked > 400,
        "the recorded tree is incomplete: {checked} lines"
    );
    assert!(
        missing.is_empty(),
        "commands or flags cua 0.7.0 accepted are gone (keep an alias):\n{}",
        missing.join("\n")
    );
}

/// The commands a 0.7.0 user typed most, parsed whole (positional arguments
/// included), not only looked up.
#[test]
fn typical_070_command_lines_parse() {
    for line in [
        "cua agent create ada --harness hermes --in local:dev --env-from-host OPENAI_API_KEY",
        "cua agent persistent",
        "cua agent tell ada hello",
        "cua agent routine add ada --title Morning --prompt plan --daily 08:00",
        "cua agent routine add ada --title Weekly --prompt sum --weekly mon 09:00",
        "cua agent routine add ada --title Sweep --prompt triage --every 60",
        "cua agent routine ls",
        "cua agent routine list",
        "cua agent routine disable 7EDDD0EB-5408-49F6-8890-F89AA12532A1",
        "cua agent notifications --unread",
        "cua agent notifications --mark-read",
        "cua agent pause ada",
        "cua agent resume ada --prompt go",
        "cua agent save ada",
        "cua agent forget ada",
        "cua agent run local:dev claude-code fix -f --exit-when-idle",
        "cua agent ls local:dev",
        "cua agent list local:dev",
        "cua agent logs local:dev run-1 -f",
        "cua --json agent persistent",
        "cua volume ls agents/",
        "cua volume cat agents/ada/memory/MEMORY.md",
    ] {
        let mut argv: Vec<&str> = line.split(' ').collect();
        // `--weekly` takes one value, `mon 09:00`.
        if let Some(i) = argv.iter().position(|a| *a == "mon") {
            argv[i] = "mon 09:00";
            argv.remove(i + 1);
        }
        if let Err(e) = Cli::try_parse_from(&argv) {
            panic!("`{line}` no longer parses: {e}");
        }
    }
}

/// Every tool `cua daemon mcp` listed in 0.7.0 is still served: by this
/// server, by the Spaces contract, or as a former name of a contract tool.
#[test]
fn every_070_mcp_tool_name_is_still_served() {
    let own = crate::mcp::own_tool_names();
    let contract: Vec<String> = cua_spaces::contract::tools()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    let old: Vec<&str> = MCP_TOOLS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    assert!(old.len() > 100, "{}", old.len());
    let gone: Vec<&str> = old
        .into_iter()
        .filter(|n| {
            let now = cua_spaces::contract::canonical(n);
            !own.contains(&now) && !contract.iter().any(|c| c == now)
        })
        .collect();
    assert!(
        gone.is_empty(),
        "0.7.0 MCP tools no longer served: {gone:?}"
    );
}
