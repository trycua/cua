//! `cua dump-docs` (hidden): the CLI and MCP surface as JSON, for the
//! generated reference (`scripts/docs-generators/cua-cli.ts`) and the docs
//! CLI-shape lane, which validates every `cua ...` command in the docs
//! against it.
//!
//! The CLI half walks the clap definition, so it cannot drift from the
//! parser. Its schema matches `cua-driver dump-docs --type cli` (name,
//! version, abstract, commands with arguments / options / flags /
//! subcommands), plus what a shape check needs: aliases, hidden commands,
//! global options, possible values, env vars and repeatability. The MCP half
//! is the `cua mcp` tool list: the Spaces contract tools (with providers,
//! metering and SDK symbols) and the sandbox / computer / skills tools.
//!
//! Output is deterministic (no paths, dates or host facts).

use clap::{Arg, ArgAction, Command as ClapCommand, CommandFactory};
use serde_json::{Value, json};

/// `cli`, `mcp` or `all` (`{"cli": ..., "mcp": ...}`).
pub fn dump(kind: &str) -> Result<Value, String> {
    match kind {
        "cli" => Ok(cli()),
        "mcp" => Ok(crate::mcp::docs()),
        "all" => Ok(json!({"cli": cli(), "mcp": crate::mcp::docs()})),
        other => Err(format!("unknown --type {other} (cli, mcp or all)")),
    }
}

/// The CLI definition as JSON.
pub fn cli() -> Value {
    // Built, so subcommands know their full bin name for `usage`.
    let mut root = crate::Cli::command();
    root.build();
    let global_options: Vec<Value> = root
        .get_arguments()
        .filter(|a| !a.is_positional() && !is_builtin(a))
        .map(option)
        .collect();
    let exit_codes: Vec<Value> = crate::EXIT_CODES
        .iter()
        .map(|(code, meaning)| json!({"code": code, "meaning": meaning}))
        .collect();
    json!({
        "name": root.get_name(),
        "version": cua_sdk::VERSION,
        "abstract": text(root.get_about()),
        "usage": "cua [OPTIONS] <COMMAND>",
        "global_options": global_options,
        "exit_codes": exit_codes,
        "commands": subcommands(&root),
    })
}

fn subcommands(cmd: &ClapCommand) -> Vec<Value> {
    cmd.get_subcommands()
        .filter(|c| c.get_name() != "help")
        .map(command)
        .collect()
}

fn command(cmd: &ClapCommand) -> Value {
    let args: Vec<&Arg> = cmd
        .get_arguments()
        .filter(|a| !is_builtin(a) && !a.is_global_set())
        .collect();
    let (flags, options): (Vec<&Arg>, Vec<&Arg>) = args
        .iter()
        .filter(|a| !a.is_positional())
        .partition(|a| takes_no_value(a));
    let about = text(cmd.get_about());
    let long = text(cmd.get_long_about());
    let usage = cmd.clone().render_usage().to_string();
    let mut v = json!({
        "name": cmd.get_name(),
        "abstract": about,
        "usage": usage.trim().trim_start_matches("Usage:").trim(),
        "aliases": cmd.get_visible_aliases().collect::<Vec<_>>(),
        "hidden_aliases": cmd
            .get_all_aliases()
            .filter(|a| !cmd.get_visible_aliases().any(|v| v == *a))
            .collect::<Vec<_>>(),
        "hidden": cmd.is_hide_set(),
        "arguments": args
            .iter()
            .filter(|a| a.is_positional())
            .map(|a| positional(a))
            .collect::<Vec<_>>(),
        "options": options.iter().map(|a| option(a)).collect::<Vec<_>>(),
        "flags": flags.iter().map(|a| flag(a)).collect::<Vec<_>>(),
        "subcommands": subcommands(cmd),
        // A group that also runs without a subcommand (`cua doctor [REF]`).
        "subcommand_optional": cmd.has_subcommands() && !cmd.is_subcommand_required_set(),
    });
    // `after_help` carries the command's `Examples:` (the docs render them).
    let after = text(cmd.get_after_long_help());
    let after = if after.is_empty() {
        text(cmd.get_after_help())
    } else {
        after
    };
    if !after.is_empty() {
        v["after_help"] = json!(after);
    }
    // The long help repeats the abstract as its first paragraph.
    if let Some(rest) = long.strip_prefix(&about).map(str::trim)
        && !rest.is_empty()
    {
        v["discussion"] = json!(rest);
    } else if !long.is_empty() && long != about {
        v["discussion"] = json!(long);
    }
    v
}

fn is_builtin(a: &Arg) -> bool {
    matches!(
        a.get_action(),
        ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong
    ) || (a.get_id() == "version" && matches!(a.get_action(), ArgAction::Version))
}

fn takes_no_value(a: &Arg) -> bool {
    matches!(
        a.get_action(),
        ArgAction::SetTrue
            | ArgAction::SetFalse
            | ArgAction::Count
            | ArgAction::Version
            | ArgAction::Help
    )
}

fn text(s: Option<&clap::builder::StyledStr>) -> String {
    s.map(|s| s.to_string().trim().to_string())
        .unwrap_or_default()
}

fn help(a: &Arg) -> String {
    let h = text(a.get_long_help());
    if h.is_empty() { text(a.get_help()) } else { h }
}

/// The values an argument accepts, when it is a closed set.
fn possible_values(a: &Arg) -> Vec<String> {
    a.get_possible_values()
        .iter()
        .filter(|p| !p.is_hide_set())
        .map(|p| p.get_name().to_string())
        .collect()
}

/// The argument's value type, from its value parser (`string` when the
/// parser is custom), or its possible values joined with ` | `.
fn value_type(a: &Arg) -> String {
    use std::any::TypeId;
    let values = possible_values(a);
    if !values.is_empty() {
        return values.join(" | ");
    }
    let id = a.get_value_parser().type_id();
    let is = |t: TypeId| id == t;
    if [
        TypeId::of::<u8>(),
        TypeId::of::<u16>(),
        TypeId::of::<u32>(),
        TypeId::of::<u64>(),
        TypeId::of::<usize>(),
        TypeId::of::<i32>(),
        TypeId::of::<i64>(),
    ]
    .into_iter()
    .any(is)
    {
        "integer".into()
    } else if is(TypeId::of::<f32>()) || is(TypeId::of::<f64>()) {
        "number".into()
    } else if is(TypeId::of::<std::path::PathBuf>()) {
        "path".into()
    } else if is(TypeId::of::<bool>()) {
        "boolean".into()
    } else {
        "string".into()
    }
}

/// The placeholder `--help` shows (`NAME`, `KEY=VALUE`).
fn value_name(a: &Arg) -> String {
    a.get_value_names()
        .map(|n| {
            n.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| a.get_id().as_str().to_uppercase().replace('-', "_"))
}

fn repeatable(a: &Arg) -> bool {
    matches!(a.get_action(), ArgAction::Append | ArgAction::Count)
        || a.get_num_args().is_some_and(|n| n.max_values() > 1)
}

fn default(a: &Arg) -> Value {
    let d: Vec<String> = a
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    if d.is_empty() || (d.len() == 1 && d[0].is_empty()) {
        Value::Null
    } else {
        json!(d.join(","))
    }
}

fn positional(a: &Arg) -> Value {
    json!({
        "name": a.get_value_names().and_then(|n| n.first()).map(|s| s.to_string())
            .unwrap_or_else(|| a.get_id().as_str().to_string()),
        "help": help(a),
        "type": value_type(a),
        "possible_values": possible_values(a),
        "default_value": default(a),
        "is_optional": !a.is_required_set(),
        "repeatable": repeatable(a),
        "hidden": a.is_hide_set(),
    })
}

fn option(a: &Arg) -> Value {
    if takes_no_value(a) {
        return flag(a);
    }
    json!({
        "name": a.get_long().unwrap_or_default(),
        "short_name": a.get_short().map(|c| c.to_string()),
        "aliases": aliases(a),
        "help": help(a),
        "type": value_type(a),
        "value_name": value_name(a),
        "possible_values": possible_values(a),
        "default_value": default(a),
        "is_optional": !a.is_required_set(),
        "repeatable": repeatable(a),
        // One occurrence takes several values (`--x a b`).
        "multiple_values": a.get_num_args().is_some_and(|n| n.max_values() > 1),
        "env": a.get_env().map(|e| e.to_string_lossy().into_owned()),
        "hidden": a.is_hide_set(),
        "takes_value": true,
    })
}

fn flag(a: &Arg) -> Value {
    json!({
        "name": a.get_long().unwrap_or_default(),
        "short_name": a.get_short().map(|c| c.to_string()),
        "aliases": aliases(a),
        "help": help(a),
        "default_value": false,
        "repeatable": repeatable(a),
        "env": a.get_env().map(|e| e.to_string_lossy().into_owned()),
        "hidden": a.is_hide_set(),
        "takes_value": false,
    })
}

fn aliases(a: &Arg) -> Vec<String> {
    let mut out: Vec<String> = a
        .get_all_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(str::to_string)
        .collect();
    out.extend(
        a.get_all_short_aliases()
            .unwrap_or_default()
            .into_iter()
            .map(|c| format!("-{c}")),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(cmds: &'a [Value], name: &str) -> &'a Value {
        cmds.iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no command {name}"))
    }

    #[test]
    fn cli_dump_describes_the_parser() {
        let v = cli();
        assert_eq!(v["name"], "cua");
        let globals: Vec<&str> = v["global_options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["name"].as_str().unwrap())
            .collect();
        for g in ["embedded", "daemon", "json", "state-dir"] {
            assert!(globals.contains(&g), "{g} missing from {globals:?}");
        }
        let cmds = v["commands"].as_array().unwrap();
        let sandbox = find(cmds, "sandbox");
        assert_eq!(sandbox["aliases"], json!(["sb"]));
        assert!(!sandbox["subcommands"].as_array().unwrap().is_empty());
        // `spacesd` carries no aliases (no `env` or `guest`).
        assert_eq!(find(cmds, "spacesd")["hidden_aliases"], json!([]));
        // Hidden commands are listed, marked hidden (the shape check needs them).
        assert_eq!(find(cmds, "dump-docs")["hidden"], true);
        let skills = find(cmds, "skills")["subcommands"]
            .as_array()
            .unwrap()
            .clone();
        let read = find(&skills, "read");
        let format = &read["options"][0];
        assert_eq!(format["name"], "format");
        assert_eq!(format["short_name"], "f");
        assert_eq!(format["type"], "md | json");
        assert_eq!(format["default_value"], "md");
        assert_eq!(read["arguments"][0]["is_optional"], false);
        // Deterministic.
        assert_eq!(cli(), v);
    }

    /// Shell words of one example line: whitespace-split, with single and
    /// double quotes (backslash escapes a double-quoted `"`).
    fn shell_words(line: &str) -> Vec<String> {
        let (mut words, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None, false);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match (quote, c) {
                (None, '\'' | '"') => {
                    quote = Some(c);
                    any = true;
                }
                (Some(q), c) if c == q => quote = None,
                (Some('"'), '\\') if chars.peek() == Some(&'"') => cur.push(chars.next().unwrap()),
                (None, c) if c.is_whitespace() => {
                    if any || !cur.is_empty() {
                        words.push(std::mem::take(&mut cur));
                        any = false;
                    }
                }
                (_, c) => cur.push(c),
            }
        }
        assert!(quote.is_none(), "unbalanced quote in {line:?}");
        if any || !cur.is_empty() {
            words.push(cur);
        }
        words
    }

    /// Every visible command's `after_help` examples parse, and every
    /// visible leaf command has at least one.
    #[test]
    fn help_examples_parse_and_cover_every_leaf() {
        use clap::{CommandFactory, Parser};
        fn walk(cmd: &ClapCommand, path: &str, missing: &mut Vec<String>, bad: &mut Vec<String>) {
            for sub in cmd
                .get_subcommands()
                .filter(|c| c.get_name() != "help" && !c.is_hide_set())
            {
                let p = format!("{path} {}", sub.get_name());
                let after = sub
                    .get_after_long_help()
                    .or(sub.get_after_help())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let examples: Vec<&str> = after
                    .lines()
                    .skip_while(|l| l.trim() != "Examples:")
                    .skip(1)
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect();
                let leaf = sub
                    .get_subcommands()
                    .all(|c| c.get_name() == "help" || c.is_hide_set());
                if leaf && examples.is_empty() {
                    missing.push(p.clone());
                }
                for ex in examples {
                    // Drop a trailing shell redirect (`> pool.tf`).
                    let ex = ex.split(" > ").next().unwrap();
                    let words = shell_words(ex);
                    if words.first().map(String::as_str) != Some("cua") {
                        bad.push(format!("{p}: {ex:?} does not start with `cua`"));
                        continue;
                    }
                    if let Err(e) = crate::Cli::try_parse_from(&words) {
                        bad.push(format!("{p}: {ex:?}: {}", e.kind()));
                    }
                }
                walk(sub, &p, missing, bad);
            }
        }
        let (mut missing, mut bad) = (Vec::new(), Vec::new());
        walk(&crate::Cli::command(), "cua", &mut missing, &mut bad);
        assert!(
            missing.is_empty(),
            "leaf commands without examples: {missing:#?}"
        );
        assert!(bad.is_empty(), "examples that do not parse: {bad:#?}");
    }

    #[test]
    fn shell_words_split_quotes() {
        assert_eq!(
            shell_words(r#"cua sb exec dev 'ls /tmp | wc -l' "a \"b\"""#),
            ["cua", "sb", "exec", "dev", "ls /tmp | wc -l", "a \"b\""]
        );
    }

    #[test]
    fn mcp_dump_lists_contract_and_extension_tools() {
        let v = dump("mcp").unwrap();
        let tools = v["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"add_space"));
        assert!(names.contains(&"sandbox_list"));
        let add = tools.iter().find(|t| t["name"] == "add_space").unwrap();
        assert_eq!(add["group"], "spaces");
        assert!(add["input_schema"]["properties"].is_object());
        let list = tools.iter().find(|t| t["name"] == "sandbox_list").unwrap();
        assert_eq!(list["permission"], "sandbox:list");
        assert!(dump("nope").is_err());
    }
}
