use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(
        args.len(),
        4,
        "expected executable, output path, and close|hold"
    );
    let output = PathBuf::from(&args[2]);
    let hold = args[3] == "hold";
    assert!(hold || args[3] == "close");
    let start = Instant::now();
    let mut child = Command::new(&args[1])
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "gdigrab",
            "-framerate",
            "30",
            "-draw_mouse",
            "1",
            "-i",
            "desktop",
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-g",
            "30",
        ])
        .arg(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    println!("child_pid={}", child.id());
    let mut stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read_result = stderr.read_to_end(&mut bytes);
        let _ = tx.send((bytes, read_result));
    });
    let probe_deadline = Instant::now() + Duration::from_millis(1500);
    let mut early_exit = None;
    loop {
        if let Some(status) = child.try_wait()? {
            early_exit = Some(status);
            break;
        }
        if Instant::now() >= probe_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("startup_exit={early_exit:?}");
    if early_exit.is_none() {
        std::thread::sleep(Duration::from_secs(3));
        let stop_started = Instant::now();
        let mut stdin = child.stdin.take().ok_or("missing stdin pipe")?;
        println!("stdin_write={:?}", stdin.write_all(b"q\n"));
        println!("stdin_flush={:?}", stdin.flush());
        let held = if hold {
            Some(stdin)
        } else {
            drop(stdin);
            None
        };
        println!("stdin_send_ms={}", stop_started.elapsed().as_millis());
        let deadline = Instant::now() + Duration::from_millis(3000);
        let (status, forced_kill) = loop {
            if let Some(status) = child.try_wait()? {
                break (status, false);
            }
            if Instant::now() > deadline {
                println!("kill_result={:?}", child.kill());
                break (child.wait()?, true);
            }
            std::thread::sleep(Duration::from_millis(80));
        };
        println!("shutdown_ms={}", stop_started.elapsed().as_millis());
        println!("forced_kill={forced_kill}");
        println!("exit_status={status:?}");
        println!("exit_code={:?}", status.code());
        println!("finalized={}", !forced_kill && status.success());
        drop(held);
    }
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok((bytes, result)) => {
            println!("stderr_read={result:?}");
            std::fs::write(output.with_extension("stderr.txt"), bytes)?;
        }
        Err(error) => println!("stderr_reader_incomplete={error}"),
    }
    println!("total_ms={}", start.elapsed().as_millis());
    println!(
        "output_bytes={:?}",
        std::fs::metadata(&output).map(|m| m.len())
    );
    Ok(())
}
