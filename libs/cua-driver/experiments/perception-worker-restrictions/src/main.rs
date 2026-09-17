use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROBE_INPUT: &str = "synthetic-request-7c1a\n";
const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const BLOCKED_STDIN_TEST_BYTES: usize = MAX_REQUEST_BYTES;

fn main() {
    let result = match env::args().nth(1).as_deref() {
        Some("self-test") => self_test(),
        Some("worker-probe") => worker_probe(),
        Some("worker-sleep") => worker_sleep(),
        Some("worker-never-read") => worker_never_read(),
        Some("worker-descendant-retains-stdio") => worker_descendant_retains_stdio(),
        Some("worker-exit-descendant-retains-stdio") => worker_exit_descendant_retains_stdio(),
        Some("worker-close-stdout-sleep") => worker_close_stdout_sleep(),
        Some("worker-flood") => worker_flood(),
        Some("worker-allocate") => worker_allocate(),
        _ => Err("usage: cua-perception-worker-restrictions self-test".into()),
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn self_test() -> Result<(), String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let private_dir = PrivateWorkingDirectory::create()?;
    assert_private_permissions(private_dir.path())?;
    let inherited_sentinel = InheritanceSentinel::create(private_dir.path())?;

    let probe = run_worker(
        &executable,
        "worker-probe",
        PROBE_INPUT.as_bytes(),
        private_dir.path(),
        &inherited_sentinel,
        Limits {
            deadline: Duration::from_secs(2),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )?;
    require_success(&probe, b"probe-ok\n")?;
    println!("probe: enforced (stdio IPC, private cwd/temp, env allowlist, owned sentinel closed)");

    let oversized_request = vec![0_u8; MAX_REQUEST_BYTES + 1];
    let request_error = run_worker(
        &executable,
        "worker-probe",
        &oversized_request,
        private_dir.path(),
        &inherited_sentinel,
        Limits {
            deadline: Duration::from_secs(2),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )
    .expect_err("oversized request must be rejected before spawn");
    if !request_error.contains("request exceeds") {
        return Err(format!(
            "unexpected oversized request error: {request_error}"
        ));
    }
    println!("request: oversized payload rejected before spawn");

    let deadline = run_worker(
        &executable,
        "worker-sleep",
        b"",
        private_dir.path(),
        &inherited_sentinel,
        Limits {
            deadline: Duration::from_millis(100),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )?;
    match deadline {
        Outcome::DeadlineExceeded {
            reaped: true,
            input_stopped: true,
            ..
        } => {
            println!("deadline: enforced, child reaped");
        }
        other => return Err(format!("deadline test returned {other:?}")),
    }

    let blocked_input = vec![0x5a_u8; BLOCKED_STDIN_TEST_BYTES];
    let blocked_stdin = run_worker(
        &executable,
        "worker-never-read",
        &blocked_input,
        private_dir.path(),
        &inherited_sentinel,
        Limits {
            deadline: Duration::from_millis(100),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )?;
    match blocked_stdin {
        Outcome::DeadlineExceeded {
            reaped: true,
            input_stopped: true,
            input_started: true,
            input_was_pending: true,
            output,
            io_stopped: true,
        } if output == b"never-read-ready\n" => {
            println!(
                "blocked-stdin deadline: synchronized never-read request remained pending; I/O stopped, direct child reaped"
            );
        }
        other => return Err(format!("blocked-stdin test returned {other:?}")),
    }

    descendant_stdio_self_test(
        &executable,
        private_dir.path(),
        &inherited_sentinel,
        &blocked_input,
    )?;
    exited_leader_stdio_self_test(
        &executable,
        private_dir.path(),
        &inherited_sentinel,
        &blocked_input,
    )?;
    close_stdout_deadline_self_test(&executable, private_dir.path(), &inherited_sentinel)?;

    let output = run_worker(
        &executable,
        "worker-flood",
        b"",
        private_dir.path(),
        &inherited_sentinel,
        Limits {
            deadline: Duration::from_secs(2),
            max_output_bytes: 4096,
            memory_bytes: None,
        },
    )?;
    match output {
        Outcome::OutputExceeded {
            reaped: true,
            input_stopped: true,
        } => {
            println!("output: enforced, direct child reaped");
        }
        other => return Err(format!("output test returned {other:?}")),
    }

    memory_self_test(&executable, private_dir.path(), &inherited_sentinel)?;
    Ok(())
}

#[cfg(unix)]
fn close_stdout_deadline_self_test(
    executable: &Path,
    working_dir: &Path,
    sentinel: &InheritanceSentinel,
) -> Result<(), String> {
    let outcome = run_worker(
        executable,
        "worker-close-stdout-sleep",
        b"",
        working_dir,
        sentinel,
        Limits {
            deadline: Duration::from_millis(100),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )?;
    match outcome {
        Outcome::DeadlineExceeded {
            reaped: true,
            input_stopped: true,
            output,
            io_stopped: true,
            ..
        } if output.is_empty() => {
            println!(
                "closed stdout: non-reaping liveness kept supervision active through deadline; direct child reaped"
            );
            Ok(())
        }
        other => Err(format!("close-stdout test returned {other:?}")),
    }
}

#[cfg(windows)]
fn close_stdout_deadline_self_test(
    _executable: &Path,
    _working_dir: &Path,
    _sentinel: &InheritanceSentinel,
) -> Result<(), String> {
    println!("closed stdout: adversarial close test is unsupported on Windows in this spike");
    Ok(())
}

#[cfg(unix)]
fn descendant_stdio_self_test(
    executable: &Path,
    working_dir: &Path,
    sentinel: &InheritanceSentinel,
    input: &[u8],
) -> Result<(), String> {
    let outcome = run_worker(
        executable,
        "worker-descendant-retains-stdio",
        input,
        working_dir,
        sentinel,
        Limits {
            deadline: Duration::from_millis(150),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )?;
    match outcome {
        Outcome::DeadlineExceeded {
            reaped: true,
            input_stopped: true,
            input_started: true,
            input_was_pending: true,
            output,
            io_stopped: true,
        } if output == b"descendant-retains-stdio-ready\n" => {
            println!(
                "descendant stdio: Unix process group terminated; I/O stopped, direct child reaped"
            );
            Ok(())
        }
        other => Err(format!("descendant-stdio test returned {other:?}")),
    }
}

#[cfg(unix)]
fn exited_leader_stdio_self_test(
    executable: &Path,
    working_dir: &Path,
    sentinel: &InheritanceSentinel,
    input: &[u8],
) -> Result<(), String> {
    let outcome = run_worker(
        executable,
        "worker-exit-descendant-retains-stdio",
        input,
        working_dir,
        sentinel,
        Limits {
            deadline: Duration::from_millis(200),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: None,
        },
    )?;
    match outcome {
        Outcome::DeadlineExceeded {
            reaped: true,
            input_stopped: true,
            input_started: true,
            input_was_pending: true,
            output,
            io_stopped: true,
        } if output == b"direct-exited-descendant-retains-stdio-ready\n" => {
            println!(
                "exited group leader: identity preserved until group termination; I/O stopped, direct child reaped"
            );
            Ok(())
        }
        other => Err(format!("exited-leader test returned {other:?}")),
    }
}

#[cfg(windows)]
fn exited_leader_stdio_self_test(
    _executable: &Path,
    _working_dir: &Path,
    _sentinel: &InheritanceSentinel,
    _input: &[u8],
) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn descendant_stdio_self_test(
    _executable: &Path,
    _working_dir: &Path,
    _sentinel: &InheritanceSentinel,
    _input: &[u8],
) -> Result<(), String> {
    println!("descendant stdio: unsupported on Windows; Job Object containment is required");
    Ok(())
}

#[cfg(target_os = "linux")]
fn memory_self_test(
    executable: &Path,
    working_dir: &Path,
    sentinel: &InheritanceSentinel,
) -> Result<(), String> {
    let outcome = run_worker(
        executable,
        "worker-allocate",
        b"",
        working_dir,
        sentinel,
        Limits {
            deadline: Duration::from_secs(4),
            max_output_bytes: MAX_OUTPUT_BYTES,
            memory_bytes: Some(96 * 1024 * 1024),
        },
    )?;

    match outcome {
        Outcome::Completed {
            status,
            reaped: true,
            ..
        } if !status.success() => {
            println!(
                "memory: {} prevented the synthetic over-allocation, child reaped",
                memory_limit_name()
            );
            Ok(())
        }
        other => Err(format!("memory test returned {other:?}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn memory_self_test(
    _executable: &Path,
    _working_dir: &Path,
    _sentinel: &InheritanceSentinel,
) -> Result<(), String> {
    println!("memory: unsupported on this platform; native enforcement is required");
    Ok(())
}

#[derive(Clone, Copy)]
struct Limits {
    deadline: Duration,
    max_output_bytes: usize,
    memory_bytes: Option<u64>,
}

#[derive(Debug)]
enum Outcome {
    Completed {
        status: ExitStatus,
        output: Vec<u8>,
        reaped: bool,
    },
    DeadlineExceeded {
        reaped: bool,
        input_stopped: bool,
        input_started: bool,
        input_was_pending: bool,
        output: Vec<u8>,
        io_stopped: bool,
    },
    OutputExceeded {
        reaped: bool,
        input_stopped: bool,
    },
}

fn run_worker(
    executable: &Path,
    mode: &str,
    input: &[u8],
    working_dir: &Path,
    sentinel: &InheritanceSentinel,
    limits: Limits,
) -> Result<Outcome, String> {
    if input.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "request exceeds {MAX_REQUEST_BYTES}-byte input limit"
        ));
    }

    #[cfg(windows)]
    if limits.memory_bytes.is_some() {
        return Err("Windows memory limiting is intentionally unsupported by this spike".into());
    }

    let mut command = Command::new(executable);
    command
        .arg(mode)
        .current_dir(working_dir)
        .env_clear()
        .env("CUA_WORKER_DIR", working_dir)
        .env("CUA_WORKER_SENTINEL", sentinel.encoded_value())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_temp_environment(&mut command, working_dir);
    configure_platform_environment(&mut command);
    configure_process_group(&mut command);
    configure_memory_limit(&mut command, limits.memory_bytes)?;

    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn {mode}: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("worker stdin was not piped")?;
    let input = input.to_vec();
    let (input_tx, input_rx) = mpsc::channel();
    let input_thread = thread::spawn(move || {
        if input_tx.send(InputWrite::Started).is_err() {
            return;
        }
        let result = stdin
            .write_all(&input)
            .map_err(|error| error.to_string())
            .map(|()| InputWrite::Complete)
            .unwrap_or_else(InputWrite::Failed);
        let _ = input_tx.send(result);
    });

    let mut stdout = child.stdout.take().ok_or("worker stdout was not piped")?;
    let (output_tx, output_rx) = mpsc::channel();
    let (output_release_tx, output_release_rx) = mpsc::channel();
    let output_limit = limits.max_output_bytes;
    let output_thread = thread::spawn(move || {
        let mut total = 0_usize;
        let mut buffer = [0_u8; 4096];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => {
                    let _ = output_tx.send(OutputRead::Eof);
                    return;
                }
                Ok(read) if total.saturating_add(read) > output_limit => {
                    let _ = output_tx.send(OutputRead::Exceeded);
                    let _ = output_release_rx.recv();
                    let _ = output_tx.send(OutputRead::Stopped);
                    return;
                }
                Ok(read) => {
                    total += read;
                    if output_tx
                        .send(OutputRead::Chunk(buffer[..read].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = output_tx.send(OutputRead::Failed(error.to_string()));
                    return;
                }
            }
        }
    });

    let mut input_stopped = false;
    let mut input_started = false;
    let mut output_stopped = false;
    let mut output = Vec::new();
    loop {
        if !input_stopped {
            match input_rx.try_recv() {
                Ok(InputWrite::Started) => input_started = true,
                Ok(InputWrite::Complete) => input_stopped = true,
                Ok(InputWrite::Failed(error)) => {
                    let _ = terminate_and_reap(
                        &mut child,
                        TerminationReason::Output,
                        CancellationState {
                            input_rx: &input_rx,
                            input_stopped: true,
                            input_started,
                            output_rx: &output_rx,
                            output_release_tx: &output_release_tx,
                            output_stopped,
                            output,
                            input_thread,
                            output_thread,
                        },
                    );
                    return Err(format!("write worker stdin: {error}"));
                }
                Err(TryRecvError::Disconnected) => {
                    let _ = terminate_and_reap(
                        &mut child,
                        TerminationReason::Output,
                        CancellationState {
                            input_rx: &input_rx,
                            input_stopped: true,
                            input_started,
                            output_rx: &output_rx,
                            output_release_tx: &output_release_tx,
                            output_stopped,
                            output,
                            input_thread,
                            output_thread,
                        },
                    );
                    return Err("worker input channel disconnected".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if !output_stopped {
            match output_rx.try_recv() {
                Ok(OutputRead::Chunk(chunk)) => output.extend_from_slice(&chunk),
                Ok(OutputRead::Eof) => output_stopped = true,
                Ok(OutputRead::Exceeded) => {
                    return terminate_and_reap(
                        &mut child,
                        TerminationReason::Output,
                        CancellationState {
                            input_rx: &input_rx,
                            input_stopped,
                            input_started,
                            output_rx: &output_rx,
                            output_release_tx: &output_release_tx,
                            output_stopped,
                            output,
                            input_thread,
                            output_thread,
                        },
                    );
                }
                Ok(OutputRead::Failed(error)) => {
                    let _ = terminate_and_reap(
                        &mut child,
                        TerminationReason::Output,
                        CancellationState {
                            input_rx: &input_rx,
                            input_stopped,
                            input_started,
                            output_rx: &output_rx,
                            output_release_tx: &output_release_tx,
                            output_stopped: true,
                            output,
                            input_thread,
                            output_thread,
                        },
                    );
                    return Err(format!("read worker stdout: {error}"));
                }
                Ok(OutputRead::Stopped) => output_stopped = true,
                Err(TryRecvError::Disconnected) => {
                    let _ = terminate_and_reap(
                        &mut child,
                        TerminationReason::Output,
                        CancellationState {
                            input_rx: &input_rx,
                            input_stopped,
                            input_started,
                            output_rx: &output_rx,
                            output_release_tx: &output_release_tx,
                            output_stopped,
                            output,
                            input_thread,
                            output_thread,
                        },
                    );
                    return Err("worker output channel disconnected".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if started.elapsed() >= limits.deadline {
            return terminate_and_reap(
                &mut child,
                TerminationReason::Deadline,
                CancellationState {
                    input_rx: &input_rx,
                    input_stopped,
                    input_started,
                    output_rx: &output_rx,
                    output_release_tx: &output_release_tx,
                    output_stopped,
                    output,
                    input_thread,
                    output_thread,
                },
            );
        }
        if input_stopped && output_stopped && child_exited_without_reaping(&child)? {
            let status = child
                .wait()
                .map_err(|error| format!("reap completed worker: {error}"))?;
            join_io_threads(input_thread, output_thread)?;
            return Ok(Outcome::Completed {
                status,
                output,
                reaped: true,
            });
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn child_exited_without_reaping(child: &Child) -> Result<bool, String> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let child_id = libc::id_t::try_from(child.id())
        .map_err(|_| "child process ID does not fit id_t".to_string())?;
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child_id,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        return Err(format!(
            "query child liveness without reaping: {}",
            io::Error::last_os_error()
        ));
    }
    let info = unsafe { info.assume_init() };
    Ok(unsafe { info.si_pid() } != 0)
}

#[cfg(windows)]
fn child_exited_without_reaping(child: &Child) -> Result<bool, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;

    let mut exit_code = 0_u32;
    let result = unsafe { GetExitCodeProcess(child.as_raw_handle() as HANDLE, &mut exit_code) };
    if result == 0 {
        return Err(format!(
            "query Windows child liveness without reaping: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(exit_code != STILL_ACTIVE as u32)
}

enum InputWrite {
    Started,
    Complete,
    Failed(String),
}

enum OutputRead {
    Chunk(Vec<u8>),
    Eof,
    Exceeded,
    Stopped,
    Failed(String),
}

enum TerminationReason {
    Deadline,
    Output,
}

struct CancellationState<'a> {
    input_rx: &'a mpsc::Receiver<InputWrite>,
    input_stopped: bool,
    input_started: bool,
    output_rx: &'a mpsc::Receiver<OutputRead>,
    output_release_tx: &'a mpsc::Sender<()>,
    output_stopped: bool,
    output: Vec<u8>,
    input_thread: thread::JoinHandle<()>,
    output_thread: thread::JoinHandle<()>,
}

fn terminate_and_reap(
    child: &mut Child,
    reason: TerminationReason,
    state: CancellationState<'_>,
) -> Result<Outcome, String> {
    let mut input_was_pending = state.input_started && !state.input_stopped;
    terminate_worker_scope(child)?;
    child
        .wait()
        .map_err(|error| format!("reap terminated worker: {error}"))?;
    let _ = state.output_release_tx.send(());
    let mut input_started = state.input_started;
    let input_stopped = if state.input_stopped {
        true
    } else {
        let stop_by = Instant::now() + Duration::from_secs(1);
        loop {
            let remaining = stop_by.saturating_duration_since(Instant::now());
            match state.input_rx.recv_timeout(remaining) {
                Ok(InputWrite::Started) => {
                    input_started = true;
                    input_was_pending = true;
                }
                Ok(InputWrite::Complete | InputWrite::Failed(_)) => break true,
                Err(error) => {
                    return Err(format!(
                        "stdin writer did not stop after child reap: {error}"
                    ))
                }
            }
        }
    };
    let mut output = state.output;
    let io_stopped = if state.output_stopped {
        true
    } else {
        let stop_by = Instant::now() + Duration::from_secs(1);
        loop {
            let remaining = stop_by.saturating_duration_since(Instant::now());
            match state.output_rx.recv_timeout(remaining) {
                Ok(OutputRead::Chunk(chunk)) => output.extend_from_slice(&chunk),
                Ok(OutputRead::Eof | OutputRead::Stopped) => break true,
                Ok(OutputRead::Exceeded) => continue,
                Ok(OutputRead::Failed(error)) => {
                    return Err(format!("read worker stdout after cancellation: {error}"))
                }
                Err(error) => {
                    return Err(format!(
                        "stdout reader did not stop after worker-scope termination: {error}"
                    ))
                }
            }
        }
    };
    join_io_threads(state.input_thread, state.output_thread)?;

    Ok(match reason {
        TerminationReason::Deadline => Outcome::DeadlineExceeded {
            reaped: true,
            input_stopped,
            input_started,
            input_was_pending,
            output,
            io_stopped,
        },
        TerminationReason::Output => Outcome::OutputExceeded {
            reaped: true,
            input_stopped,
        },
    })
}

fn join_io_threads(
    input_thread: thread::JoinHandle<()>,
    output_thread: thread::JoinHandle<()>,
) -> Result<(), String> {
    input_thread
        .join()
        .map_err(|_| "stdin writer thread panicked".to_string())?;
    output_thread
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())?;
    Ok(())
}

#[cfg(unix)]
fn terminate_worker_scope(child: &mut Child) -> Result<(), String> {
    let process_group = libc::pid_t::try_from(child.id())
        .map_err(|_| "child process ID does not fit pid_t".to_string())?;
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("terminate Unix worker process group: {error}"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn terminate_worker_scope(child: &mut Child) -> Result<(), String> {
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => Err(format!("terminate direct Windows worker: {error}")),
    }
}

fn require_success(outcome: &Outcome, expected_output: &[u8]) -> Result<(), String> {
    match outcome {
        Outcome::Completed {
            status,
            output,
            reaped: true,
        } if status.success() && output == expected_output => Ok(()),
        other => Err(format!("probe test returned {other:?}")),
    }
}

fn configure_temp_environment(command: &mut Command, working_dir: &Path) {
    #[cfg(unix)]
    command.env("TMPDIR", working_dir);
    #[cfg(windows)]
    command.env("TEMP", working_dir).env("TMP", working_dir);
}

#[cfg(unix)]
fn configure_platform_environment(_command: &mut Command) {}

#[cfg(windows)]
fn configure_platform_environment(command: &mut Command) {
    if let Some(system_root) = env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn configure_process_group(_command: &mut Command) {}

#[cfg(target_os = "linux")]
fn configure_memory_limit(command: &mut Command, memory_bytes: Option<u64>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    if let Some(memory_bytes) = memory_bytes {
        let limit = libc::rlimit {
            rlim_cur: memory_bytes as libc::rlim_t,
            rlim_max: memory_bytes as libc::rlim_t,
        };
        unsafe {
            command.pre_exec(move || {
                if libc::setrlimit(libc::RLIMIT_AS, &limit) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn memory_limit_name() -> &'static str {
    "RLIMIT_AS"
}

#[cfg(not(target_os = "linux"))]
fn configure_memory_limit(_command: &mut Command, memory_bytes: Option<u64>) -> Result<(), String> {
    if memory_bytes.is_some() {
        Err("memory limiting requires a platform-native launcher on this operating system".into())
    } else {
        Ok(())
    }
}

fn worker_probe() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| error.to_string())?;
    if input != PROBE_INPUT {
        return Err("worker received unexpected stdin payload".into());
    }

    let expected_dir = PathBuf::from(required_env("CUA_WORKER_DIR")?)
        .canonicalize()
        .map_err(|error| format!("canonicalize worker directory: {error}"))?;
    let actual_dir = env::current_dir().map_err(|error| error.to_string())?;
    if actual_dir != expected_dir {
        return Err(format!(
            "worker cwd mismatch: expected {}, got {}",
            expected_dir.display(),
            actual_dir.display()
        ));
    }
    verify_temp_environment(&expected_dir)?;
    verify_environment_allowlist()?;
    verify_sentinel_closed(&required_env("CUA_WORKER_SENTINEL")?)?;
    println!("probe-ok");
    Ok(())
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("missing required environment variable {name}"))
}

fn verify_temp_environment(expected_dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let names = ["TMPDIR"].as_slice();
    #[cfg(windows)]
    let names = ["TEMP", "TMP"].as_slice();

    for name in names {
        let value = PathBuf::from(required_env(name)?)
            .canonicalize()
            .map_err(|error| format!("canonicalize {name}: {error}"))?;
        if value != expected_dir {
            return Err(format!("{name} does not point to the private directory"));
        }
    }
    Ok(())
}

fn verify_environment_allowlist() -> Result<(), String> {
    let actual: BTreeSet<String> = env::vars().map(|(name, _)| name).collect();
    let mut expected = BTreeSet::from([
        "CUA_WORKER_DIR".to_string(),
        "CUA_WORKER_SENTINEL".to_string(),
    ]);
    #[cfg(unix)]
    expected.insert("TMPDIR".to_string());
    #[cfg(windows)]
    {
        expected.insert("TEMP".to_string());
        expected.insert("TMP".to_string());
        if env::var_os("SystemRoot").is_some() {
            expected.insert("SystemRoot".to_string());
        }
    }

    if actual != expected {
        return Err(format!(
            "environment differs from allowlist: actual={actual:?}, expected={expected:?}"
        ));
    }
    Ok(())
}

fn worker_sleep() -> Result<(), String> {
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn worker_never_read() -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"never-read-ready\n")
        .map_err(|error| error.to_string())?;
    stdout.flush().map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

#[cfg(unix)]
fn worker_descendant_retains_stdio() -> Result<(), String> {
    let child_pid = unsafe { libc::fork() };
    if child_pid == -1 {
        return Err(format!(
            "fork synthetic descendant: {}",
            io::Error::last_os_error()
        ));
    }
    if child_pid == 0 {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"descendant-retains-stdio-ready\n");
        let _ = stdout.flush();
        thread::sleep(Duration::from_secs(30));
        unsafe { libc::_exit(0) }
    }
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

#[cfg(unix)]
fn worker_exit_descendant_retains_stdio() -> Result<(), String> {
    let mut exit_signal = [-1; 2];
    if unsafe { libc::pipe(exit_signal.as_mut_ptr()) } == -1 {
        return Err(format!(
            "create synthetic exit signal: {}",
            io::Error::last_os_error()
        ));
    }
    let child_pid = unsafe { libc::fork() };
    if child_pid == -1 {
        unsafe {
            libc::close(exit_signal[0]);
            libc::close(exit_signal[1]);
        }
        return Err(format!(
            "fork synthetic exited-leader descendant: {}",
            io::Error::last_os_error()
        ));
    }
    if child_pid == 0 {
        unsafe { libc::close(exit_signal[1]) };
        let mut byte = 0_u8;
        loop {
            let read = unsafe {
                libc::read(
                    exit_signal[0],
                    (&mut byte as *mut u8).cast::<libc::c_void>(),
                    1,
                )
            };
            if read == 0 {
                break;
            }
            if read == -1 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                unsafe { libc::_exit(2) }
            }
        }
        unsafe { libc::close(exit_signal[0]) };
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"direct-exited-descendant-retains-stdio-ready\n");
        let _ = stdout.flush();
        thread::sleep(Duration::from_secs(30));
        unsafe { libc::_exit(0) }
    }
    unsafe { libc::close(exit_signal[0]) };
    // Keep the write end open until process exit; the descendant's EOF is the proof.
    Ok(())
}

#[cfg(unix)]
fn worker_close_stdout_sleep() -> Result<(), String> {
    if unsafe { libc::close(libc::STDOUT_FILENO) } == -1 {
        return Err(format!(
            "close synthetic worker stdout: {}",
            io::Error::last_os_error()
        ));
    }
    thread::sleep(Duration::from_secs(30));
    Ok(())
}

#[cfg(windows)]
fn worker_descendant_retains_stdio() -> Result<(), String> {
    Err("descendant containment is unsupported on Windows in this spike".into())
}

#[cfg(windows)]
fn worker_exit_descendant_retains_stdio() -> Result<(), String> {
    Err("descendant containment is unsupported on Windows in this spike".into())
}

#[cfg(windows)]
fn worker_close_stdout_sleep() -> Result<(), String> {
    Err("closing stdout adversarially is unsupported on Windows in this spike".into())
}

fn worker_flood() -> Result<(), String> {
    let block = [b'x'; 4096];
    let mut stdout = io::stdout().lock();
    loop {
        stdout
            .write_all(&block)
            .map_err(|error| error.to_string())?;
    }
}

fn worker_allocate() -> Result<(), String> {
    let mut allocations = Vec::new();
    loop {
        allocations.push(vec![0x5a_u8; 8 * 1024 * 1024]);
        std::hint::black_box(&allocations);
    }
}

struct PrivateWorkingDirectory {
    path: PathBuf,
}

impl PrivateWorkingDirectory {
    fn create() -> Result<Self, String> {
        let base = env::temp_dir();
        let process_id = std::process::id();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        for attempt in 0..32_u8 {
            let path = base.join(format!(
                "cua-perception-worker-{process_id}-{timestamp}-{attempt}"
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    set_private_permissions(&path)?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("create private working directory: {error}")),
            }
        }
        Err("could not allocate a unique private working directory".into())
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateWorkingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("set private working directory permissions: {error}"))
}

#[cfg(windows)]
fn set_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn assert_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o700 {
        return Err(format!(
            "private working directory mode is {mode:o}, not 700"
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn assert_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

struct InheritanceSentinel {
    file: File,
}

impl InheritanceSentinel {
    fn create(directory: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(directory.join("inheritance-sentinel"))
            .map_err(|error| format!("create inheritance sentinel: {error}"))?;
        mark_not_inheritable(&file)?;
        Ok(Self { file })
    }

    fn encoded_value(&self) -> String {
        encoded_handle(&self.file)
    }
}

#[cfg(unix)]
fn mark_not_inheritable(file: &File) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    if result == -1 {
        Err(format!(
            "mark sentinel close-on-exec: {}",
            io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn encoded_handle(file: &File) -> String {
    use std::os::fd::AsRawFd;

    file.as_raw_fd().to_string()
}

#[cfg(unix)]
fn verify_sentinel_closed(encoded: &str) -> Result<(), String> {
    let descriptor: libc::c_int = encoded
        .parse()
        .map_err(|error| format!("parse sentinel descriptor: {error}"))?;
    let result = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::EBADF) {
        Ok(())
    } else {
        Err("owned sentinel descriptor was inherited by worker".into())
    }
}

#[cfg(windows)]
fn mark_not_inheritable(file: &File) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

    let handle = file.as_raw_handle() as HANDLE;
    let result = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
    if result == 0 {
        Err(format!(
            "clear sentinel handle inheritance: {}",
            io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn encoded_handle(file: &File) -> String {
    use std::os::windows::io::AsRawHandle;

    (file.as_raw_handle() as usize).to_string()
}

#[cfg(windows)]
fn verify_sentinel_closed(encoded: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE};

    let handle: usize = encoded
        .parse()
        .map_err(|error| format!("parse sentinel handle: {error}"))?;
    let mut flags = 0_u32;
    let result = unsafe { GetHandleInformation(handle as HANDLE, &mut flags) };
    if result == 0 {
        Ok(())
    } else {
        Err("owned sentinel handle was inherited by worker".into())
    }
}
