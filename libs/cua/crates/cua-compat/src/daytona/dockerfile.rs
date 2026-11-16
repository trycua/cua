//! The Dockerfile subset Daytona's `Image` builder (and `Image.from_dockerfile`)
//! sends as `buildInfo.dockerfileContent`, turned into a [`Plan`] that Cua
//! builds with its own image layers (`ImageBuild`): one base image, then
//! `RUN` steps in order with the `ENV`, `WORKDIR` and `USER` in force.
//!
//! The grammar is Docker's public Dockerfile reference; the instructions the
//! SDK emits are in `daytona/common/image.py` (Apache-2.0, SDK 0.225.0):
//! `FROM`, `RUN`, `COPY <context archive path> <dest>`, `ENV k=v`, `WORKDIR`,
//! `ENTRYPOINT [..]`, `CMD [..]`, plus whatever `dockerfile_commands` and
//! `from_dockerfile` carry. What cannot be built is refused with the
//! instruction named; nothing is skipped silently.

use std::collections::BTreeMap;

/// One instruction after line joining and argument splitting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    From {
        image: String,
        platform: Option<String>,
        stage: Option<String>,
    },
    Run(String),
    Env(Vec<(String, String)>),
    Arg(String, Option<String>),
    Workdir(String),
    User(String),
    Copy {
        sources: Vec<String>,
        dest: String,
        chown: Option<String>,
        chmod: Option<String>,
        /// `ADD` (which also unpacks local tar archives; URLs are refused).
        add: bool,
    },
    Expose(Vec<u16>),
    Entrypoint(Vec<String>),
    Cmd(Vec<String>),
    /// `LABEL`, `MAINTAINER`, `STOPSIGNAL`, `HEALTHCHECK`, `VOLUME`, `SHELL`:
    /// metadata that does not change what runs at build time.
    Metadata(String),
}

/// A parsed Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dockerfile {
    pub instructions: Vec<Instruction>,
}

/// Joins continuation lines, drops comments and blank lines.
pub fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let t = line.trim_start();
        if cur.is_empty() && (t.is_empty() || t.starts_with('#')) {
            continue;
        }
        if !cur.is_empty() && (t.starts_with('#') || t.is_empty()) {
            // Comments and blank lines inside a continued instruction.
            continue;
        }
        if let Some(stripped) = line.trim_end().strip_suffix('\\') {
            cur.push_str(stripped);
            cur.push(' ');
            continue;
        }
        cur.push_str(line);
        out.push(std::mem::take(&mut cur).trim().to_string());
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Shell-like word splitting with `'..'`, `".."` and backslash escapes.
pub fn split_words(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if have {
                    words.push(std::mem::take(&mut cur));
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
                        Some('\\') => match chars.peek().copied() {
                            Some(n @ ('"' | '\\' | '$' | '`')) => {
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
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c => {
                have = true;
                cur.push(c);
            }
        }
    }
    if have {
        words.push(cur);
    }
    Ok(words)
}

fn json_array(arg: &str) -> Option<Vec<String>> {
    let t = arg.trim();
    if !t.starts_with('[') {
        return None;
    }
    serde_json::from_str::<Vec<String>>(t).ok()
}

/// Splits `--flag=value` options off the front of an argument string.
fn flags(arg: &str) -> (BTreeMap<String, String>, &str) {
    let mut rest = arg.trim_start();
    let mut m = BTreeMap::new();
    while let Some(r) = rest.strip_prefix("--") {
        let end = r.find(char::is_whitespace).unwrap_or(r.len());
        let (f, tail) = r.split_at(end);
        let (k, v) = f.split_once('=').unwrap_or((f, ""));
        m.insert(k.to_ascii_lowercase(), v.to_string());
        rest = tail.trim_start();
    }
    (m, rest)
}

/// Parses a Dockerfile.
pub fn parse(text: &str) -> Result<Dockerfile, String> {
    let mut instructions = Vec::new();
    for line in logical_lines(text) {
        let (kw, arg) = match line.split_once(char::is_whitespace) {
            Some((k, a)) => (k.to_ascii_uppercase(), a.trim()),
            None => (line.to_ascii_uppercase(), ""),
        };
        let need = |what: &str| -> Result<(), String> {
            if arg.is_empty() {
                Err(format!("{kw} needs {what}"))
            } else {
                Ok(())
            }
        };
        let ins = match kw.as_str() {
            "FROM" => {
                need("an image")?;
                let (fl, rest) = flags(arg);
                let w = split_words(rest)?;
                let image = w.first().cloned().ok_or("FROM needs an image")?;
                let stage = match w.as_slice() {
                    [_, as_, name] if as_.eq_ignore_ascii_case("as") => Some(name.clone()),
                    [_] => None,
                    _ => return Err(format!("invalid FROM: {line}")),
                };
                Instruction::From {
                    image,
                    platform: fl.get("platform").cloned(),
                    stage,
                }
            }
            "RUN" => {
                need("a command")?;
                let (fl, rest) = flags(arg);
                if let Some(k) = fl.keys().next() {
                    return Err(format!("RUN --{k} is not supported"));
                }
                if rest.contains("<<") && rest.lines().count() > 1 {
                    return Err("RUN with a here-document is not supported".into());
                }
                match json_array(rest) {
                    Some(argv) => Instruction::Run(
                        argv.iter()
                            .map(|a| shell_quote(a))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    None => Instruction::Run(rest.to_string()),
                }
            }
            "ENV" => {
                need("a name and value")?;
                Instruction::Env(parse_env(arg)?)
            }
            "ARG" => {
                need("a name")?;
                let w = split_words(arg)?;
                let first = w.first().ok_or("ARG needs a name")?;
                match first.split_once('=') {
                    Some((k, v)) => Instruction::Arg(k.to_string(), Some(v.to_string())),
                    None => Instruction::Arg(first.clone(), None),
                }
            }
            "WORKDIR" => {
                need("a path")?;
                Instruction::Workdir(
                    split_words(arg)?
                        .first()
                        .cloned()
                        .ok_or("WORKDIR needs a path")?,
                )
            }
            "USER" => {
                need("a user")?;
                Instruction::User(
                    split_words(arg)?
                        .first()
                        .cloned()
                        .ok_or("USER needs a user")?,
                )
            }
            "COPY" | "ADD" => {
                need("sources and a destination")?;
                let (fl, rest) = flags(arg);
                if fl.contains_key("from") {
                    return Err(format!("{kw} --from (multi-stage builds) is not supported"));
                }
                for k in fl.keys() {
                    if !["chown", "chmod", "link", "keep-git-dir"].contains(&k.as_str()) {
                        return Err(format!("{kw} --{k} is not supported"));
                    }
                }
                if rest.contains("<<") {
                    return Err(format!("{kw} with a here-document is not supported"));
                }
                let mut w = json_array(rest).map_or_else(|| split_words(rest), Ok)?;
                if w.len() < 2 {
                    return Err(format!("{kw} needs sources and a destination"));
                }
                let dest = w.pop().expect("len checked");
                if kw == "ADD" && w.iter().any(|s| s.contains("://")) {
                    return Err("ADD from a URL is not supported".into());
                }
                Instruction::Copy {
                    sources: w,
                    dest,
                    chown: fl.get("chown").cloned(),
                    chmod: fl.get("chmod").cloned(),
                    add: kw == "ADD",
                }
            }
            "EXPOSE" => {
                let mut ports = Vec::new();
                for w in split_words(arg)? {
                    let p = w.split('/').next().unwrap_or("");
                    ports.push(
                        p.parse::<u16>()
                            .map_err(|_| format!("invalid port {w:?}"))?,
                    );
                }
                Instruction::Expose(ports)
            }
            "ENTRYPOINT" | "CMD" => {
                need("a command")?;
                let argv = json_array(arg)
                    .unwrap_or_else(|| vec!["/bin/sh".into(), "-c".into(), arg.to_string()]);
                if kw == "CMD" {
                    Instruction::Cmd(argv)
                } else {
                    Instruction::Entrypoint(argv)
                }
            }
            "LABEL" | "MAINTAINER" | "STOPSIGNAL" | "HEALTHCHECK" | "VOLUME" | "SHELL" => {
                Instruction::Metadata(kw)
            }
            "ONBUILD" => return Err("ONBUILD is not supported".into()),
            other => return Err(format!("unknown instruction {other}")),
        };
        instructions.push(ins);
    }
    Ok(Dockerfile { instructions })
}

fn parse_env(arg: &str) -> Result<Vec<(String, String)>, String> {
    let words = split_words(arg)?;
    if words.first().is_some_and(|w| w.contains('=')) {
        words
            .iter()
            .map(|w| {
                w.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .ok_or_else(|| format!("invalid ENV entry {w:?}"))
            })
            .collect()
    } else {
        // `ENV key value with spaces`
        let (k, v) = arg
            .split_once(char::is_whitespace)
            .ok_or("ENV needs a value")?;
        Ok(vec![(k.to_string(), v.trim().to_string())])
    }
}

/// Single-quotes `s` for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,@%+".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Replaces `$NAME`, `${NAME}`, `${NAME:-default}` and `${NAME-default}`
/// from `vars` (unknown names become empty, as Docker does).
pub fn substitute(s: &str, vars: &BTreeMap<String, String>) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' && i + 1 < b.len() && b[i + 1] == '$' {
            out.push('$');
            i += 2;
        } else if b[i] == '$' && i + 1 < b.len() && b[i + 1] == '{' {
            if let Some(end) = b[i + 2..].iter().position(|c| *c == '}') {
                let inner: String = b[i + 2..i + 2 + end].iter().collect();
                let (name, dflt) = match inner.split_once(":-").or_else(|| inner.split_once('-')) {
                    Some((n, d)) => (n.to_string(), Some(d.to_string())),
                    None => (inner.clone(), None),
                };
                match vars.get(&name).filter(|v| !v.is_empty()) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&dflt.unwrap_or_default()),
                }
                i += end + 3;
            } else {
                out.push('$');
                i += 1;
            }
        } else if b[i] == '$'
            && i + 1 < b.len()
            && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == '_')
        {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == '_') {
                j += 1;
            }
            let name: String = b[i + 1..j].iter().collect();
            out.push_str(vars.get(&name).map_or("", String::as_str));
            i = j;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// What building the Dockerfile means for Cua.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// The base image reference.
    pub from: String,
    /// `--platform` of FROM, if any.
    pub platform: Option<String>,
    /// Shell commands, in order (each already `cd`/`su`/`export`-wrapped).
    pub steps: Vec<Step>,
    /// Final `ENV`.
    pub env: BTreeMap<String, String>,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub ports: Vec<u16>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A shell command run in the image as root.
    Run(String),
    /// A context file or directory copied in: archive path, destination, the
    /// user/mode to set.
    Copy {
        source: String,
        dest: String,
        chown: Option<String>,
        chmod: Option<String>,
        unpack: bool,
    },
}

/// Walks the instructions and resolves ARG/ENV/WORKDIR/USER into the steps.
/// `build_args` are the values `ARG` may take (none from the SDK).
pub fn plan(df: &Dockerfile, build_args: &BTreeMap<String, String>) -> Result<Plan, String> {
    let mut p = Plan::default();
    let mut froms = 0;
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut args: BTreeMap<String, String> = BTreeMap::new();
    for ins in &df.instructions {
        match ins {
            Instruction::From {
                image, platform, ..
            } => {
                froms += 1;
                if froms > 1 {
                    return Err(
                        "multi-stage Dockerfiles (more than one FROM) are not supported".into(),
                    );
                }
                p.from = substitute(image, &vars);
                p.platform = platform.clone();
            }
            Instruction::Arg(k, d) => {
                if froms == 0 && p.from.is_empty() {
                    // ARG before FROM only feeds FROM.
                }
                let v = build_args
                    .get(k)
                    .cloned()
                    .or_else(|| d.clone())
                    .unwrap_or_default();
                vars.insert(k.clone(), v.clone());
                args.insert(k.clone(), v);
            }
            _ if froms == 0 => return Err("a Dockerfile must start with FROM".into()),
            Instruction::Env(kv) => {
                for (k, v) in kv {
                    let v = substitute(v, &vars);
                    vars.insert(k.clone(), v.clone());
                    env.insert(k.clone(), v);
                }
            }
            Instruction::Workdir(w) => {
                let w = substitute(w, &vars);
                let abs = if w.starts_with('/') {
                    w
                } else {
                    format!(
                        "{}/{}",
                        p.workdir.as_deref().unwrap_or("/").trim_end_matches('/'),
                        w
                    )
                };
                p.steps
                    .push(Step::Run(format!("mkdir -p {}", shell_quote(&abs))));
                p.workdir = Some(abs);
            }
            Instruction::User(u) => p.user = Some(substitute(u, &vars)),
            Instruction::Run(cmd) => {
                let mut script = String::new();
                for (k, v) in args.iter().chain(env.iter()) {
                    script.push_str(&format!("export {k}={}; ", shell_quote(v)));
                }
                if let Some(w) = &p.workdir {
                    script.push_str(&format!("cd {} && ", shell_quote(w)));
                }
                script.push_str(cmd);
                let line = match p.user.as_deref() {
                    Some(u) if u != "root" && u != "0" => format!(
                        "su -s /bin/sh {} -c {}",
                        shell_quote(u.split(':').next().unwrap_or(u)),
                        shell_quote(&script)
                    ),
                    _ => script,
                };
                p.steps.push(Step::Run(line));
            }
            Instruction::Copy {
                sources,
                dest,
                chown,
                chmod,
                add,
            } => {
                let dest = substitute(dest, &vars);
                let dest = if dest.starts_with('/') {
                    dest
                } else {
                    format!(
                        "{}/{}",
                        p.workdir.as_deref().unwrap_or("/").trim_end_matches('/'),
                        dest
                    )
                };
                for s in sources {
                    p.steps.push(Step::Copy {
                        source: substitute(s, &vars),
                        dest: dest.clone(),
                        chown: chown.clone(),
                        chmod: chmod.clone(),
                        unpack: *add,
                    });
                }
            }
            Instruction::Expose(ports) => p.ports.extend(ports),
            Instruction::Entrypoint(a) => p.entrypoint = Some(a.clone()),
            Instruction::Cmd(a) => p.cmd = Some(a.clone()),
            Instruction::Metadata(_) => {}
        }
    }
    if froms == 0 {
        return Err("a Dockerfile must start with FROM".into());
    }
    p.env = env;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cfg() -> ProptestConfig {
        ProptestConfig::with_cases(
            std::env::var("PROPTEST_CASES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64),
        )
    }

    #[test]
    fn what_the_sdk_emits() {
        // Image.debian_slim("3.12").pip_install("numpy").env({"A": "b c"}).workdir("/w")
        let t = "FROM python:3.12-slim-bookworm\nRUN python -m pip install numpy\nENV A='b c'\nWORKDIR /w\nCOPY home/u/f.txt /w/f.txt\nENTRYPOINT [\"sleep\", \"infinity\"]\n";
        let df = parse(t).unwrap();
        assert_eq!(df.instructions.len(), 6);
        let p = plan(&df, &BTreeMap::new()).unwrap();
        assert_eq!(p.from, "python:3.12-slim-bookworm");
        assert_eq!(p.env["A"], "b c");
        assert_eq!(p.workdir.as_deref(), Some("/w"));
        assert_eq!(
            p.entrypoint.as_deref(),
            Some(&["sleep".to_string(), "infinity".into()][..])
        );
        assert!(
            matches!(&p.steps[0], Step::Run(s) if s.starts_with("export ") || s.contains("pip install"))
        );
        assert!(p.steps.iter().any(|s| matches!(s, Step::Copy { source, dest, .. } if source == "home/u/f.txt" && dest == "/w/f.txt")));
    }

    #[test]
    fn continuations_comments_and_json_forms() {
        let t = "# c\nFROM a:b\nRUN apt-get update && \\\n    apt-get install -y \\\n  # inner comment\n    curl\nCMD [\"echo\", \"it's\"]\nRUN [\"echo\", \"a b\"]\n";
        let df = parse(t).unwrap();
        let Instruction::Run(r) = &df.instructions[1] else {
            panic!("{:?}", df.instructions[1])
        };
        assert!(
            r.starts_with("apt-get update &&")
                && r.contains("apt-get install -y")
                && r.ends_with("curl"),
            "{r}"
        );
        assert!(!r.contains("inner comment"), "{r}");
        assert_eq!(df.instructions[3], Instruction::Run("echo 'a b'".into()));
    }

    #[test]
    fn refuses_what_it_cannot_build() {
        for bad in [
            "FROM a\nCOPY --from=b /x /y",
            "FROM a\nONBUILD RUN x",
            "FROM a\nRUN --mount=type=cache x",
            "FROM a\nADD http://x/y /z",
            "FROM a\nFROM b",
            "RUN x",
            "FROM a\nBOGUS x",
        ] {
            let r = parse(bad).and_then(|d| plan(&d, &BTreeMap::new()).map(|_| ()));
            assert!(r.is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn arg_env_substitution_and_user() {
        let t = "ARG V=3.12\nFROM python:${V}\nENV P=/opt/${V}\nUSER app\nRUN echo $P\n";
        let p = plan(&parse(t).unwrap(), &BTreeMap::new()).unwrap();
        assert_eq!(p.from, "python:3.12");
        assert_eq!(p.env["P"], "/opt/3.12");
        let Step::Run(s) = p.steps.last().unwrap() else {
            panic!()
        };
        assert!(s.starts_with("su -s /bin/sh app -c "), "{s}");
    }

    #[test]
    fn substitution_forms() {
        let v: BTreeMap<String, String> = [("A".to_string(), "1".to_string())].into();
        assert_eq!(
            substitute("$A-${A}-${B:-x}-${A:-y}-\\$A-$B", &v),
            "1-1-x-1-$A-"
        );
    }

    proptest! {
        #![proptest_config(cfg())]
        #[test]
        fn parse_never_panics(s in "\\PC{0,300}") {
            let _ = parse(&s).and_then(|d| plan(&d, &BTreeMap::new()));
        }

        #[test]
        fn split_words_roundtrips_quoted(words in proptest::collection::vec("[ -~]{0,12}", 0..6)) {
            let line = words.iter().map(|w| shell_quote(w)).collect::<Vec<_>>().join(" ");
            prop_assert_eq!(split_words(&line).unwrap(), words);
        }

        #[test]
        fn run_steps_keep_order(cmds in proptest::collection::vec("[a-z]{1,8}", 1..8)) {
            let mut t = String::from("FROM x\n");
            for c in &cmds { t.push_str(&format!("RUN {c}\n")); }
            let p = plan(&parse(&t).unwrap(), &BTreeMap::new()).unwrap();
            let got: Vec<String> = p.steps.iter().map(|s| match s { Step::Run(r) => r.clone(), _ => String::new() }).collect();
            prop_assert_eq!(got, cmds);
        }
    }
}
