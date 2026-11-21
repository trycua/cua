// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use super::*;

#[test]
fn guest_manifest_handles_native_tools_spaces_and_credential_exclusions() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a user's home with spaces");
    let files = [
        ("%z", b"probe".as_slice()),
        ("hermes/memories/hello world.md", b"remember me".as_slice()),
        ("work/a file.txt", b"abc".as_slice()),
        ("hermes/state.db", b"durable state".as_slice()),
        ("hermes/.env", b"secret".as_slice()),
        ("hermes/auth.json", b"secret".as_slice()),
        ("hermes/.credentials.json", b"secret".as_slice()),
        ("hermes/credentials/provider", b"secret".as_slice()),
        ("hermes/vault/token", b"secret".as_slice()),
        ("hermes/cache/noise", b"cache".as_slice()),
    ];
    for (rel, bytes) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    check_manifest(&dir, &mut std::process::Command::new("sh"));
    // Exercise the real macOS fallback even on machines with sha256sum
    // elsewhere on PATH. This is only a test-local directory of native tools.
    #[cfg(target_os = "macos")]
    {
        let bin = root.path().join("native-tools");
        std::fs::create_dir(&bin).unwrap();
        for tool in ["find", "stat", "shasum"] {
            std::os::unix::fs::symlink(format!("/usr/bin/{tool}"), bin.join(tool)).unwrap();
        }
        let mut command = std::process::Command::new("/bin/sh");
        command.env("PATH", bin);
        check_manifest(&dir, &mut command);
    }
}

fn check_manifest(dir: &std::path::Path, command: &mut std::process::Command) {
    let output = command
        .args(["-c", &manifest_script(dir.to_str().unwrap())])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = parse_manifest(&output.stdout);
    assert_eq!(
        manifest.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "%z",
            "hermes/memories/hello world.md",
            "hermes/state.db",
            "work/a file.txt",
        ]
    );
    assert_eq!(
        manifest["work/a file.txt"],
        FileState {
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        }
    );
    assert_eq!(manifest["hermes/memories/hello world.md"].size, 11);
    assert_eq!(manifest["hermes/state.db"].size, 13);
}

#[test]
fn empty_or_absent_home_has_an_empty_manifest_without_reading_stdin() {
    let root = tempfile::tempdir().unwrap();
    for dir in [root.path().to_path_buf(), root.path().join("missing home")] {
        let output = std::process::Command::new("sh")
            .args(["-c", &manifest_script(dir.to_str().unwrap())])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(parse_manifest(&output.stdout).is_empty());
    }
}
