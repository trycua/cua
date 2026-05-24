//! Parity check for the `mcp-config` CLI subcommand.

use std::process::Command;

#[cfg(target_os = "windows")]
fn main() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .parent().unwrap()
        .join("target/debug/cua-driver.exe");

    let cases = [
        // Each (args, expected substrings)
        (vec![], vec!["\"mcpServers\"", "\"cua-driver\"", "\"command\":", "\"args\": [\"mcp\"]"]),
        (vec!["--client", "claude"], vec!["claude mcp add --transport stdio cua-driver"]),
        (vec!["--client", "codex"],  vec!["codex mcp add cua-driver"]),
        (vec!["--client", "cursor"], vec!["\"type\": \"stdio\"", "\"mcpServers\""]),
        (vec!["--client", "openclaw"], vec!["openclaw mcp set cua-driver"]),
        (vec!["--client", "opencode"], vec!["opencode.ai/config.json", "\"type\": \"local\""]),
        (vec!["--client", "hermes"], vec!["mcp_servers:", "cua-driver:"]),
        (vec!["--client", "pi"],     vec!["does not support MCP natively"]),
    ];

    for (args, needles) in cases {
        let mut cmd_args = vec!["mcp-config"];
        cmd_args.extend(args.iter().copied());
        let out = Command::new(&exe).args(&cmd_args).output().expect("run");
        assert!(out.status.success(), "mcp-config {args:?} failed: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        for needle in &needles {
            assert!(stdout.contains(needle),
                "mcp-config {args:?} output missing {needle:?}\n--- stdout ---\n{stdout}");
        }
        println!("mcp-config {args:?} OK");
    }

    // Unknown client → exit 2, stderr names valid clients.
    let out = Command::new(&exe).args(["mcp-config", "--client", "bogus"]).output().expect("run");
    assert_eq!(out.status.code(), Some(2), "unknown client should exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Unknown client 'bogus'") && stderr.contains("claude"),
        "unknown-client stderr wrong: {stderr:?}");
    println!("unknown-client err OK");

    println!("\n✅ PASS: mcp-config 8 client paths + unknown-client error verified");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
