//! Hermetic Windows lifecycle contract: a disposable process speaks fake QMP.
//! It never runs QEMU, boots a VM, injects input, or touches the user's cua home.
//! A harness=false target lets this same test executable serve as fake QEMU.

#[cfg(not(windows))]
fn main() {
    println!("qemu_windows_lifecycle: Windows-only fixture suite skipped");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::entry() {
        eprintln!("Windows lifecycle fixture failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use std::collections::BTreeMap;
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use cua_vmm::host::windows_process::{Identity, Process};
    use cua_vmm::qemu::{EntryKind, Firmware, QemuConfig, QemuRuntime, QemuState};
    use cua_vmm::{Arch, GuestOs, ImageSource, Runtime, StartSpec, Status, VmmError};
    use serde_json::{Value, json};
    use windows_sys::Win32::Foundation::{FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    pub fn entry() -> TestResult {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.iter().any(|arg| arg == "-qmp") {
            return fake_qemu(&args);
        }
        if args.first().is_some_and(|arg| arg == "--idle") {
            let mut data = Vec::new();
            std::io::stdin().read_to_end(&mut data)?;
            return Ok(());
        }
        if args.first().is_some_and(|arg| arg == "--lock") {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&args[1])?;
            file.try_lock()?;
            println!("locked");
            std::io::stdout().flush()?;
            let mut data = Vec::new();
            std::io::stdin().read_to_end(&mut data)?;
            return Ok(());
        }
        if args.first().is_some_and(|arg| arg == "--suite") {
            return tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(suite(Path::new(&args[1])));
        }

        // Set the isolated process environment through Command, never a shared
        // runtime's env. The only discoverable qemu-system binary is this fixture.
        let temp = tempfile::tempdir()?;
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin)?;
        let executable = std::env::current_exe()?;
        std::fs::copy(
            &executable,
            bin.join(format!("qemu-system-{}.exe", Arch::host().qemu())),
        )?;
        let mut paths = vec![bin];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        let result = Command::new(&executable)
            .arg("--suite")
            .arg(temp.path())
            .env("PATH", std::env::join_paths(paths)?)
            .env("CUA_HOME", temp.path().join("home"))
            .current_dir(temp.path())
            .status()?;
        assert!(result.success(), "isolated Windows fixture suite failed");
        Ok(())
    }

    fn runtime(root: &Path, graceful: Duration) -> QemuRuntime {
        QemuRuntime::new(QemuConfig {
            root: root.into(),
            graceful_stop: graceful,
            vnc: false,
            ..Default::default()
        })
    }

    fn state(root: &Path, name: &str, mode: &str) -> TestResult<QemuState> {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir)?;
        let disk = dir.join("disk.qcow2");
        let seed = dir.join("seed.iso");
        std::fs::write(&disk, b"disposable fixture disk")?;
        std::fs::write(&seed, b"preserve this fixture seed")?;
        let ports = cua_vmm::host::free_ports(2)?;
        let st = QemuState {
            name: name.into(),
            kind: EntryKind::Instance,
            arch: Arch::host(),
            os: GuestOs::Windows,
            disk,
            disk_format: "qcow2".into(),
            install_iso: None,
            seed_iso: Some(seed),
            firmware: Firmware::Bios,
            cpus: 4,
            memory_mb: 6144,
            ports: BTreeMap::from([(3211, ports[0])]),
            qmp_port: Some(ports[1]),
            vnc_display: None,
            pid: None,
            process_identity: None,
            launch_pending: false,
            accel: "tcg".into(),
            ssh: None,
            restrict_network: true,
            extra_args: vec!["--fixture-mode".into(), mode.into()],
            snapshots: vec![],
            paused: false,
            source: Some("fixture".into()),
            created_at: 1,
            gpu: None,
        };
        save(root, &st)?;
        Ok(st)
    }

    fn save(root: &Path, st: &QemuState) -> TestResult {
        std::fs::write(
            root.join(&st.name).join("state.json"),
            serde_json::to_vec(st)?,
        )?;
        Ok(())
    }

    fn spec(st: &QemuState) -> StartSpec {
        let mut spec = StartSpec::new(&st.name, ImageSource::Existing)
            .os(st.os)
            .arch(st.arch)
            .cpus(st.cpus)
            .memory_mb(st.memory_mb);
        spec.ports = st.ports.keys().copied().collect();
        spec.restrict_network = st.restrict_network;
        spec
    }

    fn events(dir: &Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("fixture-events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn event(dir: &Path, value: Value) -> TestResult {
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("fixture-events.jsonl"))?,
            "{value}"
        )?;
        Ok(())
    }

    struct ChildFixture(Child);
    impl Drop for ChildFixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // Cleanup controls only the fixture executable/creation time it recorded.
    // The force fallback below is test-only and never a production VM path.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Err(error) = cleanup_fixture(&self.0) {
                eprintln!("fixture cleanup: {error}");
            }
        }
    }

    fn cleanup_fixture(dir: &Path) -> TestResult {
        let Some(started) = events(dir)
            .into_iter()
            .rev()
            .find(|event| event["event"] == "started")
        else {
            return Ok(());
        };
        let pid = started["pid"].as_u64().unwrap() as u32;
        let identity: Identity = serde_json::from_value(started["identity"].clone())?;
        let Some(process) = Process::open(pid)? else {
            return Ok(());
        };
        if process.state()? == cua_vmm::host::ProcessState::Exited {
            return Ok(());
        }
        process.verify(&identity)?;
        // SAFETY: request a separate owned handle, then verify its creation time
        // before using it to stop this disposable fixture, even across PID reuse.
        let raw = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: successful OpenProcess transfers one valid owned handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: all output buffers are live and the owned handle stays valid.
        if unsafe {
            GetProcessTimes(
                handle.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        assert_eq!(
            (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
            identity.creation_time
        );
        let st: QemuState = serde_json::from_slice(&std::fs::read(dir.join("state.json"))?)?;
        if let Some(port) = st.qmp_port
            && let Ok(mut stream) = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        {
            stream.set_read_timeout(Some(Duration::from_secs(1)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut greeting = String::new();
            reader.read_line(&mut greeting)?;
            sync_command(&mut stream, &mut reader, "qmp_capabilities")?;
            let _ = sync_command(&mut stream, &mut reader, "fixture-cleanup");
        }
        // SAFETY: this handle's creation time matched the fixture and is held
        // throughout cleanup. Its kernel object cannot be redirected by PID reuse.
        let mut result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 2000) };
        if result == WAIT_TIMEOUT {
            // Only this test fixture; never name-based or an unverified PID kill.
            if unsafe { TerminateProcess(handle.as_raw_handle(), 1) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            result = unsafe { WaitForSingleObject(handle.as_raw_handle(), 2000) };
        }
        if result != WAIT_OBJECT_0 {
            return Err(format!("fixture {pid} did not confirm exit (wait={result})").into());
        }
        Ok(())
    }

    fn sync_command(
        stream: &mut TcpStream,
        reader: &mut BufReader<TcpStream>,
        command: &str,
    ) -> TestResult<Value> {
        writeln!(stream, "{}", json!({"execute": command}))?;
        stream.flush()?;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }

    async fn suite(temp: &Path) -> TestResult {
        let root = temp.join("vmm");
        let rt = runtime(&root, Duration::from_secs(3));
        let st = state(&root, "cold", "normal")?;
        let _cleanup = Cleanup(root.join("cold"));
        let first = rt.resume("cold").await?;
        assert_eq!(first.status, Status::Running);
        assert_eq!(rt.status("cold").await?, Status::Running);
        let running = rt.load("cold")?;
        assert!(running.process_identity.is_some());
        assert_eq!(
            (
                running.cpus,
                running.memory_mb,
                running.ports.clone(),
                running.qmp_port
            ),
            (4, 6144, st.ports.clone(), st.qmp_port)
        );
        assert_eq!(
            std::fs::read(st.seed_iso.as_ref().unwrap())?,
            b"preserve this fixture seed"
        );
        assert_eq!(std::fs::read(&st.disk)?, b"disposable fixture disk");
        assert_eq!(rt.start(&spec(&st)).await?.endpoints.ports, st.ports);
        assert_eq!(rt.load("cold")?.pid, running.pid);
        assert_eq!(
            events(&root.join("cold"))
                .iter()
                .filter(|event| event["event"] == "started")
                .count(),
            1
        );
        rt.suspend("cold").await?;
        assert_eq!(rt.status("cold").await?, Status::Paused);
        assert_eq!(rt.resume("cold").await?.status, Status::Running);
        assert_eq!(rt.load("cold")?.pid, running.pid);
        rt.stop("cold").await?;
        assert_eq!(rt.status("cold").await?, Status::Stopped);
        assert!(rt.load("cold")?.pid.is_none());
        assert!(rt.endpoints("cold").await?.ports.is_empty());
        assert!(!root.join("cold/qemu.pid").exists());
        rt.resume("cold").await?;
        assert_eq!(rt.load("cold")?.ports, st.ports);
        assert_eq!(
            events(&root.join("cold"))
                .iter()
                .filter(|event| event["event"] == "started")
                .count(),
            2
        );
        rt.stop("cold").await?;
        println!(
            "PASS cold resume, resource/seed/disk preservation, repeat start, stop and restart"
        );

        let paused = state(&root, "paused-stop", "normal")?;
        let _cleanup = Cleanup(root.join("paused-stop"));
        rt.resume(&paused.name).await?;
        rt.suspend(&paused.name).await?;
        rt.stop(&paused.name).await?;
        assert_eq!(rt.status(&paused.name).await?, Status::Stopped);
        assert!(!rt.load(&paused.name)?.paused);
        let commands = events(&root.join("paused-stop"));
        let resume = commands
            .iter()
            .position(|e| e["command"] == "cont")
            .unwrap();
        let shutdown = commands
            .iter()
            .position(|e| e["command"] == "system_powerdown")
            .unwrap();
        assert!(resume < shutdown);
        assert!(!commands.iter().any(|e| e["command"] == "quit"));

        let paused_timeout = state(&root, "paused-timeout", "ignore-stop")?;
        let _cleanup = Cleanup(root.join("paused-timeout"));
        rt.resume(&paused_timeout.name).await?;
        rt.suspend(&paused_timeout.name).await?;
        let original_pid = rt.load(&paused_timeout.name)?.pid;
        let quick = runtime(&root, Duration::from_millis(100));
        assert!(matches!(
            quick.stop(&paused_timeout.name).await,
            Err(VmmError::Timeout { .. })
        ));
        let retained = rt.load(&paused_timeout.name)?;
        assert_eq!(retained.pid, original_pid);
        assert!(!retained.paused);
        assert_eq!(rt.status(&paused_timeout.name).await?, Status::Running);
        assert!(retained.process_identity.is_some());
        assert!(
            !events(&root.join("paused-timeout"))
                .iter()
                .any(|e| e["command"] == "quit")
        );
        println!("PASS paused graceful shutdown and truthful state after shutdown timeout");

        let mut legacy = state(&root, "legacy", "normal")?;
        let mut child = ChildFixture(
            Command::new(std::env::current_exe()?)
                .arg("--idle")
                .stdin(Stdio::piped())
                .spawn()?,
        );
        legacy.pid = Some(child.0.id());
        save(&root, &legacy)?;
        assert!(matches!(
            rt.inspect_status("legacy"),
            Err(VmmError::ProcessIdentity { .. })
        ));
        assert!(matches!(
            rt.start(&spec(&legacy)).await,
            Err(VmmError::ProcessIdentity { .. })
        ));
        assert!(matches!(
            rt.stop("legacy").await,
            Err(VmmError::ProcessIdentity { .. })
        ));
        let mut identity = Process::open(child.0.id())?.unwrap().identity()?;
        identity.creation_time ^= 1;
        legacy.process_identity = Some(identity);
        save(&root, &legacy)?;
        assert!(matches!(
            rt.stop("legacy").await,
            Err(VmmError::ProcessIdentity { .. })
        ));
        legacy.pid = None;
        legacy.process_identity = None;
        save(&root, &legacy)?;
        std::fs::write(root.join("legacy/qemu.pid"), child.0.id().to_string())?;
        assert!(matches!(
            rt.inspect_status("legacy"),
            Err(VmmError::ProcessIdentity { .. })
        ));
        assert!(matches!(
            rt.resume("legacy").await,
            Err(VmmError::ProcessIdentity { .. })
        ));
        assert!(child.0.try_wait()?.is_none());
        drop(child.0.stdin.take());
        child.0.wait()?;
        println!("PASS live legacy PID/pidfile and reused identity are retained/refused");

        let interrupted = state(&root, "interrupted", "normal")?;
        let mut uncertain = interrupted.clone();
        uncertain.launch_pending = true;
        save(&root, &uncertain)?;
        assert!(matches!(
            rt.resume("interrupted").await,
            Err(VmmError::State { .. })
        ));
        assert!(rt.load("interrupted")?.launch_pending);
        let occupied = state(&root, "occupied", "normal")?;
        let _listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, occupied.ports[&3211]))?;
        let error = rt.resume("occupied").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unavailable; no process launched")
        );
        assert!(rt.load("occupied")?.pid.is_none());
        assert!(events(&root.join("occupied")).is_empty());
        println!("PASS interrupted spawn and occupied fixed forward cannot launch another process");

        let exited = state(&root, "exited", "exit-at-launch")?;
        let _cleanup = Cleanup(root.join("exited"));
        let result = rt.resume(&exited.name).await;
        assert!(
            matches!(result, Err(VmmError::Command { .. })),
            "exit-at-launch must report the failed command, got {result:?}"
        );
        assert!(rt.load(&exited.name)?.pid.is_none());
        assert!(!rt.load(&exited.name)?.launch_pending);
        assert_eq!(std::fs::read(&exited.disk)?, b"disposable fixture disk");

        for mode in ["wrong-name", "read-only-disk", "wrong-device"] {
            let invalid = state(&root, mode, mode)?;
            let _cleanup = Cleanup(root.join(mode));
            assert!(matches!(
                rt.resume(mode).await,
                Err(VmmError::ProcessIdentity { .. })
            ));
            let pid = rt.load(mode)?.pid;
            assert!(pid.is_some());
            assert!(matches!(
                rt.start(&spec(&invalid)).await,
                Err(VmmError::ProcessIdentity { .. })
            ));
            assert_eq!(rt.load(mode)?.pid, pid);
            assert!(matches!(
                rt.stop(mode).await,
                Err(VmmError::ProcessIdentity { .. })
            ));
            assert_eq!(
                events(&root.join(mode))
                    .iter()
                    .filter(|event| event["event"] == "started")
                    .count(),
                1
            );
            assert!(
                !events(&root.join(mode))
                    .iter()
                    .any(|event| event["command"] == "system_powerdown"
                        || event["command"] == "quit")
            );
        }
        println!(
            "PASS startup failure, live failure retention, QMP identity/read-only disk refusal"
        );

        let refused = state(&root, "refused", "refuse-stop")?;
        let refused_cleanup = Cleanup(root.join("refused"));
        rt.resume(&refused.name).await?;
        let before = std::fs::read(root.join("refused/state.json"))?;
        assert!(
            matches!(rt.stop("refused").await, Err(VmmError::Qmp(message)) if message == "GenericError: fixture refused shutdown")
        );
        assert_eq!(std::fs::read(root.join("refused/state.json"))?, before);
        assert_eq!(rt.status("refused").await?, Status::Running);
        let ignored = state(&root, "ignored", "ignore-stop")?;
        let ignored_cleanup = Cleanup(root.join("ignored"));
        let bounded = runtime(&root, Duration::from_millis(50));
        bounded.resume(&ignored.name).await?;
        let before = std::fs::read(root.join("ignored/state.json"))?;
        assert!(matches!(
            bounded.stop("ignored").await,
            Err(VmmError::Timeout { .. })
        ));
        assert_eq!(std::fs::read(root.join("ignored/state.json"))?, before);
        assert!(
            !events(&root.join("ignored"))
                .iter()
                .any(|event| event["command"] == "quit")
        );
        let explicit_force = runtime(&root, Duration::ZERO);
        assert!(matches!(
            explicit_force.stop("ignored").await,
            Err(VmmError::Timeout { .. })
        ));
        assert_eq!(std::fs::read(root.join("ignored/state.json"))?, before);
        cleanup_fixture(&root.join("refused"))?;
        cleanup_fixture(&root.join("ignored"))?;
        drop(refused_cleanup);
        drop(ignored_cleanup);
        println!(
            "PASS shutdown refusal, timeout without force, failed explicit fixture quit without false success"
        );

        let locked = state(&root, "locked", "normal")?;
        let mut locker = ChildFixture(
            Command::new(std::env::current_exe()?)
                .arg("--lock")
                .arg(root.join(".lifecycle.lock"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        let mut barrier = String::new();
        BufReader::new(locker.0.stdout.take().unwrap()).read_line(&mut barrier)?;
        assert_eq!(barrier.trim(), "locked");
        assert!(
            rt.resume(&locked.name)
                .await
                .unwrap_err()
                .to_string()
                .contains("another QEMU lifecycle operation")
        );
        assert!(
            rt.inspect_status(&locked.name)
                .unwrap_err()
                .to_string()
                .contains("another QEMU lifecycle operation")
        );
        assert!(events(&root.join("locked")).is_empty());
        drop(locker.0.stdin.take());
        assert!(locker.0.wait()?.success());
        let _cleanup = Cleanup(root.join("locked"));
        let other = runtime(&root, Duration::from_secs(3));
        let spec = spec(&locked);
        let (a, b) = tokio::join!(rt.start(&spec), other.start(&spec));
        assert!(a.is_ok() || b.is_ok());
        for result in [a, b] {
            if let Err(error) = result {
                assert!(
                    error
                        .to_string()
                        .contains("another QEMU lifecycle operation")
                );
            }
        }
        assert_eq!(
            events(&root.join("locked"))
                .iter()
                .filter(|event| event["event"] == "started")
                .count(),
            1
        );
        rt.stop("locked").await?;
        println!("PASS interprocess and separate-runtime launch serialization");

        let spawn_failed = state(&root, "spawn-failed", "normal")?;
        let executable =
            cua_vmm::host::which(&format!("qemu-system-{}", Arch::host().qemu())).unwrap();
        let exclusive = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(executable)?;
        assert!(matches!(
            rt.resume(&spawn_failed.name).await,
            Err(VmmError::Io(_))
        ));
        assert!(rt.load(&spawn_failed.name)?.pid.is_none());
        assert!(!rt.load(&spawn_failed.name)?.launch_pending);
        assert!(events(&root.join("spawn-failed")).is_empty());
        drop(exclusive);
        let _cleanup = Cleanup(root.join("spawn-failed"));
        rt.resume(&spawn_failed.name).await?;
        rt.stop(&spawn_failed.name).await?;
        println!("PASS failed process creation leaves a proven stopped state");

        let log_failed = state(&root, "log-failed", "normal")?;
        std::fs::create_dir(root.join("log-failed/qemu.log"))?;
        assert!(matches!(
            rt.resume(&log_failed.name).await,
            Err(VmmError::Io(_))
        ));
        assert!(rt.load(&log_failed.name)?.pid.is_none());
        assert!(!rt.load(&log_failed.name)?.launch_pending);
        assert!(events(&root.join("log-failed")).is_empty());
        std::fs::remove_dir(root.join("log-failed/qemu.log"))?;
        let _cleanup = Cleanup(root.join("log-failed"));
        rt.resume(&log_failed.name).await?;
        rt.stop(&log_failed.name).await?;
        println!("PASS failed log preparation creates no unknown launch and remains retryable");
        Ok(())
    }

    fn option(args: &[String], flag: &str) -> TestResult<String> {
        Ok(args
            .windows(2)
            .find(|pair| pair[0] == flag)
            .ok_or_else(|| format!("missing fixture option {flag}"))?[1]
            .clone())
    }

    fn fake_qemu(args: &[String]) -> TestResult {
        let mode = option(args, "--fixture-mode")?;
        let name = option(args, "-name")?;
        let pidfile = PathBuf::from(option(args, "-pidfile")?);
        let dir = pidfile.parent().unwrap();
        let disk_arg = args
            .windows(2)
            .find(|pair| pair[0] == "-drive" && pair[1].contains("id=disk0"))
            .ok_or("fixture disk0 absent")?[1]
            .clone();
        let disk = PathBuf::from(
            disk_arg
                .strip_prefix("file=")
                .unwrap()
                .split(",if=none,id=disk0,")
                .next()
                .unwrap(),
        );
        let _disk_owner = OpenOptions::new().read(true).write(true).open(&disk)?;
        let identity = Process::open(std::process::id())?.unwrap().identity()?;
        event(
            dir,
            json!({"event": "started", "pid": std::process::id(), "identity": identity}),
        )?;
        std::fs::write(&pidfile, std::process::id().to_string())?;
        let addr = option(args, "-qmp")?
            .trim_start_matches("tcp:")
            .split(',')
            .next()
            .unwrap()
            .to_owned();
        let listener = TcpListener::bind(addr)?;
        let network = option(args, "-netdev")?;
        let mut forwards = Vec::new();
        for forward in network
            .split(',')
            .filter_map(|part| part.strip_prefix("hostfwd=tcp:127.0.0.1:"))
        {
            let port: u16 = forward.split("-:").next().unwrap().parse()?;
            forwards.push(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?);
        }
        let mut paused = false;
        for stream in listener.incoming() {
            // Exit only once launch has persisted identity and connected. This
            // tests a real early process exit without a timing-dependent race
            // between CreateProcess and the process identity API.
            if mode == "exit-at-launch" {
                std::process::exit(42)
            }
            let mut stream = stream?;
            let mut reader = BufReader::new(stream.try_clone()?);
            writeln!(
                stream,
                "{}",
                json!({"QMP": {"version": {}, "capabilities": []}})
            )?;
            stream.flush()?;
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                let value: Value = serde_json::from_str(&line)?;
                let command = value["execute"].as_str().unwrap();
                event(dir, json!({"command": command}))?;
                let response = match command {
                    "qmp_capabilities" => json!({"return": {}}),
                    "query-name" => {
                        json!({"return": {"name": if mode == "wrong-name" { "unowned" } else { &name }}})
                    }
                    "query-block" => {
                        json!({"return": [{"device": if mode == "wrong-device" { "cd0" } else { "disk0" }, "inserted": {"ro": mode == "read-only-disk", "image": {"filename": disk}}}]})
                    }
                    "query-status" => {
                        json!({"return": {"status": if paused { "paused" } else { "running" }}})
                    }
                    "stop" => {
                        paused = true;
                        json!({"return": {}})
                    }
                    "cont" => {
                        paused = false;
                        json!({"return": {}})
                    }
                    "system_powerdown" if mode == "refuse-stop" => {
                        json!({"error": {"class": "GenericError", "desc": "fixture refused shutdown"}})
                    }
                    // ACPI cannot be handled by a guest whose vCPUs are paused.
                    "system_powerdown" if paused => json!({"return": {}}),
                    "system_powerdown" | "quit" if mode != "ignore-stop" => {
                        writeln!(stream, "{}", json!({"return": {}}))?;
                        stream.flush()?;
                        std::process::exit(0);
                    }
                    "system_powerdown" | "quit" => json!({"return": {}}),
                    "fixture-cleanup" => std::process::exit(0),
                    _ => {
                        json!({"error": {"class": "CommandNotFound", "desc": "fixture command absent"}})
                    }
                };
                writeln!(stream, "{response}")?;
                stream.flush()?;
                line.clear();
            }
        }
        Ok(())
    }
}
