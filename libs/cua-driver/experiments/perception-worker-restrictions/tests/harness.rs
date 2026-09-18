use std::process::Command;

#[test]
fn supervisor_restrictions_hold_for_synthetic_workers() {
    let output = Command::new(env!("CARGO_BIN_EXE_cua-perception-worker-restrictions"))
        .arg("self-test")
        .output()
        .expect("run restriction self-test");

    assert!(
        output.status.success(),
        "self-test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("self-test output is UTF-8");
    assert!(stdout.contains("probe: enforced"));
    assert!(stdout.contains("request: oversized payload rejected before spawn"));
    assert!(stdout.contains("deadline: enforced, child reaped"));
    assert!(stdout.contains(
        "blocked-stdin deadline: synchronized never-read request remained pending; I/O stopped, direct child reaped"
    ));
    #[cfg(unix)]
    assert!(stdout.contains(
        "descendant stdio: Unix process group terminated; I/O stopped, direct child reaped"
    ));
    #[cfg(unix)]
    assert!(stdout.contains(
        "exited group leader: identity preserved until group termination; I/O stopped, direct child reaped"
    ));
    #[cfg(unix)]
    assert!(stdout.contains(
        "closed stdout: non-reaping liveness kept supervision active through deadline; direct child reaped"
    ));
    assert!(stdout.contains("output: enforced, direct child reaped"));
    assert!(stdout.contains("memory:"));
}
