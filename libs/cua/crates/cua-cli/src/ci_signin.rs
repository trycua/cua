//! The GitHub sign-in `cua ci connect` finds when no token is given.
//!
//! Tried in order: `GH_TOKEN` or `GITHUB_TOKEN` in the environment, then the
//! GitHub CLI (`gh auth token`, signed in with `gh auth login`). The token is
//! held in memory for the one call that uses it and never written down by Cua.
//! The environment and `gh` are behind small traits so tests use fakes.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The default host.
pub const DEFAULT_HOST: &str = "github.com";

/// How long one `gh` call may take.
const GH_WAIT: Duration = Duration::from_secs(15);

/// A token. It prints as `(hidden)`; only [`Token::expose`] gives it out.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// A token from `value` (trimmed).
    pub fn new(value: impl Into<String>) -> Token {
        Token(value.into().trim().to_string())
    }

    /// The token, for the call that uses it. Do not store, log or format it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token((hidden))")
    }
}

/// A sign-in that was found.
#[derive(Debug)]
pub struct Resolved {
    pub token: Token,
}

/// Why no sign-in can be used, in the words the person is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// Nothing is signed in.
    Nothing(String),
    /// `gh` knows the account but would not give a token.
    GhRefused { host: String },
}

impl Unavailable {
    /// The sentence for the person.
    pub fn message(&self) -> String {
        match self {
            Unavailable::Nothing(h) if h == DEFAULT_HOST => {
                "No GitHub sign-in found: run gh auth login or paste a token".into()
            }
            Unavailable::Nothing(h) => {
                format!("No sign-in found for {h}: run gh auth login --hostname {h}")
            }
            Unavailable::GhRefused { host } => format!(
                "gh is signed in on {host} but would not give a token: run gh auth login again"
            ),
        }
    }
}

/// Environment variables.
pub trait Env: Send + Sync {
    fn var(&self, name: &str) -> Option<String>;
}

/// This process's environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.trim().is_empty())
    }
}

/// What a `gh` call came to.
#[derive(Debug, Default)]
pub struct GhOut {
    pub ok: bool,
    pub stdout: String,
}

/// The GitHub CLI.
pub trait Gh: Send + Sync {
    /// Whether a `gh` can be found.
    fn found(&self) -> bool;
    /// Runs `gh args` with no sign-in variables in its environment and its
    /// stdin closed. `None` when it cannot run.
    fn run(&self, args: &[&str]) -> Option<GhOut>;
}

/// The `gh` on this computer's `PATH`.
pub struct SystemGh;

impl Gh for SystemGh {
    fn found(&self) -> bool {
        self.run(&["--version"]).is_some_and(|o| o.ok)
    }

    fn run(&self, args: &[&str]) -> Option<GhOut> {
        let mut child = Command::new("gh")
            .args(args)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_ENTERPRISE_TOKEN")
            .env_remove("GITHUB_ENTERPRISE_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut out = String::new();
                    if let Some(mut so) = child.stdout.take() {
                        use std::io::Read as _;
                        let _ = so.read_to_string(&mut out);
                    }
                    return Some(GhOut {
                        ok: status.success(),
                        stdout: out,
                    });
                }
                Ok(None) if start.elapsed() < GH_WAIT => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }
}

/// The sign-in sources, in order.
pub struct Chain {
    gh: Box<dyn Gh>,
    env: Box<dyn Env>,
}

impl Chain {
    pub fn new(gh: Box<dyn Gh>, env: Box<dyn Env>) -> Chain {
        Chain { gh, env }
    }

    /// The token to use for `host`.
    pub fn token(&self, host: &str) -> Result<Resolved, Unavailable> {
        let names: &[&str] = if host == DEFAULT_HOST {
            &["GH_TOKEN", "GITHUB_TOKEN"]
        } else {
            &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
        };
        for n in names {
            if let Some(v) = self.env.var(n) {
                return Ok(Resolved {
                    token: Token::new(v),
                });
            }
        }
        if !self.gh.found() {
            return Err(Unavailable::Nothing(host.to_string()));
        }
        match self.gh.run(&["auth", "token", "--hostname", host]) {
            Some(o) if o.ok && !o.stdout.trim().is_empty() => Ok(Resolved {
                token: Token::new(o.stdout),
            }),
            Some(o) if !o.ok => Err(Unavailable::Nothing(host.to_string())),
            _ => Err(Unavailable::GhRefused {
                host: host.to_string(),
            }),
        }
    }
}

/// This process's chain.
pub fn global() -> Chain {
    Chain::new(Box::new(SystemGh), Box::new(ProcessEnv))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeEnv(Vec<(&'static str, &'static str)>);
    impl Env for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        }
    }

    struct FakeGh(Option<GhOut>);
    impl Gh for FakeGh {
        fn found(&self) -> bool {
            self.0.is_some()
        }
        fn run(&self, _: &[&str]) -> Option<GhOut> {
            self.0.as_ref().map(|o| GhOut {
                ok: o.ok,
                stdout: o.stdout.clone(),
            })
        }
    }

    fn chain(env: Vec<(&'static str, &'static str)>, gh: Option<GhOut>) -> Chain {
        Chain::new(Box::new(FakeGh(gh)), Box::new(FakeEnv(env)))
    }

    #[test]
    fn the_environment_comes_before_gh() {
        let c = chain(
            vec![("GITHUB_TOKEN", "ghp_env")],
            Some(GhOut {
                ok: true,
                stdout: "ghp_gh\n".into(),
            }),
        );
        assert_eq!(c.token("github.com").unwrap().token.expose(), "ghp_env");
    }

    #[test]
    fn gh_gives_a_token_when_the_environment_has_none() {
        let c = chain(
            vec![],
            Some(GhOut {
                ok: true,
                stdout: " ghp_gh\n".into(),
            }),
        );
        assert_eq!(c.token("github.com").unwrap().token.expose(), "ghp_gh");
    }

    #[test]
    fn nothing_signed_in_says_how_to_sign_in() {
        let c = chain(vec![], None);
        let u = c.token("github.com").unwrap_err();
        assert_eq!(u, Unavailable::Nothing("github.com".into()));
        assert!(u.message().contains("gh auth login"));
        let u = c.token("ghe.example.com").unwrap_err();
        assert!(u.message().contains("--hostname ghe.example.com"));
    }

    #[test]
    fn gh_that_signed_in_but_gives_nothing_is_refused_in_its_own_words() {
        let c = chain(
            vec![],
            Some(GhOut {
                ok: true,
                stdout: "".into(),
            }),
        );
        assert!(matches!(
            c.token("github.com"),
            Err(Unavailable::GhRefused { .. })
        ));
    }

    #[test]
    fn a_token_never_prints() {
        assert_eq!(format!("{:?}", Token::new("ghp_secret")), "Token((hidden))");
    }
}
