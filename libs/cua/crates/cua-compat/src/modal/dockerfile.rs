//! Dockerfile text, as the Modal client sends it: the lines of an image
//! layer (`FROM base`, `RUN ...`, `COPY ...`, the lines of a user's
//! Dockerfile split on newlines) joined and parsed here, then interpreted
//! by the image builder. Pure (no I/O), so the parser is unit-tested and
//! fuzzed.
//!
//! Supported: `FROM [--platform=..] ref [AS stage]`, `RUN` (shell and exec
//! forms, `--mount`/`--network`/`--security` flags are refused), `ENV`,
//! `ARG`, `WORKDIR`, `USER`, `COPY`/`ADD` (`--from`, `--chown`, `--chmod`;
//! context files and mounts, `ADD` of local files only), `CMD`,
//! `ENTRYPOINT`, `SHELL`, `LABEL`, `EXPOSE`, `VOLUME`, `STOPSIGNAL`,
//! `HEALTHCHECK` (recorded). Line continuations, comments, parser
//! directives, quoting and `${VAR}` / `${VAR:-default}` / `${VAR:+alt}`
//! expansion follow the Dockerfile reference. `ONBUILD` and heredocs are
//! refused with a clear error.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instr {
    From {
        reference: String,
        stage: Option<String>,
        platform: Option<String>,
    },
    Run(Exec),
    Env(Vec<(String, String)>),
    Arg {
        name: String,
        default: Option<String>,
    },
    Workdir(String),
    User(String),
    Copy(Copy),
    Cmd(Exec),
    Entrypoint(Exec),
    Shell(Vec<String>),
    Label(Vec<(String, String)>),
    Expose(Vec<String>),
    Volume(Vec<String>),
    StopSignal(String),
    Healthcheck(String),
}

/// A command: a shell line or an argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exec {
    Shell(String),
    Argv(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    pub add: bool,
    pub from: Option<String>,
    pub chown: Option<String>,
    pub chmod: Option<String>,
    pub sources: Vec<String>,
    pub dest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        line,
        message: message.into(),
    })
}

/// Logical lines: continuations joined, comments and blanks dropped, with
/// the 1-based number of each one's first physical line.
pub fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    let mut escape = '\\';
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        // Parser directives (`# escape=` ) before any instruction.
        if cur.is_empty()
            && out.is_empty()
            && let Some(d) = trimmed.strip_prefix('#')
            && let Some(v) = d.trim().strip_prefix("escape=")
            && let Some(c) = v.trim().chars().next()
        {
            escape = c;
        }
        if trimmed.starts_with('#') {
            continue; // a comment line, even inside a continuation
        }
        if cur.is_empty() {
            if trimmed.is_empty() {
                continue;
            }
            start = n;
        }
        // Whitespace after the escape character still continues the line
        // (Modal's own Poetry layer writes `\ ` at the end of lines).
        if let Some(body) = line.trim_end().strip_suffix(escape)
            && !body.ends_with(escape)
        {
            cur.push_str(body);
            continue;
        }
        cur.push_str(line);
        out.push((start, std::mem::take(&mut cur)));
    }
    if !cur.is_empty() {
        out.push((start, cur));
    }
    out
}

/// Splits a word list on unquoted whitespace, removing quotes and
/// backslash escapes (the Dockerfile's shell-form-free tokenization).
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
        fl.push((k.to_string(), v.to_string()));
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

/// Parses Dockerfile text.
pub fn parse(text: &str) -> Result<Vec<Instr>, ParseError> {
    let mut out = Vec::new();
    for (n, line) in logical_lines(text) {
        let line = line.trim();
        let (kw, rest) = line
            .split_once(char::is_whitespace)
            .map_or((line, ""), |(a, b)| (a, b.trim_start()));
        let kw_up = kw.to_ascii_uppercase();
        if rest.contains("<<") && matches!(kw_up.as_str(), "RUN" | "COPY" | "ADD") {
            let h = rest.split("<<").nth(1).unwrap_or("");
            if h.starts_with(|c: char| c.is_ascii_alphabetic() || c == '"' || c == '\'' || c == '-')
            {
                return err(
                    n,
                    "heredocs are not supported in image layers; put the script in a file and COPY it",
                );
            }
        }
        let instr = match kw_up.as_str() {
            "FROM" => {
                let (fl, body) = flags(rest);
                let ws = words(body).or_else(|m| err(n, m))?;
                let platform = fl
                    .iter()
                    .find(|(k, _)| k == "platform")
                    .map(|(_, v)| v.clone());
                match ws.as_slice() {
                    [r] => Instr::From {
                        reference: r.clone(),
                        stage: None,
                        platform,
                    },
                    [r, a, name] if a.eq_ignore_ascii_case("as") => Instr::From {
                        reference: r.clone(),
                        stage: Some(name.to_ascii_lowercase()),
                        platform,
                    },
                    _ => return err(n, "FROM needs an image: FROM <image> [AS <name>]"),
                }
            }
            "RUN" => {
                let (fl, body) = flags(rest);
                if let Some((k, _)) = fl
                    .iter()
                    .find(|(k, _)| matches!(k.as_str(), "mount" | "network" | "security"))
                {
                    return err(
                        n,
                        format!(
                            "RUN --{k} is a BuildKit feature and is not supported in image layers"
                        ),
                    );
                }
                if body.is_empty() {
                    return err(n, "RUN needs a command");
                }
                Instr::Run(exec(body))
            }
            "ENV" => Instr::Env(key_values(n, rest, "ENV")?),
            "LABEL" => Instr::Label(key_values(n, rest, "LABEL")?),
            "ARG" => {
                let ws = words(rest).or_else(|m| err(n, m))?;
                let Some(first) = ws.first() else {
                    return err(n, "ARG needs a name");
                };
                match first.split_once('=') {
                    Some((k, v)) => Instr::Arg {
                        name: k.to_string(),
                        default: Some(v.to_string()),
                    },
                    None => Instr::Arg {
                        name: first.clone(),
                        default: None,
                    },
                }
            }
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
                let mut c = Copy {
                    add: kw_up == "ADD",
                    from: None,
                    chown: None,
                    chmod: None,
                    sources: Vec::new(),
                    dest: String::new(),
                };
                for (k, v) in fl {
                    match k.as_str() {
                        "from" => c.from = Some(v.to_ascii_lowercase()),
                        "chown" => c.chown = Some(v),
                        "chmod" => c.chmod = Some(v),
                        "link" | "parents" | "exclude" | "keep-git-dir" => {}
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
            "ONBUILD" => return err(n, "ONBUILD is not supported in image layers"),
            other => return err(n, format!("unknown instruction {other}")),
        };
        out.push(instr);
    }
    Ok(out)
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
                let Some(close) = cs[i + 2..].iter().position(|&c| c == '}') else {
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

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parses_the_lines_modal_generates() {
        let p = parse(
            "FROM python:3.12-slim-bookworm\nRUN apt-get update\nRUN apt-get install -y gcc gfortran build-essential\nRUN echo 'debconf debconf/frontend select Noninteractive' | debconf-set-selections\nCMD [\"sleep\", \"172800\"]",
        )
        .unwrap();
        assert_eq!(
            p[0],
            Instr::From {
                reference: "python:3.12-slim-bookworm".into(),
                stage: None,
                platform: None
            }
        );
        assert_eq!(p[1], Instr::Run(Exec::Shell("apt-get update".into())));
        assert_eq!(
            p[4],
            Instr::Cmd(Exec::Argv(vec!["sleep".into(), "172800".into()]))
        );
    }

    #[test]
    fn a_continuation_may_have_trailing_whitespace() {
        let lines = logical_lines("RUN cd /tmp && \\ \n  echo a && \\\t\n  echo b\nENV X=1\n");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].1.contains("echo a") && lines[0].1.contains("echo b"));
    }

    #[test]
    fn joins_continuations_and_skips_comments() {
        let p = parse("# syntax=docker/dockerfile:1\nFROM a\n# note\nRUN a \\\n  # inner comment\n  && b\n\nENV X=1")
            .unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[1], Instr::Run(Exec::Shell("a   && b".into())));
        assert_eq!(p[2], Instr::Env(vec![("X".into(), "1".into())]));
    }

    #[test]
    fn env_forms() {
        assert_eq!(
            parse("ENV A=1 B=\"two words\" C=x\\ y").unwrap(),
            vec![Instr::Env(vec![
                ("A".into(), "1".into()),
                ("B".into(), "two words".into()),
                ("C".into(), "x y".into())
            ])]
        );
        assert_eq!(
            parse("ENV MSG hello world").unwrap(),
            vec![Instr::Env(vec![("MSG".into(), "hello world".into())])]
        );
    }

    #[test]
    fn copy_flags_and_json_form() {
        let p = parse("COPY --chown=1:2 --from=Build /a /b /dst/\nADD [\"x y\", \"/z\"]").unwrap();
        let Instr::Copy(c) = &p[0] else { panic!() };
        assert_eq!(c.from.as_deref(), Some("build"));
        assert_eq!(c.chown.as_deref(), Some("1:2"));
        assert_eq!(c.sources, ["/a", "/b"]);
        assert_eq!(c.dest, "/dst/");
        let Instr::Copy(a) = &p[1] else { panic!() };
        assert!(a.add);
        assert_eq!(a.sources, ["x y"]);
    }

    #[test]
    fn refuses_what_it_cannot_do_with_a_reason() {
        assert!(
            parse("RUN --mount=type=cache,target=/x pip install a")
                .unwrap_err()
                .message
                .contains("BuildKit")
        );
        assert!(parse("ONBUILD RUN x").is_err());
        assert!(
            parse("RUN cat <<EOF\nhi\nEOF")
                .unwrap_err()
                .message
                .contains("heredocs")
        );
        assert!(
            parse("BOGUS x")
                .unwrap_err()
                .message
                .contains("unknown instruction")
        );
        assert!(parse("FROM").is_err());
        assert!(parse("COPY a").is_err());
        assert!(parse("ENV A").is_err());
        assert!(parse("SHELL /bin/bash").is_err());
    }

    #[test]
    fn multi_stage_from() {
        let p = parse("FROM golang:1.22 AS Build\nFROM scratch").unwrap();
        assert_eq!(
            p[0],
            Instr::From {
                reference: "golang:1.22".into(),
                stage: Some("build".into()),
                platform: None
            }
        );
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
}
