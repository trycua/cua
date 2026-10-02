// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The single choke point for every side effect a provider has on the host.
//!
//! The sending half of teleport runs on the user's own machine and is
//! inherently invasive there: it looks up running apps (`pgrep`), drives
//! AppleScript against a live browser (`osascript`), reads the login Keychain
//! (`security find-*`), talks to a browser's DevTools port, raises a Touch ID
//! prompt and resolves `$HOME`. None of that may ever happen from a test on a
//! developer's machine: an export test must not read the user's real Chrome
//! profile or Keychain credentials. (Writing the Keychain, terminating and
//! launching apps are receiver effects and live in `cua-spacesd-teleport`.)
//!
//! So providers never spawn a process, open a socket or read `$HOME`
//! themselves. They go through a [`HostEffects`] they were constructed with:
//!
//! - [`RealHost`] is the production implementation. It refuses every effect
//!   (and reports no home directory) when compiled into this crate's own tests
//!   or when [`TEST_SANDBOX_ENV`]` = 1` is set, so a test that forgot to inject
//!   a fake fails closed instead of touching the machine.
//! - [`FakeHost`] records every requested effect and answers from a scripted
//!   responder. It is the default for every provider constructor under
//!   `cfg(test)`, and what integration tests in other crates (the SDK, the
//!   `cua` CLI) must inject, usually with a temporary home.
//!
//! A source scan in this module's tests pins the structure: no file in this
//! crate other than `host.rs` may name `std::process::Command` or open a
//! `TcpStream`.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_teleport_bundle::InstallProbe;

use crate::biometric::AuthError;

/// Setting this to `1` makes [`RealHost`] refuse every host effect. Test
/// harnesses that link this crate (SDK and CLI tests, e2e drivers) set it so a
/// missing injection fails closed.
pub const TEST_SANDBOX_ENV: &str = "CUA_ENV_TEST_SANDBOX";

/// What a host effect does, so a fake can assert on intent rather than on a
/// command line, and so a denial can say what was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    /// Is a process running? (`pgrep`)
    ProcessLookup,
    /// Run AppleScript against a live application. (`osascript`)
    AppleScript,
    /// Read Keychain items or Keychain configuration. (`security find-*`)
    KeychainRead,
    /// Query a live browser's DevTools endpoint on this host.
    BrowserDevTools,
    /// Raise an interactive OS authorization prompt (Touch ID / passcode).
    UserAuthorization,
}

/// One external program invocation, described rather than executed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCommand {
    pub kind: EffectKind,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    /// Hard deadline for [`HostEffects::run`]; the child is killed and the
    /// call fails with [`io::ErrorKind::TimedOut`] when it is exceeded.
    pub timeout: Option<Duration>,
}

impl HostCommand {
    pub fn new(kind: EffectKind, program: impl Into<String>) -> Self {
        Self {
            kind,
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            timeout: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// The result of a completed [`HostCommand`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl HostOutput {
    /// A successful run with the given stdout.
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            success: true,
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    /// A failed run (non-zero exit) with empty output.
    pub fn failed() -> Self {
        Self::default()
    }
}

/// Every host side effect a session provider may perform.
pub trait HostEffects: Send + Sync {
    /// Run a program to completion and capture its output.
    fn run(&self, command: &HostCommand) -> io::Result<HostOutput>;

    /// The home directory whose application state is exported from, or
    /// `None` when there is none (or it must not be touched).
    fn home_dir(&self) -> Option<PathBuf>;

    /// Obtain interactive OS user authorization before a sensitive export.
    fn authorize_sensitive_export(&self, reason: &str) -> Result<(), AuthError>;

    /// Page URLs from a Chrome DevTools `/json` endpoint at `addr`.
    fn devtools_page_urls(&self, addr: &str) -> io::Result<Vec<String>>;
}

/// The host effects every provider constructor uses when none is injected:
/// [`FakeHost`] in this crate's own tests, [`RealHost`] otherwise.
pub fn default_host() -> Arc<dyn HostEffects> {
    #[cfg(test)]
    {
        Arc::new(FakeHost::new())
    }
    #[cfg(not(test))]
    {
        Arc::new(RealHost)
    }
}

/// Whether [`RealHost`] must refuse to act: always in this crate's unit tests,
/// and whenever [`TEST_SANDBOX_ENV`] is `1`.
pub fn host_effects_forbidden() -> bool {
    cfg!(test)
        || std::env::var(TEST_SANDBOX_ENV)
            .map(|value| value == "1")
            .unwrap_or(false)
}

fn refused(kind: EffectKind) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "host effect {kind:?} refused: running under a test sandbox \
             (cfg(test) or {TEST_SANDBOX_ENV}=1); inject a FakeHost instead"
        ),
    )
}

/// The production host: really runs programs, reads `$HOME` and prompts.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealHost;

impl HostEffects for RealHost {
    fn run(&self, command: &HostCommand) -> io::Result<HostOutput> {
        if host_effects_forbidden() {
            return Err(refused(command.kind));
        }
        real::run(command)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        if host_effects_forbidden() {
            return None;
        }
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
    }

    fn authorize_sensitive_export(&self, reason: &str) -> Result<(), AuthError> {
        if host_effects_forbidden() {
            return Err(AuthError::Unavailable);
        }
        crate::biometric::authorize_sensitive_export(reason)
    }

    fn devtools_page_urls(&self, addr: &str) -> io::Result<Vec<String>> {
        if host_effects_forbidden() {
            return Err(refused(EffectKind::BrowserDevTools));
        }
        real::devtools_page_urls(addr)
    }
}

/// Whether an install probe currently resolves on this host (a path that
/// exists, or a binary on `PATH`). Read-only; consent UIs use it to decide
/// whether to offer an app. Refused (reports `false`) under a test sandbox.
pub fn is_installed(probe: &InstallProbe) -> bool {
    if host_effects_forbidden() {
        return false;
    }
    real::is_installed(probe)
}

type Responder = dyn Fn(&HostCommand) -> io::Result<HostOutput> + Send + Sync;

/// A recording, scripted stand-in for the host. Touches nothing.
///
/// By default every command "fails" (non-zero exit, no output) — no process is
/// running, no Keychain item exists — the home directory is `None`,
/// authorization is granted and no DevTools endpoint answers. Each of those can
/// be overridden.
pub struct FakeHost {
    calls: Mutex<Vec<HostCommand>>,
    responder: Option<Box<Responder>>,
    home: Option<PathBuf>,
    deny_authorization: bool,
    authorizations: Mutex<Vec<String>>,
    devtools_urls: Option<Vec<String>>,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FakeHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeHost")
            .field("calls", &self.calls())
            .field("home", &self.home)
            .field("deny_authorization", &self.deny_authorization)
            .finish_non_exhaustive()
    }
}

impl FakeHost {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            responder: None,
            home: None,
            deny_authorization: false,
            authorizations: Mutex::new(Vec::new()),
            devtools_urls: None,
        }
    }

    /// Report `home` as the host's home directory.
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Answer [`HostEffects::run`] with `responder` instead of a failure.
    pub fn with_responder(
        mut self,
        responder: impl Fn(&HostCommand) -> io::Result<HostOutput> + Send + Sync + 'static,
    ) -> Self {
        self.responder = Some(Box::new(responder));
        self
    }

    /// Deny every sensitive-export authorization.
    pub fn denying_authorization(mut self) -> Self {
        self.deny_authorization = true;
        self
    }

    /// Answer DevTools queries with these page URLs.
    pub fn with_devtools_urls(mut self, urls: Vec<String>) -> Self {
        self.devtools_urls = Some(urls);
        self
    }

    /// Every command requested so far, in order.
    pub fn calls(&self) -> Vec<HostCommand> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The commands of one kind requested so far.
    pub fn calls_of(&self, kind: EffectKind) -> Vec<HostCommand> {
        self.calls()
            .into_iter()
            .filter(|call| call.kind == kind)
            .collect()
    }

    /// The reasons of every authorization requested so far.
    pub fn authorizations(&self) -> Vec<String> {
        self.authorizations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record(&self, command: &HostCommand) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.clone());
    }
}

impl HostEffects for FakeHost {
    fn run(&self, command: &HostCommand) -> io::Result<HostOutput> {
        self.record(command);
        match &self.responder {
            Some(responder) => responder(command),
            None => Ok(HostOutput::failed()),
        }
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn authorize_sensitive_export(&self, reason: &str) -> Result<(), AuthError> {
        self.authorizations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(reason.to_string());
        if self.deny_authorization {
            Err(AuthError::Denied)
        } else {
            Ok(())
        }
    }

    fn devtools_page_urls(&self, addr: &str) -> io::Result<Vec<String>> {
        self.record(&HostCommand::new(EffectKind::BrowserDevTools, addr));
        match &self.devtools_urls {
            Some(urls) => Ok(urls.clone()),
            None => Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("fake host: no DevTools endpoint at {addr}"),
            )),
        }
    }
}

/// The only code in this crate that spawns processes, opens sockets or
/// probes the host's installed apps.
mod real {
    use std::io::{self, Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use serde::Deserialize;

    use super::{HostCommand, HostOutput};

    fn command(spec: &HostCommand) -> Command {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        command
    }

    pub(super) fn run(spec: &HostCommand) -> io::Result<HostOutput> {
        let mut child = command(spec)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Drain both pipes on their own threads so a chatty child can never
        // block on a full pipe while we poll for its exit.
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(pipe) = stdout.as_mut() {
                let _ = pipe.read_to_end(&mut buffer);
            }
            buffer
        });
        let err = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(pipe) = stderr.as_mut() {
                let _ = pipe.read_to_end(&mut buffer);
            }
            buffer
        });
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if spec.timeout.is_some_and(|budget| start.elapsed() >= budget) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} did not finish within {} ms",
                        spec.program,
                        spec.timeout.unwrap_or_default().as_millis()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Ok(HostOutput {
            success: status.success(),
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        })
    }

    pub(super) fn is_installed(probe: &super::InstallProbe) -> bool {
        if probe.on_path {
            let Some(path) = std::env::var_os("PATH") else {
                return false;
            };
            std::env::split_paths(&path).any(|dir| dir.join(&probe.probe).is_file())
        } else {
            std::path::Path::new(&probe.probe).exists()
        }
    }

    /// One entry of the DevTools `/json` response.
    #[derive(Deserialize)]
    struct DevtoolsTarget {
        #[serde(default)]
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        url: String,
    }

    pub(super) fn devtools_page_urls(addr: &str) -> io::Result<Vec<String>> {
        let socket_addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "no address"))?;
        let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_millis(300))?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let request = format!(
            "GET /json HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes())?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        let text = String::from_utf8_lossy(&response);
        let body_start = text.find("\r\n\r\n").map(|index| index + 4).unwrap_or(0);
        let body = &text[body_start..];
        let targets: Vec<DevtoolsTarget> = serde_json::from_str(body.trim()).unwrap_or_default();
        Ok(targets
            .into_iter()
            .filter(|target| target.kind == "page" && !target.url.is_empty())
            .map(|target| target.url)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crate's own tests must never get a real host by default.
    #[test]
    fn default_host_is_fake_under_test() {
        let host = default_host();
        assert!(host.home_dir().is_none());
        let output = host
            .run(&HostCommand::new(EffectKind::ProcessLookup, "pgrep").arg("anything"))
            .unwrap();
        assert!(!output.success);
    }

    /// Even an explicitly constructed RealHost refuses to act under test: it
    /// fails closed with PermissionDenied for every kind of effect and reports
    /// no home directory, so a test that bypassed injection cannot reach
    /// pgrep/osascript/security, the install probes or the real `$HOME`.
    #[test]
    fn real_host_refuses_every_effect_under_test() {
        assert!(host_effects_forbidden());
        let host = RealHost;
        for (kind, program) in [
            (EffectKind::ProcessLookup, "pgrep"),
            (EffectKind::AppleScript, "osascript"),
            (EffectKind::KeychainRead, "security"),
        ] {
            let error = host
                .run(&HostCommand::new(kind, program).arg("--version"))
                .expect_err("real host must refuse under test");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{kind:?}");
        }
        assert!(host.home_dir().is_none());
        assert!(!is_installed(&InstallProbe::path("/")));
        assert!(!is_installed(&InstallProbe::on_path("sh")));
        assert!(host.devtools_page_urls("127.0.0.1:9222").is_err());
        assert!(host.authorize_sensitive_export("test").is_err());
    }

    #[test]
    fn fake_host_records_and_scripts() {
        let host = FakeHost::new()
            .with_home("/fake/home")
            .with_responder(|command| {
                Ok(if command.kind == EffectKind::KeychainRead {
                    HostOutput::ok("secret\n")
                } else {
                    HostOutput::failed()
                })
            });
        let read = host
            .run(&HostCommand::new(EffectKind::KeychainRead, "security"))
            .unwrap();
        assert_eq!(read.stdout, b"secret\n");
        let _ = host.run(&HostCommand::new(EffectKind::ProcessLookup, "pgrep"));
        assert_eq!(host.calls().len(), 2);
        assert_eq!(host.calls_of(EffectKind::ProcessLookup).len(), 1);
        assert_eq!(host.home_dir(), Some(PathBuf::from("/fake/home")));
    }

    /// Structural guard: host effects live in exactly one file. Any other
    /// source file in this crate that names a process or socket API is a new
    /// path to the real machine that bypasses injection.
    #[test]
    fn no_source_file_outside_host_rs_spawns_or_connects() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs")
                    || path.file_name().is_some_and(|name| name == "host.rs")
                {
                    continue;
                }
                // `HostCommand::new` is the sanctioned way to *describe* an
                // effect; only a bare `Command::new` spawns one.
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("HostCommand::new", "");
                for needle in [
                    "process::Command",
                    "Command::new",
                    "TcpStream",
                    "UdpSocket",
                    "std::net",
                    "var_os(\"HOME\")",
                    "var(\"HOME\")",
                    "var_os(\"PATH\")",
                    "set_var",
                    "remove_var",
                ] {
                    if text.contains(needle) {
                        offenders.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "host effects outside host.rs: {offenders:#?}"
        );
    }
}
