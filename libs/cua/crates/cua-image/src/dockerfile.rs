//! Dockerfile parser: the one grammar every Cua build surface shares
//! (`cua build`, `Spaces.build`, the compat layers). Pure (no I/O), so it is
//! unit-tested, golden-tested against what the Modal and Daytona SDKs
//! generate, and fuzzed.
//!
//! A Dockerfile here is one stage: any `ARG` lines, one `FROM`, then the
//! instructions. Supported, following the Dockerfile reference: line
//! continuations, comments, the `escape` parser directive, `ARG` (before and
//! after `FROM`), `FROM [--platform=..] ref [AS name]`, `RUN` (shell and exec
//! form), `ENV`, `WORKDIR`, `USER`, `COPY` / `ADD` (`--chown`, `--chmod`;
//! local sources only), `CMD`, `ENTRYPOINT`, `SHELL`, `LABEL`, `MAINTAINER`,
//! `EXPOSE`, `VOLUME`, `STOPSIGNAL`, `HEALTHCHECK`, quoting, and `$VAR`,
//! `${VAR}`, `${VAR:-x}`, `${VAR:+x}` expansion ([`expand`]).
//!
//! What a Space cannot build is refused with the line and the instruction
//! named, never skipped: a second `FROM` (multi-stage), `COPY --from`,
//! `RUN --mount/--network/--security`, `ONBUILD`, heredocs, `ADD` of a URL or
//! git reference, and flags that change what is copied.

use std::collections::BTreeMap;

/// A parsed single-stage Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dockerfile {
    /// `ARG` lines before `FROM` (they feed only the `FROM` line, and are
    /// not visible in the stage until it declares them again).
    pub global_args: Vec<ArgDecl>,
    /// The `FROM` line.
    pub from: FromStage,
    /// Line of the `FROM`.
    pub from_line: usize,
    /// Every instruction after `FROM`, in order, with its first line.
    pub instructions: Vec<Located>,
}

/// An instruction and the line it starts on (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// First physical line of the instruction.
    pub line: usize,
    /// The instruction.
    pub instr: Instr,
}

/// `FROM [--platform=p] reference [AS stage]`. The reference is as written
/// (it may hold `${ARG}`; see [`Dockerfile::from_reference`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FromStage {
    /// Image reference as written.
    pub reference: String,
    /// `--platform`, as written.
    pub platform: Option<String>,
    /// `AS name` (lowercased).
    pub stage: Option<String>,
}

/// One `ARG name[=default]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgDecl {
    /// Variable name.
    pub name: String,
    /// Default value, as written (unexpanded).
    pub default: Option<String>,
}

/// One instruction of the stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instr {
    /// `RUN`.
    Run(Exec),
    /// `ENV k=v ...` (values unexpanded).
    Env(Vec<(String, String)>),
    /// `ARG a b=c ...`.
    Arg(Vec<ArgDecl>),
    /// `WORKDIR path` (unexpanded).
    Workdir(String),
    /// `USER user[:group]` (unexpanded).
    User(String),
    /// `COPY` / `ADD`.
    Copy(CopyInstr),
    /// `CMD`.
    Cmd(Exec),
    /// `ENTRYPOINT`.
    Entrypoint(Exec),
    /// `SHELL ["exe", "param"]`.
    Shell(Vec<String>),
    /// `LABEL` / `MAINTAINER`.
    Label(Vec<(String, String)>),
    /// `EXPOSE port[/proto] ...`.
    Expose(Vec<String>),
    /// `VOLUME`.
    Volume(Vec<String>),
    /// `STOPSIGNAL`.
    StopSignal(String),
    /// `HEALTHCHECK ...` (the text after the keyword).
    Healthcheck(String),
}

impl Instr {
    /// The instruction's keyword, as Docker spells it.
    pub fn keyword(&self) -> &'static str {
        match self {
            Instr::Run(_) => "RUN",
            Instr::Env(_) => "ENV",
            Instr::Arg(_) => "ARG",
            Instr::Workdir(_) => "WORKDIR",
            Instr::User(_) => "USER",
            Instr::Copy(c) if c.add => "ADD",
            Instr::Copy(_) => "COPY",
            Instr::Cmd(_) => "CMD",
            Instr::Entrypoint(_) => "ENTRYPOINT",
            Instr::Shell(_) => "SHELL",
            Instr::Label(_) => "LABEL",
            Instr::Expose(_) => "EXPOSE",
            Instr::Volume(_) => "VOLUME",
            Instr::StopSignal(_) => "STOPSIGNAL",
            Instr::Healthcheck(_) => "HEALTHCHECK",
        }
    }
}

/// A command: a shell line or an argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exec {
    /// Shell form (run by the stage's `SHELL`).
    Shell(String),
    /// Exec form (`["a", "b"]`).
    Argv(Vec<String>),
}

/// `COPY` / `ADD` sources and destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyInstr {
    /// `ADD` (a local tar archive is unpacked) rather than `COPY`.
    pub add: bool,
    /// `--chown`, unexpanded.
    pub chown: Option<String>,
    /// `--chmod`, as written.
    pub chmod: Option<String>,
    /// Source paths (relative to the build context), unexpanded.
    pub sources: Vec<String>,
    /// Destination, unexpanded.
    pub dest: String,
}

/// A refused or malformed Dockerfile, with the line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Dockerfile line {line}: {message}")]
pub struct ParseError {
    /// 1-based line (0: the file as a whole).
    pub line: usize,
    /// What is wrong and, when refused, what to do instead.
    pub message: String,
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        line,
        message: message.into(),
    })
}

/// Logical lines: continuations joined, comments and blanks dropped, with
/// the 1-based number of each one's first physical line. The `escape`
/// parser directive (`# escape=\``) is honoured.
pub fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    let mut escape = '\\';
    let mut directives_open = true;
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        // Parser directives: `# key=value` comments before anything else.
        if directives_open {
            let directive = trimmed
                .strip_prefix('#')
                .and_then(|d| d.split_once('='))
                .filter(|(k, _)| {
                    let k = k.trim();
                    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphabetic())
                });
            match directive {
                Some((k, v)) => {
                    if k.trim().eq_ignore_ascii_case("escape")
                        && let Some(c) = v.trim().chars().next()
                        && matches!(c, '\\' | '`')
                    {
                        escape = c;
                    }
                    continue;
                }
                None => directives_open = false,
            }
        }
        if trimmed.starts_with('#') {
            continue; // a comment line, even inside a continuation
        }
        if trimmed.is_empty() {
            continue; // blank lines end nothing, even inside a continuation
        }
        if cur.is_empty() {
            start = n;
        }
        let body = line.trim_end();
        if let Some(head) = body.strip_suffix(escape) {
            cur.push_str(head);
            continue;
        }
        cur.push_str(line);
        out.push((start, std::mem::take(&mut cur)));
    }
    if !cur.trim().is_empty() {
        out.push((start, cur));
    }
    out
}

/// Splits a word list on unquoted whitespace, removing quotes and
/// backslash escapes.
pub fn words(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            '\'' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.peek() {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                cur.push(n);
                                chars.next();
                            }
                            _ => cur.push('\\'),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                have = true;
                match chars.next() {
                    Some(n) => cur.push(n),
                    None => cur.push('\\'),
                }
            }
            c => {
                have = true;
                cur.push(c);
            }
        }
    }
    if have {
        out.push(cur);
    }
    Ok(out)
}

/// Quotes `s` as one POSIX shell word.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,@%+".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A JSON array of strings (the exec form), if `s` is one.
fn json_argv(s: &str) -> Option<Vec<String>> {
    let t = s.trim();
    if !t.starts_with('[') {
        return None;
    }
    serde_json::from_str::<Vec<String>>(t).ok()
}

fn exec(s: &str) -> Exec {
    match json_argv(s) {
        Some(v) => Exec::Argv(v),
        None => Exec::Shell(s.trim().to_string()),
    }
}

/// Leading `--flag=value` / `--flag` words.
fn flags(rest: &str) -> (Vec<(String, String)>, &str) {
    let mut fl = Vec::new();
    let mut s = rest.trim_start();
    while s.starts_with("--") && s.as_bytes().get(2).is_some_and(u8::is_ascii_alphabetic) {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        let tok = &s[2..end];
        let (k, v) = tok.split_once('=').unwrap_or((tok, ""));
        fl.push((k.to_ascii_lowercase(), v.to_string()));
        s = s[end..].trim_start();
    }
    (fl, s)
}

/// `KEY=value KEY2="v 2"` or the legacy `KEY value with spaces`.
fn key_values(line: usize, rest: &str, what: &str) -> Result<Vec<(String, String)>, ParseError> {
    let ws = words(rest).or_else(|m| err(line, m))?;
    if ws.is_empty() {
        return err(line, format!("{what} needs a name and a value"));
    }
    if !ws[0].contains('=') {
        // `ENV KEY some value`
        let (k, v) = rest
            .trim_start()
            .split_once(char::is_whitespace)
            .ok_or_else(|| ParseError {
                line,
                message: format!("{what} {} needs a value", ws[0]),
            })?;
        let v = words(v).or_else(|m| err(line, m))?.join(" ");
        return Ok(vec![(k.to_string(), v)]);
    }
    ws.into_iter()
        .map(|w| match w.split_once('=') {
            Some((k, v)) if !k.is_empty() => Ok((k.to_string(), v.to_string())),
            _ => err(line, format!("{what}: expected KEY=value, got {w:?}")),
        })
        .collect()
}

fn arg_decls(line: usize, rest: &str) -> Result<Vec<ArgDecl>, ParseError> {
    let ws = words(rest).or_else(|m| err(line, m))?;
    if ws.is_empty() {
        return err(line, "ARG needs a name");
    }
    ws.into_iter()
        .map(|w| match w.split_once('=') {
            Some(("", _)) => err(line, format!("ARG: expected NAME[=default], got {w:?}")),
            Some((k, v)) => Ok(ArgDecl {
                name: k.to_string(),
                default: Some(v.to_string()),
            }),
            None => Ok(ArgDecl {
                name: w,
                default: None,
            }),
        })
        .collect()
}

/// Whether `rest` opens a heredoc (`<<EOF`, `<<-EOF`, `<<"EOF"`).
fn has_heredoc(rest: &str) -> bool {
    rest.match_indices("<<").any(|(i, _)| {
        let after = rest[i + 2..].trim_start_matches('-');
        // `1<<2` and `<<<` (a here-string) are shell, not a heredoc.
        after.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '"' || c == '\'')
            && !rest[..i].ends_with('<')
    })
}

const COPY_FLAGS_IGNORED: &[&str] = &["link"];

/// Parses Dockerfile text.
pub fn parse(text: &str) -> Result<Dockerfile, ParseError> {
    let mut global_args = Vec::new();
    let mut from: Option<(FromStage, usize)> = None;
    let mut instructions = Vec::new();
    for (n, line) in logical_lines(text) {
        let line = line.trim();
        let (kw, rest) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(a, b)| (a, b.trim_start()));
        let kw_up = kw.to_ascii_uppercase();
        if matches!(kw_up.as_str(), "RUN" | "COPY" | "ADD") && has_heredoc(rest) {
            return err(
                n,
                format!(
                    "{kw_up} with a heredoc is not supported; put the script in a file and COPY it, or join the commands with &&"
                ),
            );
        }
        if kw_up == "FROM" {
            if let Some((_, first)) = &from {
                return err(
                    n,
                    format!(
                        "a second FROM (line {first} was the first) makes a multi-stage build, which is not supported; build one stage and COPY its results in a separate build"
                    ),
                );
            }
            let (fl, body) = flags(rest);
            for (k, _) in &fl {
                if k != "platform" {
                    return err(n, format!("FROM --{k} is not supported"));
                }
            }
            let ws = words(body).or_else(|m| err(n, m))?;
            let platform = fl
                .into_iter()
                .find(|(k, _)| k == "platform")
                .map(|(_, v)| v);
            let f = match ws.as_slice() {
                [r] => FromStage {
                    reference: r.clone(),
                    stage: None,
                    platform,
                },
                [r, a, name] if a.eq_ignore_ascii_case("as") => FromStage {
                    reference: r.clone(),
                    stage: Some(name.to_ascii_lowercase()),
                    platform,
                },
                _ => return err(n, "FROM needs an image: FROM <image> [AS <name>]"),
            };
            from = Some((f, n));
            continue;
        }
        if kw_up == "ARG" && from.is_none() {
            global_args.extend(arg_decls(n, rest)?);
            continue;
        }
        if from.is_none() {
            return err(
                n,
                format!(
                    "{kw_up} before FROM; a Dockerfile starts with FROM (only ARG may come first)"
                ),
            );
        }
        let instr = match kw_up.as_str() {
            "RUN" => {
                let (fl, body) = flags(rest);
                if let Some((k, _)) = fl
                    .iter()
                    .find(|(k, _)| matches!(k.as_str(), "mount" | "network" | "security"))
                {
                    return err(
                        n,
                        format!("RUN --{k} is a BuildKit feature and is not supported"),
                    );
                }
                if let Some((k, _)) = fl.first() {
                    return err(n, format!("RUN --{k} is not supported"));
                }
                if body.is_empty() {
                    return err(n, "RUN needs a command");
                }
                Instr::Run(exec(body))
            }
            "ENV" => Instr::Env(key_values(n, rest, "ENV")?),
            "LABEL" => Instr::Label(key_values(n, rest, "LABEL")?),
            "ARG" => Instr::Arg(arg_decls(n, rest)?),
            "WORKDIR" => {
                if rest.is_empty() {
                    return err(n, "WORKDIR needs a path");
                }
                Instr::Workdir(rest.to_string())
            }
            "USER" => {
                if rest.is_empty() {
                    return err(n, "USER needs a user");
                }
                Instr::User(rest.to_string())
            }
            "COPY" | "ADD" => {
                let (fl, body) = flags(rest);
                let mut c = CopyInstr {
                    add: kw_up == "ADD",
                    chown: None,
                    chmod: None,
                    sources: Vec::new(),
                    dest: String::new(),
                };
                for (k, v) in fl {
                    match k.as_str() {
                        "from" => {
                            return err(
                                n,
                                format!(
                                    "{kw_up} --from={v} copies from another stage or image, which needs a multi-stage build; this build has one stage"
                                ),
                            );
                        }
                        "chown" => c.chown = Some(v),
                        "chmod" => c.chmod = Some(v),
                        k if COPY_FLAGS_IGNORED.contains(&k) => {}
                        other => {
                            return err(n, format!("{kw_up} --{other} is not supported"));
                        }
                    }
                }
                let mut ws = match json_argv(body) {
                    Some(v) => v,
                    None => words(body).or_else(|m| err(n, m))?,
                };
                if ws.len() < 2 {
                    return err(n, format!("{kw_up} needs a source and a destination"));
                }
                c.dest = ws.pop().expect("len >= 2");
                if let Some(s) = ws.iter().find(|s| {
                    s.contains("://")
                        || s.starts_with("git@")
                        || s.ends_with(".git#")
                        || s.contains(".git#")
                }) {
                    return err(
                        n,
                        format!(
                            "{kw_up} of {s:?} fetches from the network; only files of the build context can be copied (download in a RUN instead)"
                        ),
                    );
                }
                c.sources = ws;
                Instr::Copy(c)
            }
            "CMD" => Instr::Cmd(exec(rest)),
            "ENTRYPOINT" => Instr::Entrypoint(exec(rest)),
            "SHELL" => match json_argv(rest) {
                Some(v) if !v.is_empty() => Instr::Shell(v),
                _ => return err(n, "SHELL needs a JSON array: SHELL [\"/bin/bash\", \"-c\"]"),
            },
            "EXPOSE" => Instr::Expose(words(rest).or_else(|m| err(n, m))?),
            "VOLUME" => Instr::Volume(match json_argv(rest) {
                Some(v) => v,
                None => words(rest).or_else(|m| err(n, m))?,
            }),
            "STOPSIGNAL" => Instr::StopSignal(rest.to_string()),
            "HEALTHCHECK" => Instr::Healthcheck(rest.to_string()),
            "MAINTAINER" => Instr::Label(vec![("maintainer".into(), rest.to_string())]),
            "ONBUILD" => {
                return err(
                    n,
                    "ONBUILD is not supported (it sets triggers for images built FROM this one)",
                );
            }
            other => return err(n, format!("unknown instruction {other}")),
        };
        instructions.push(Located { line: n, instr });
    }
    let Some((from, from_line)) = from else {
        return err(0, "no FROM: a Dockerfile starts with FROM <image>");
    };
    Ok(Dockerfile {
        global_args,
        from,
        from_line,
        instructions,
    })
}

impl Dockerfile {
    /// The `FROM` reference with the global `ARG`s expanded: `args` (build
    /// arguments) win over the defaults the `ARG` lines declare.
    pub fn from_reference(&self, args: &BTreeMap<String, String>) -> String {
        let mut scope = BTreeMap::new();
        for a in &self.global_args {
            let v = args
                .get(&a.name)
                .cloned()
                .or_else(|| a.default.as_ref().map(|d| expand(d, &scope)));
            if let Some(v) = v {
                scope.insert(a.name.clone(), v);
            }
        }
        expand(&self.from.reference, &scope)
    }
}

/// Expands `$VAR`, `${VAR}`, `${VAR:-d}`, `${VAR-d}`, `${VAR:+a}`,
/// `${VAR+a}` against `env`; `\$` is a literal dollar. Unset variables
/// expand to nothing.
pub fn expand(s: &str, env: &BTreeMap<String, String>) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' if i + 1 < cs.len() && cs[i + 1] == '$' => {
                out.push('$');
                i += 2;
            }
            '$' if i + 1 < cs.len() && cs[i + 1] == '{' => {
                // The matching brace (a default may hold `${...}` itself).
                let mut depth = 1usize;
                let close = cs[i + 2..].iter().position(|&c| {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                });
                let Some(close) = close else {
                    out.push('$');
                    i += 1;
                    continue;
                };
                let body: String = cs[i + 2..i + 2 + close].iter().collect();
                i += close + 3;
                let (name, op) = match body.find([':', '-', '+']) {
                    Some(p) => (&body[..p], &body[p..]),
                    None => (body.as_str(), ""),
                };
                let val = env.get(name);
                let (colon, rest) = match op.strip_prefix(':') {
                    Some(r) => (true, r),
                    None => (false, op),
                };
                let set = |v: Option<&String>| match v {
                    Some(v) => !colon || !v.is_empty(),
                    None => false,
                };
                if let Some(d) = rest.strip_prefix('-') {
                    out.push_str(&if set(val) {
                        val.cloned().unwrap_or_default()
                    } else {
                        expand(d, env)
                    });
                } else if let Some(a) = rest.strip_prefix('+') {
                    if set(val) {
                        out.push_str(&expand(a, env));
                    }
                } else if let Some(v) = val {
                    out.push_str(v);
                }
            }
            '$' if i + 1 < cs.len() && (cs[i + 1].is_ascii_alphabetic() || cs[i + 1] == '_') => {
                let mut j = i + 1;
                while j < cs.len() && (cs[j].is_ascii_alphanumeric() || cs[j] == '_') {
                    j += 1;
                }
                let name: String = cs[i + 1..j].iter().collect();
                if let Some(v) = env.get(&name) {
                    out.push_str(v);
                }
                i = j;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn instrs(t: &str) -> Vec<Instr> {
        parse(t)
            .unwrap()
            .instructions
            .into_iter()
            .map(|l| l.instr)
            .collect()
    }

    fn refused(t: &str) -> ParseError {
        parse(t).expect_err(t)
    }

    #[test]
    fn parses_the_lines_modal_generates() {
        // Image.debian_slim("3.12").apt_install(...).run_commands(...)
        let d = parse(
            "FROM python:3.12-slim-bookworm\nRUN apt-get update\nRUN apt-get install -y gcc gfortran build-essential\nRUN echo 'debconf debconf/frontend select Noninteractive' | debconf-set-selections\nCMD [\"sleep\", \"172800\"]",
        )
        .unwrap();
        assert_eq!(d.from.reference, "python:3.12-slim-bookworm");
        assert_eq!(d.from_line, 1);
        assert_eq!(
            d.instructions[0],
            Located {
                line: 2,
                instr: Instr::Run(Exec::Shell("apt-get update".into()))
            }
        );
        assert_eq!(
            d.instructions[3].instr,
            Instr::Cmd(Exec::Argv(vec!["sleep".into(), "172800".into()]))
        );
    }

    #[test]
    fn parses_the_lines_daytona_generates() {
        // Image.debian_slim("3.12").pip_install("numpy").env(..).workdir(..)
        let t = "FROM python:3.12-slim-bookworm\nRUN python -m pip install numpy\nENV A='b c'\nWORKDIR /w\nCOPY home/u/f.txt /w/f.txt\nENTRYPOINT [\"sleep\", \"infinity\"]\n";
        let p = instrs(t);
        assert_eq!(p.len(), 5);
        assert_eq!(p[1], Instr::Env(vec![("A".into(), "b c".into())]));
        assert_eq!(p[2], Instr::Workdir("/w".into()));
        let Instr::Copy(c) = &p[3] else { panic!() };
        assert_eq!(c.sources, ["home/u/f.txt"]);
        assert_eq!(c.dest, "/w/f.txt");
        assert_eq!(
            p[4],
            Instr::Entrypoint(Exec::Argv(vec!["sleep".into(), "infinity".into()]))
        );
    }

    #[test]
    fn joins_continuations_and_skips_comments() {
        let d = parse(
            "# syntax=docker/dockerfile:1\nFROM a\n# note\nRUN a \\\n  # inner comment\n\n  && b\n\nENV X=1",
        )
        .unwrap();
        assert_eq!(d.instructions.len(), 2);
        assert_eq!(
            d.instructions[0].instr,
            Instr::Run(Exec::Shell("a   && b".into()))
        );
        assert_eq!(d.instructions[0].line, 4);
        assert_eq!(d.instructions[1].line, 9);
    }

    #[test]
    fn trailing_whitespace_after_the_escape_still_continues() {
        assert_eq!(
            instrs("FROM a\nRUN a \\  \n b"),
            vec![Instr::Run(Exec::Shell("a  b".into()))]
        );
    }

    #[test]
    fn escape_directive_changes_the_continuation_character() {
        let d = parse("# escape=`\nFROM a\nRUN echo a `\n b \\\nENV X=1").unwrap();
        assert_eq!(d.instructions.len(), 2);
        assert_eq!(
            d.instructions[0].instr,
            Instr::Run(Exec::Shell("echo a  b \\".into()))
        );
    }

    #[test]
    fn env_forms() {
        assert_eq!(
            instrs("FROM a\nENV A=1 B=\"two words\" C=x\\ y"),
            vec![Instr::Env(vec![
                ("A".into(), "1".into()),
                ("B".into(), "two words".into()),
                ("C".into(), "x y".into())
            ])]
        );
        assert_eq!(
            instrs("FROM a\nENV MSG hello world"),
            vec![Instr::Env(vec![("MSG".into(), "hello world".into())])]
        );
    }

    #[test]
    fn arg_before_and_after_from() {
        let d = parse("ARG V=3.12 BASE\nFROM python:${V}\nARG X\nARG Y=2 Z=3").unwrap();
        assert_eq!(d.global_args.len(), 2);
        assert_eq!(d.global_args[0].default.as_deref(), Some("3.12"));
        assert_eq!(d.global_args[1].default, None);
        assert_eq!(d.from_reference(&BTreeMap::new()), "python:3.12");
        assert_eq!(d.from_reference(&env(&[("V", "3.9")])), "python:3.9");
        assert_eq!(d.instructions.len(), 2);
        let Instr::Arg(a) = &d.instructions[1].instr else {
            panic!()
        };
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn from_flags_and_stage() {
        let d = parse("FROM --platform=linux/amd64 golang:1.22 AS Build").unwrap();
        assert_eq!(d.from.platform.as_deref(), Some("linux/amd64"));
        assert_eq!(d.from.stage.as_deref(), Some("build"));
    }

    #[test]
    fn copy_flags_and_json_form() {
        let p = instrs("FROM a\nCOPY --chown=1:2 --chmod=755 /a /b /dst/\nADD [\"x y\", \"/z\"]");
        let Instr::Copy(c) = &p[0] else { panic!() };
        assert_eq!(c.chown.as_deref(), Some("1:2"));
        assert_eq!(c.chmod.as_deref(), Some("755"));
        assert_eq!(c.sources, ["/a", "/b"]);
        assert_eq!(c.dest, "/dst/");
        let Instr::Copy(a) = &p[1] else { panic!() };
        assert!(a.add);
        assert_eq!(a.sources, ["x y"]);
        assert_eq!(p[1].keyword(), "ADD");
    }

    #[test]
    fn exec_and_shell_forms() {
        let p = instrs(
            "FROM a\nRUN [\"echo\", \"a b\"]\nRUN echo [not json\nSHELL [\"/bin/bash\", \"-c\"]",
        );
        assert_eq!(
            p[0],
            Instr::Run(Exec::Argv(vec!["echo".into(), "a b".into()]))
        );
        assert_eq!(p[1], Instr::Run(Exec::Shell("echo [not json".into())));
        assert_eq!(p[2], Instr::Shell(vec!["/bin/bash".into(), "-c".into()]));
    }

    #[test]
    fn metadata_instructions() {
        let p = instrs(
            "FROM a\nLABEL a=1 b=\"x y\"\nMAINTAINER me <me@x>\nEXPOSE 80 443/tcp\nVOLUME [\"/a\", \"/b\"]\nVOLUME /c /d\nSTOPSIGNAL SIGTERM\nHEALTHCHECK CMD true\nUSER app:app\nWORKDIR $HOME/x",
        );
        assert_eq!(p.len(), 9);
        assert_eq!(p[2], Instr::Expose(vec!["80".into(), "443/tcp".into()]));
        assert_eq!(p[3], Instr::Volume(vec!["/a".into(), "/b".into()]));
        assert_eq!(p[4], Instr::Volume(vec!["/c".into(), "/d".into()]));
        assert_eq!(p[7], Instr::User("app:app".into()));
    }

    #[test]
    fn refuses_what_it_cannot_build_and_names_the_line() {
        let cases: &[(&str, usize, &str)] = &[
            ("FROM a\nFROM b", 2, "multi-stage"),
            ("FROM a\nRUN x\nCOPY --from=b /x /y", 3, "multi-stage"),
            ("FROM a\nONBUILD RUN x", 2, "ONBUILD"),
            (
                "FROM a\nRUN --mount=type=cache,target=/x pip install a",
                2,
                "BuildKit",
            ),
            ("FROM a\nRUN --network=none x", 2, "--network"),
            ("FROM a\nRUN cat <<EOF\nhi\nEOF", 2, "heredoc"),
            ("FROM a\nCOPY <<-EOT /x\nhi\nEOT", 2, "heredoc"),
            ("FROM a\nADD http://x/y /z", 2, "network"),
            ("FROM a\nADD git@github.com:a/b.git /z", 2, "network"),
            ("FROM a\nADD https://x/a.tar.gz /z", 2, "network"),
            ("FROM a\nCOPY --parents a /b", 2, "--parents"),
            ("FROM a\nBOGUS x", 2, "unknown instruction"),
            ("RUN x", 1, "before FROM"),
            ("", 0, "no FROM"),
            ("FROM", 1, "FROM needs"),
            ("FROM a\nCOPY a", 2, "needs a source"),
            ("FROM a\nENV A", 2, "needs a value"),
            ("FROM a\nSHELL /bin/bash", 2, "JSON array"),
            ("FROM a\nENV A='x", 2, "unterminated"),
        ];
        for (text, line, why) in cases {
            let e = refused(text);
            assert_eq!(e.line, *line, "{text:?}: {e}");
            assert!(e.message.contains(why), "{text:?}: {e}");
        }
    }

    #[test]
    fn shell_arithmetic_and_here_strings_are_not_heredocs() {
        assert!(parse("FROM a\nRUN echo $((1<<2))").is_ok());
        assert!(parse("FROM a\nRUN cat <<< hello").is_ok());
        assert!(parse("FROM a\nRUN x << 3").is_ok());
    }

    #[test]
    fn expansion_matches_the_reference() {
        let e = env(&[("A", "1"), ("EMPTY", "")]);
        assert_eq!(
            expand(
                "$A-${A}-${B:-d}-${EMPTY:-d}-${EMPTY-d}-${A:+alt}-${B:+alt}x",
                &e
            ),
            "1-1-d-d--alt-x"
        );
        assert_eq!(expand("\\$A $UNSET end", &e), "$A  end");
        assert_eq!(expand("${A", &e), "${A");
        assert_eq!(expand("price $5", &e), "price $5");
        assert_eq!(expand("${B:-${A}}", &e), "1");
    }

    #[test]
    fn words_handle_quotes() {
        assert_eq!(
            words("a 'b c' \"d \\\" e\" f\\ g").unwrap(),
            ["a", "b c", "d \" e", "f g"]
        );
        assert!(words("'x").is_err());
        assert_eq!(words("").unwrap(), Vec::<String>::new());
        assert_eq!(words("''").unwrap(), [""]);
    }

    fn cases() -> ProptestConfig {
        ProptestConfig::with_cases(
            std::env::var("PROPTEST_CASES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64),
        )
    }

    proptest! {
        #![proptest_config(cases())]

        #[test]
        fn parse_never_panics(s in "\\PC{0,300}") {
            let _ = parse(&s);
        }

        #[test]
        fn parse_never_panics_on_dockerfile_shaped_text(
            lines in proptest::collection::vec(
                (
                    proptest::sample::select(vec![
                        "FROM", "RUN", "ENV", "ARG", "COPY", "ADD", "WORKDIR", "USER", "CMD", "SHELL",
                        "EXPOSE", "LABEL", "VOLUME", "#", "",
                    ]),
                    "[ -~]{0,30}",
                    proptest::bool::ANY,
                ),
                0..12,
            )
        ) {
            let mut t = String::new();
            for (kw, rest, cont) in lines {
                t.push_str(&format!("{kw} {rest}{}\n", if cont { " \\" } else { "" }));
            }
            let _ = parse(&t);
            let _ = expand(&t, &BTreeMap::new());
        }

        #[test]
        fn words_roundtrip_shell_quoted(ws in proptest::collection::vec("[ -~]{0,12}", 0..6)) {
            let line = ws.iter().map(|w| shell_quote(w)).collect::<Vec<_>>().join(" ");
            prop_assert_eq!(words(&line).unwrap(), ws);
        }

        #[test]
        fn run_lines_keep_order_and_line_numbers(cmds in proptest::collection::vec("[a-z]{1,8}", 1..8)) {
            let mut t = String::from("FROM x\n");
            for c in &cmds { t.push_str(&format!("RUN {c}\n")); }
            let d = parse(&t).unwrap();
            let got: Vec<(usize, String)> = d.instructions.iter().map(|l| match &l.instr {
                Instr::Run(Exec::Shell(s)) => (l.line, s.clone()),
                other => panic!("{other:?}"),
            }).collect();
            let want: Vec<(usize, String)> = cmds.iter().enumerate().map(|(i, c)| (i + 2, c.clone())).collect();
            prop_assert_eq!(got, want);
        }

        #[test]
        fn expand_without_dollars_is_identity(s in "[^$\\\\]{0,40}") {
            prop_assert_eq!(expand(&s, &BTreeMap::new()), s);
        }
    }
}
