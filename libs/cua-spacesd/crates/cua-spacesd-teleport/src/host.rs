// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The single choke point for every side effect a provider has on the host.
//!
//! Importing a session is inherently invasive on the destination: it looks up
//! and terminates running apps (`pgrep`/`pkill`), writes the login Keychain
//! (`security`), inspects code signatures (`codesign`), launches the imported
//! app and resolves `$HOME`. None of that may ever happen from a test on a
//! developer's machine: a unit test that imports a "Slack" bundle must not
//! `pkill -f slack` the user's real Slack, and a Keychain test must not write
//! real credentials. (Reading a source machine — AppleScript, DevTools, Touch
//! ID — is the sender's business and lives in `cua-teleport`.)
//!
//! So importers never spawn a process, open a socket or read `$HOME`
//! themselves. They go through a [`HostEffects`] they were constructed with:
//!
//! - [`RealHost`] is the production implementation. It refuses every effect
//!   (and reports no home directory) when compiled into this crate's own tests
//!   or when [`TEST_SANDBOX_ENV`]` = 1` is set, so a test that forgot to inject
//!   a fake fails closed instead of touching the machine.
//! - [`FakeHost`] records every requested effect and answers from a scripted
//!   responder. It is the default for every provider constructor under
//!   `cfg(test)`, and what integration tests in other crates must inject.
//!
//! A source scan in this module's tests pins the structure: no file in this
//! crate other than `host.rs` may name `std::process::Command` or open a
//! `TcpStream`.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Setting this to `1` makes [`RealHost`] refuse every host effect. Test
/// harnesses that link this crate (cua-spacesd-server's integration tests, e2e
/// drivers) set it so a missing injection fails closed.
pub const TEST_SANDBOX_ENV: &str = "CUA_ENV_TEST_SANDBOX";

/// What a host effect does, so a fake can assert on intent rather than on a
/// command line, and so a denial can say what was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    /// Is a process running? (`pgrep`)
    ProcessLookup,
    /// Stop a running process before its profile is replaced. (`pkill`)
    ProcessTerminate,
    /// Launch an application (the imported app's [`crate::LaunchSpec`]).
    AppLaunch,
    /// Read Keychain configuration. (`security list-keychains`, …)
    KeychainRead,
    /// Write, delete, unlock or reconfigure Keychain items. (`security add-*`)
    KeychainWrite,
    /// Inspect an installed app's code signature. (`codesign -dv`)
    CodeSignInspect,
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
    /// Bytes written to the child's stdin, then closed. This is how secrets
    /// reach a child: argv is world-readable (`ps`), stdin is not. `None`
    /// connects stdin to `/dev/null`.
    pub stdin: Option<StdinBytes>,
}

/// Bytes fed to a child's stdin. Treated as secret: `Debug` never prints
/// them, so a logged or recorded [`HostCommand`] cannot leak them.
#[derive(Clone, PartialEq, Eq)]
pub struct StdinBytes(Vec<u8>);

impl StdinBytes {
    /// Wraps `bytes`.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for StdinBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StdinBytes(<{} bytes redacted>)", self.0.len())
    }
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
            stdin: None,
        }
    }

    /// Feed `bytes` to the child's stdin (and close it).
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(StdinBytes::new(bytes));
        self
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

/// Every host side effect an importer may perform.
pub trait HostEffects: Send + Sync {
    /// Run a program to completion and capture its output.
    fn run(&self, command: &HostCommand) -> io::Result<HostOutput>;

    /// Spawn a program detached (an app launch) and return its pid.
    fn spawn(&self, command: &HostCommand) -> io::Result<u32>;

    /// The destination user's home directory (where the default Keychain
    /// lives), or `None` when there is none (or it must not be touched).
    fn home_dir(&self) -> Option<PathBuf>;
}

/// The host effects every importer constructor uses when none is injected:
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

/// The production host: really runs programs and reads `$HOME`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealHost;

impl HostEffects for RealHost {
    fn run(&self, command: &HostCommand) -> io::Result<HostOutput> {
        if host_effects_forbidden() {
            return Err(refused(command.kind));
        }
        real::run(command)
    }

    fn spawn(&self, command: &HostCommand) -> io::Result<u32> {
        if host_effects_forbidden() {
            return Err(refused(command.kind));
        }
        real::spawn(command)
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
}

type Responder = dyn Fn(&HostCommand) -> io::Result<HostOutput> + Send + Sync;

/// A recording, scripted stand-in for the host. Touches nothing.
///
/// By default every command "fails" (non-zero exit, no output) — no process is
/// running, no Keychain is configured — launches report a fake pid and the
/// home directory is `None`. Each of those can be overridden.
pub struct FakeHost {
    calls: Mutex<Vec<HostCommand>>,
    responder: Option<Box<Responder>>,
    home: Option<PathBuf>,
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
            .finish_non_exhaustive()
    }
}

impl FakeHost {
    /// The pid [`HostEffects::spawn`] reports for a fake launch.
    pub const FAKE_PID: u32 = 0;

    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            responder: None,
            home: None,
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

    /// Every command requested so far, in order (runs and spawns).
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

    fn spawn(&self, command: &HostCommand) -> io::Result<u32> {
        self.record(command);
        Ok(Self::FAKE_PID)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// The only code in this crate that spawns processes.
mod real {
    use std::io::{self, Read};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

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
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Feed stdin on its own thread and drop the pipe when done, so the
        // child sees EOF; a child that never reads cannot block us.
        let feeder = match (child.stdin.take(), &spec.stdin) {
            (Some(mut pipe), Some(bytes)) => {
                let bytes = bytes.as_bytes().to_vec();
                Some(std::thread::spawn(move || {
                    use std::io::Write as _;
                    let _ = pipe.write_all(&bytes);
                }))
            }
            _ => None,
        };
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
        if let Some(feeder) = feeder {
            let _ = feeder.join();
        }
        Ok(HostOutput {
            success: status.success(),
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        })
    }

    pub(super) fn spawn(spec: &HostCommand) -> io::Result<u32> {
        if spec.stdin.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stdin is only supported for run(), not spawn()",
            ));
        }
        Ok(command(spec).spawn()?.id())
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
    /// pkill/pgrep/security/codesign/open or the real `$HOME`.
    #[test]
    fn real_host_refuses_every_effect_under_test() {
        assert!(host_effects_forbidden());
        let host = RealHost;
        for (kind, program) in [
            (EffectKind::ProcessLookup, "pgrep"),
            (EffectKind::ProcessTerminate, "pkill"),
            (EffectKind::KeychainRead, "security"),
            (EffectKind::KeychainWrite, "security"),
            (EffectKind::CodeSignInspect, "codesign"),
        ] {
            let error = host
                .run(&HostCommand::new(kind, program).arg("--version"))
                .expect_err("real host must refuse under test");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{kind:?}");
        }
        let error = host
            .spawn(&HostCommand::new(EffectKind::AppLaunch, "open").arg("-a"))
            .expect_err("real host must refuse launches under test");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(host.home_dir().is_none());
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
        assert_eq!(
            host.spawn(&HostCommand::new(EffectKind::AppLaunch, "open"))
                .unwrap(),
            FakeHost::FAKE_PID
        );
        assert_eq!(host.calls().len(), 2);
        assert_eq!(host.calls_of(EffectKind::AppLaunch).len(), 1);
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
