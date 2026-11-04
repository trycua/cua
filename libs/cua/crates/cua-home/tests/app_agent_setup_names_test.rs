//! The apps register Cua's MCP server in a coding agent (Claude Code, Codex,
//! ...) without a name of their own: the SDK names it for the app's channel
//! (`CuaChannel::mcp_server_name`, `cua` or `cua-local`) and installs the
//! channel's skill copies (CUA-1776). An app that wrote `cua` itself would
//! have Cua Spaces Local replace the released app's entry in the user's
//! `~/.claude.json` on "Continue".
//!
//! The SDK's record defaults its name to `cua` (`AgentMcpServer`), which a
//! caller that leaves it out gets, so every caller says `name: ""` out loud.
//! The behaviour behind it is tested where it lives: an empty name is the
//! channel's (`cua-sdk`'s `agent_setup` tests), and a Local setup leaves
//! Stable's entry alone (`cua-agent-setup`'s `local_channel` test).
//!
//! The Swift and Electron sources are read straight from the repository; the
//! check is skipped where the apps are not there (the crate published on
//! crates.io).

use cua_home::channel::CuaChannel;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .join(rel)
}

fn swift_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            swift_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "swift") {
            out.push(p);
        }
    }
}

/// Every `AgentMcpServer(` the Swift app builds, with the text up to its
/// closing parenthesis.
fn swift_servers() -> Option<Vec<(PathBuf, String)>> {
    let root = repo("apps/cua-spaces-macos/Sources");
    if !root.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    swift_files(&root, &mut files);
    let mut found = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (at, _) in text.match_indices("AgentMcpServer(") {
            let call = &text[at..];
            let end = call.find(')').map_or(call.len(), |i| i + 1);
            found.push((f.clone(), call[..end].to_string()));
        }
    }
    Some(found)
}

#[test]
fn the_swift_app_leaves_the_mcp_server_name_to_the_channel() {
    let Some(servers) = swift_servers() else {
        eprintln!("SKIP: no apps/cua-spaces-macos in this checkout");
        return;
    };
    // The app builds one server (`LiveAgentSetup`); a second one must say the same.
    assert!(
        !servers.is_empty(),
        "no AgentMcpServer in the Swift app: did LiveAgentSetup move? Update this test"
    );
    for (file, call) in &servers {
        assert!(
            call.contains("name: \"\""),
            "{} builds `{call}`: pass `name: \"\"` so the SDK names the server for the channel \
             (`cua` or `cua-local`); the SDK's default, `cua`, is Stable's",
            file.display()
        );
    }
}

#[test]
fn the_electron_app_leaves_the_mcp_server_name_to_the_channel() {
    let Ok(ts) = std::fs::read_to_string(repo("apps/cua-spaces-desktop/src/model/agents.ts"))
    else {
        eprintln!("SKIP: no apps/cua-spaces-desktop in this checkout");
        return;
    };
    assert!(
        ts.contains("{ name: \"\", command: cuaBinary,"),
        "LiveAgentSetup no longer builds its server with an empty name: the SDK names it for the \
         channel (`cua` or `cua-local`)"
    );
    for stable in [
        format!("name: \"{}\"", CuaChannel::Stable.mcp_server_name()),
        format!("name: \"{}\"", CuaChannel::Local.mcp_server_name()),
    ] {
        assert!(
            !ts.contains(&stable),
            "agents.ts names the MCP server itself ({stable}); the channel does that"
        );
    }
}
