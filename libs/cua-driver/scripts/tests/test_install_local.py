from __future__ import annotations

import fcntl
import os
import pty
import select
import shutil
import subprocess
import sys
import termios
import time
from pathlib import Path

import pytest

INSTALL_LOCAL = Path(__file__).resolve().parents[1] / "_install-local-rust.sh"
LOCAL_SIGNING = INSTALL_LOCAL.with_name("_local-signing.sh")
DISPATCHER = INSTALL_LOCAL.with_name("install-local.sh")
WINDOWS_INSTALL_LOCAL = INSTALL_LOCAL.with_name("install-local.ps1")
SKILL_PACK = INSTALL_LOCAL.parents[1] / "rust/Skills/cua-driver"


def test_local_installers_stage_the_canonical_skill_pack() -> None:
    windows = WINDOWS_INSTALL_LOCAL.read_text(encoding="utf-8")

    assert 'Join-Path $RepoRoot "Skills\\cua-driver"' in windows
    assert 'Join-Path $VersionedDir "Skills\\cua-driver"' in windows
    assert "Skills\\cua-driver-rs" not in windows
    assert "Skills/cua-driver-rs" not in INSTALL_LOCAL.read_text(encoding="utf-8")
    assert {path.name for path in SKILL_PACK.iterdir()} >= {
        "SKILL.md",
        "BROWSER.md",
        "MACOS.md",
        "WINDOWS.md",
        "LINUX.md",
    }


def _write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    path.chmod(0o755)


def test_explicit_local_signing_identity_is_selected_exactly(tmp_path: Path) -> None:
    keychain = tmp_path / "signing.keychain-db"
    keychain.touch()
    fake_bin = tmp_path / "fake-bin"
    _write_executable(fake_bin / "codesign", "exit 0\n")
    wanted = "F2D26B5AFAAB910B340FBD8F480F88DF748D9D48"
    other = "A" * 40
    _write_executable(
        fake_bin / "security",
        f"printf '%s\\n' '  1) {wanted} \"Developer ID Application: Example\"' "
        f"'  2) {other} \"Developer ID Application: Renewal\"'\n",
    )
    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN": str(keychain),
            "CUA_DRIVER_LOCAL_SIGNING_IDENTITY": wanted.lower(),
        }
    )
    result = subprocess.run(
        [
            "/bin/bash",
            "-c",
            f'OS=Darwin; . "{LOCAL_SIGNING}"; ensure_local_signing_identity',
        ],
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == wanted


def test_explicit_local_signing_identity_never_falls_back(tmp_path: Path) -> None:
    keychain = tmp_path / "signing.keychain-db"
    keychain.touch()
    fake_bin = tmp_path / "fake-bin"
    _write_executable(fake_bin / "codesign", "exit 0\n")
    _write_executable(
        fake_bin / "security",
        f"printf '%s\\n' '  1) {'A' * 40} \"Developer ID Application: Other\"'\n",
    )
    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN": str(keychain),
            "CUA_DRIVER_LOCAL_SIGNING_IDENTITY": "B" * 40,
        }
    )
    result = subprocess.run(
        [
            "/bin/bash",
            "-c",
            f'OS=Darwin; . "{LOCAL_SIGNING}"; ensure_local_signing_identity',
        ],
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == "-"


def _classify(requirement: str) -> str:
    result = subprocess.run(
        [
            "/bin/bash",
            "-c",
            f'. "{LOCAL_SIGNING}"; classify_designated_requirement "$1"',
            "bash",
            requirement,
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout


def test_stable_signing_accepts_every_certificate_pin_spelling() -> None:
    # An untrusted local signing certificate is pinned as the leaf.
    assert (
        _classify(
            'identifier "com.trycua.driver.local" and certificate leaf = '
            'H"71b7d45889593c1b77459fcb981d9460cc929431"'
        )
        == "certificate-backed"
    )
    # The same certificate, once trusted in the keychain, evaluates as its own
    # anchor and codesign pins it as the root. Equally stable across rebuilds.
    assert (
        _classify(
            'identifier "com.trycua.driver.local" and certificate root = '
            'H"662061ed4588ed82dd4c0999adaf5e8014abd46f"'
        )
        == "certificate-backed"
    )
    # An Apple-issued identity pins its team instead of a certificate hash.
    assert (
        _classify(
            'anchor apple generic and identifier "com.trycua.driver.local" and '
            "certificate leaf[subject.OU] = YCK386LBJ7"
        )
        == "certificate-backed"
    )
    # A cdhash pin is the rebuild-fragile case --require-stable-signing refuses,
    # and stays rebuild-fragile when the requirement also pins a certificate.
    assert _classify('cdhash H"1234"') == "ad-hoc"
    assert (
        _classify(
            'cdhash H"71b7d45889593c1b77459fcb981d9460cc929431" and certificate '
            'root = H"662061ed4588ed82dd4c0999adaf5e8014abd46f"'
        )
        == "ad-hoc"
    )
    # Certificate text that pins nothing must not read as stable.
    assert (
        _classify('identifier "com.trycua.driver.local" and certificate root trusted')
        == "unknown"
    )


GENERATED_IDENTITY = "71B7D45889593C1B77459FCB981D9460CC929431"
GENERATED_LABEL = "CuaDriver Local Signing (cua-driver-rs)"
REAL_KEYCHAIN_GATE = "CUA_DRIVER_LOCAL_SIGNING_REAL_KEYCHAIN_TEST"


def _generated_identity_env(
    tmp_path: Path, authorize_exit: int = 0, authorize_password: str | None = None
) -> tuple[dict[str, str], Path]:
    """Env whose fake `security` forces the certificate-creation branch.

    Calls are logged one pipe-delimited argument list per line, so a test can
    assert the exact argv the script passes to `security`. With
    `authorize_password`, the stub imitates a host that refuses a passwordless
    `set-key-partition-list` and accepts `-k <password>`.
    """
    keychain = tmp_path / "signing.keychain-db"
    keychain.touch()
    log = tmp_path / "security-calls.log"
    fake_bin = tmp_path / "fake-bin"
    _write_executable(fake_bin / "codesign", "exit 0\n")
    if authorize_password is None:
        authorize = f"exit {authorize_exit}"
    else:
        authorize = (
            "for argument in \"$@\"; do\n"
            f'      [ "$argument" = "{authorize_password}" ] && exit 0\n'
            "    done\n"
            "    exit 1"
        )
    _write_executable(
        fake_bin / "security",
        f'printf "%s|" "$@" >> "{log}"; printf "\\n" >> "{log}"\n'
        'case "$1" in\n'
        "  find-identity)\n"
        # Empty until the certificate exists, then the created identity.
        f'    [ -f "{log}.imported" ] && printf "%s\\n" '
        f"'  1) {GENERATED_IDENTITY} \"{GENERATED_LABEL}\"'\n"
        "    ;;\n"
        f'  import) : > "{log}.imported" ;;\n'
        f"  set-key-partition-list) {authorize} ;;\n"
        "esac\n"
        "exit 0\n",
    )
    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN": str(keychain),
        }
    )
    env.pop("CUA_DRIVER_LOCAL_SIGNING_IDENTITY", None)
    return env, log


def _ensure_identity(env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            "/bin/bash",
            "-c",
            f'OS=Darwin; . "{LOCAL_SIGNING}"; ensure_local_signing_identity',
        ],
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


def _authorize_calls(log: Path) -> list[list[str]]:
    return [
        line.rstrip("|").split("|")
        for line in log.read_text(encoding="utf-8").splitlines()
        if line.startswith("set-key-partition-list|")
    ]


def test_generated_signing_key_is_authorized_by_exact_label(tmp_path: Path) -> None:
    # `security import -A -T /usr/bin/codesign` leaves the key's partition list
    # closed, so the first codesign prompts for a keychain password or fails
    # with errSecInternalComponent. Opening it must name the key this script
    # imported: `-s` alone matches every signing key in the keychain, which on a
    # login keychain means unrelated identities.
    env, log = _generated_identity_env(tmp_path)

    result = _ensure_identity(env)

    assert result.returncode == 0, result.stderr
    assert result.stdout == GENERATED_IDENTITY
    # The argv is exact: scoped to the imported private key by label, and with
    # no password in it, because an unlocked keychain does not need one.
    assert _authorize_calls(log) == [
        [
            "set-key-partition-list",
            "-S",
            "apple-tool:,apple:,codesign:",
            "-l",
            GENERATED_LABEL,
            "-t",
            "private",
            "-s",
            env["CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN"],
        ]
    ]


def test_existing_generated_key_is_authorized_on_a_later_run(tmp_path: Path) -> None:
    # Authorizing only in the creation branch makes the printed retry
    # instruction a no-op: the second run finds the identity and returns it
    # before any partition-list call.
    env, log = _generated_identity_env(tmp_path)

    first = _ensure_identity(env)
    second = _ensure_identity(env)

    assert first.stdout == GENERATED_IDENTITY
    assert second.stdout == GENERATED_IDENTITY
    imports = [
        line
        for line in log.read_text(encoding="utf-8").splitlines()
        if line.startswith("import|")
    ]
    assert len(imports) == 1
    assert len(_authorize_calls(log)) == 2


def test_unauthorizable_generated_key_says_how_to_authorize(tmp_path: Path) -> None:
    # No terminal to ask a keychain password on: say what to run instead of
    # claiming the key was authorized.
    env, log = _generated_identity_env(tmp_path, authorize_exit=1)

    result = _ensure_identity(env)

    # The identity still goes to stdout; the guidance must not pollute it.
    assert result.stdout == GENERATED_IDENTITY
    assert "set-key-partition-list|" in log.read_text(encoding="utf-8")
    assert "unlock-keychain" in result.stderr
    assert (
        f"-l \"{GENERATED_LABEL}\" -t private -s -k '<keychain-password>'"
        in result.stderr
    )
    assert "errSecInternalComponent" in result.stderr
    assert "is not authorized" in result.stderr


def _become_session_leader() -> None:  # pragma: no cover - runs in the child
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


def _ensure_identity_on_a_terminal(
    env: dict[str, str], prompt: str, typed: str, identity_file: Path
) -> str:
    """Run the identity helper with a controlling terminal and answer its prompt."""
    primary, secondary = pty.openpty()
    transcript = b""
    try:
        process = subprocess.Popen(
            [
                "/bin/bash",
                "-c",
                f'OS=Darwin; . "{LOCAL_SIGNING}"; '
                f'ensure_local_signing_identity > "{identity_file}"',
            ],
            stdin=secondary,
            stdout=secondary,
            stderr=secondary,
            env=env,
            preexec_fn=_become_session_leader,
        )
        os.close(secondary)
        deadline = time.monotonic() + 60
        answered = False
        while time.monotonic() < deadline:
            readable, _, _ = select.select([primary], [], [], 1)
            if readable:
                try:
                    chunk = os.read(primary, 4096)
                except OSError:  # the child closed the terminal
                    break
                if not chunk:
                    break
                transcript += chunk
            if not answered and prompt in transcript.decode(errors="replace"):
                # Answer only once the prompt is up, so the terminal has already
                # turned echo off and the password cannot land in the transcript.
                os.write(primary, typed.encode())
                answered = True
        assert answered, transcript.decode(errors="replace")
        assert process.wait(timeout=60) == 0
    finally:
        os.close(primary)
    return transcript.decode(errors="replace")


def test_keychain_password_is_asked_for_once_and_only_on_the_terminal(
    tmp_path: Path,
) -> None:
    # Whether the partition-list write needs the keychain password depends on
    # the host. When it does, the password is typed on the terminal and used for
    # exactly one security call: never through the environment, and never in the
    # argv of anything else the installer runs.
    env, log = _generated_identity_env(tmp_path, authorize_password="hunter2")
    identity_file = tmp_path / "identity"

    transcript = _ensure_identity_on_a_terminal(
        env, "Keychain password", "hunter2\n", identity_file
    )

    assert identity_file.read_text(encoding="utf-8") == GENERATED_IDENTITY
    assert "Keychain password" in transcript
    # The passwordless attempt first, then one retry carrying the password.
    passwordless, with_password = _authorize_calls(log)
    assert "-k" not in passwordless
    assert with_password[-3:] == [
        "-k",
        "hunter2",
        env["CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN"],
    ]
    assert [call for call in _authorize_calls(log) if "hunter2" in call] == [
        with_password
    ]
    assert "hunter2" not in transcript


def test_explicit_signing_identity_is_never_authorized(tmp_path: Path) -> None:
    # An identity the developer named is not the script's to modify.
    keychain = tmp_path / "signing.keychain-db"
    keychain.touch()
    log = tmp_path / "security-calls.log"
    fake_bin = tmp_path / "fake-bin"
    _write_executable(fake_bin / "codesign", "exit 0\n")
    wanted = "F2D26B5AFAAB910B340FBD8F480F88DF748D9D48"
    _write_executable(
        fake_bin / "security",
        f'printf "%s|" "$@" >> "{log}"; printf "\\n" >> "{log}"\n'
        f"printf '%s\\n' '  1) {wanted} \"Developer ID Application: Example\"'\n",
    )
    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN": str(keychain),
            "CUA_DRIVER_LOCAL_SIGNING_IDENTITY": wanted,
        }
    )

    result = _ensure_identity(env)

    assert result.stdout == wanted
    assert "set-key-partition-list" not in log.read_text(encoding="utf-8")


@pytest.mark.skipif(
    sys.platform != "darwin" or os.environ.get(REAL_KEYCHAIN_GATE) != "1",
    reason=f"set {REAL_KEYCHAIN_GATE}=1 on macOS to touch a real keychain",
)
def test_real_security_accepts_the_scoped_authorization_command(tmp_path: Path) -> None:
    # Exercises the exact command shape against the real `security` tool in a
    # throwaway keychain: whether the password is needed depends on the host, so
    # the shape has to work with `-k`, and it must not be able to reach a key it
    # was not pointed at.
    keychain = tmp_path / "cua-driver-signing-test.keychain-db"
    password = "cua-driver-real-keychain-test"
    request = tmp_path / "req.cnf"
    request.write_text(
        "[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n"
        f"[dn]\nCN={GENERATED_LABEL}\n[ext]\n"
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n"
        "extendedKeyUsage=critical,codeSigning\n",
        encoding="utf-8",
    )

    def run(*command: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            command, text=True, capture_output=True, check=False, stdin=subprocess.DEVNULL
        )

    assert run("security", "create-keychain", "-p", password, str(keychain)).returncode == 0
    try:
        assert run("security", "unlock-keychain", "-p", password, str(keychain)).returncode == 0
        assert (
            run(
                "openssl", "req", "-x509", "-newkey", "rsa:2048",
                "-keyout", str(tmp_path / "key.pem"), "-out", str(tmp_path / "cert.pem"),
                "-days", "1", "-nodes", "-config", str(request),
            ).returncode
            == 0
        )
        assert (
            run(
                "openssl", "pkcs12", "-export", "-legacy",
                "-inkey", str(tmp_path / "key.pem"), "-in", str(tmp_path / "cert.pem"),
                "-out", str(tmp_path / "id.p12"), "-passout", "pass:p12",
                "-name", GENERATED_LABEL,
            ).returncode
            == 0
        )
        assert (
            run(
                "security", "import", str(tmp_path / "id.p12"), "-k", str(keychain),
                "-P", "p12", "-A", "-T", "/usr/bin/codesign",
            ).returncode
            == 0
        )

        authorize = [
            "security", "set-key-partition-list",
            "-S", "apple-tool:,apple:,codesign:",
            "-l", GENERATED_LABEL, "-t", "private", "-s",
            "-k", password, str(keychain),
        ]
        authorized = run(*authorize)
        assert authorized.returncode == 0, authorized.stderr

        elsewhere = run(*authorize[:5], "Not This Key", *authorize[6:])
        assert elsewhere.returncode != 0
        assert "could not be found" in elsewhere.stderr
    finally:
        run("security", "delete-keychain", str(keychain))


@pytest.mark.parametrize("relative_target", [False, True], ids=["absolute", "relative"])
def test_installer_stages_binary_from_custom_cargo_target(
    tmp_path: Path, relative_target: bool
) -> None:
    fixture_root = tmp_path / "cua-driver"
    scripts_dir = fixture_root / "scripts"
    rust_dir = fixture_root / "rust"
    scripts_dir.mkdir(parents=True)
    rust_dir.mkdir()
    shutil.copy2(INSTALL_LOCAL, scripts_dir / INSTALL_LOCAL.name)
    shutil.copy2(LOCAL_SIGNING, scripts_dir / LOCAL_SIGNING.name)

    wayland_helper = fixture_root / "wayland-helper/winrects@cua"
    wayland_helper.mkdir(parents=True)
    (wayland_helper / "metadata.json").write_text('{"version":5}\n', encoding="utf-8")
    (wayland_helper / "extension.js").write_text("// semantic cursor v5\n", encoding="utf-8")

    stale_binary = rust_dir / "target/release/cua-driver"
    _write_executable(stale_binary, "printf 'stale workspace target\\n'")

    custom_target = (
        rust_dir / "relative custom target" if relative_target else tmp_path / "custom target"
    )
    cargo_target_dir = (
        str(custom_target.relative_to(rust_dir)) if relative_target else str(custom_target)
    )
    fake_bin = tmp_path / "fake-bin"
    _write_executable(
        fake_bin / "cargo",
        """set -eu
test "${1:-}" = build
test "$CARGO_TARGET_DIR" = "$EXPECTED_CARGO_TARGET_DIR"
mkdir -p "$CARGO_TARGET_DIR/release"
printf 'fresh custom target\n' > "$CARGO_TARGET_DIR/release/cua-driver"
printf 'fresh cursor theme compiler\n' > "$CARGO_TARGET_DIR/release/cua-cursor-theme"
chmod +x "$CARGO_TARGET_DIR/release/cua-driver"
chmod +x "$CARGO_TARGET_DIR/release/cua-cursor-theme"
""",
    )
    _write_executable(
        fake_bin / "uname",
        """case "${1:-}" in
    -s) printf 'Linux\n' ;;
    -m) printf 'x86_64\n' ;;
    *) exit 2 ;;
esac
""",
    )
    _write_executable(fake_bin / "systemctl", "exit 0")
    _write_executable(fake_bin / "pkill", "exit 0")

    local_home = tmp_path / "local-home"
    user_home = tmp_path / "home"
    installed_helper = user_home / ".local/share/gnome-shell/extensions/winrects@cua"
    installed_helper.mkdir(parents=True)
    (installed_helper / "metadata.json").write_text('{"version":4}\n', encoding="utf-8")
    (installed_helper / "extension.js").write_text("// legacy cursor\n", encoding="utf-8")
    install_bin = tmp_path / "install-bin"
    env = os.environ.copy()
    env.pop("SUDO_USER", None)
    env.update(
        {
            "HOME": str(user_home),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CARGO_TARGET_DIR": cargo_target_dir,
            "EXPECTED_CARGO_TARGET_DIR": str(custom_target),
            "CUA_DRIVER_SOURCE_SHA": "a" * 40,
            "CUA_DRIVER_LOCAL_HOME": str(local_home),
            "CUA_DRIVER_LOCAL_INSTALL_DIR": str(install_bin),
        }
    )

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / INSTALL_LOCAL.name), "--release"],
        cwd=fixture_root,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert (custom_target / "release/cua-driver").read_text() == "fresh custom target\n"
    assert (
        custom_target / "release/cua-cursor-theme"
    ).read_text() == "fresh cursor theme compiler\n"
    assert (install_bin / "cua-driver-local").read_text() == "fresh custom target\n"
    assert (
        local_home / "packages/current/cua-cursor-theme"
    ).read_text() == "fresh cursor theme compiler\n"
    assert (
        local_home / "packages/current/wayland-helper/winrects@cua/metadata.json"
    ).read_text() == '{"version":5}\n'
    assert (installed_helper / "metadata.json").read_text() == '{"version":5}\n'
    assert (installed_helper / "extension.js").read_text() == "// semantic cursor v5\n"


def _linux_fixture(tmp_path: Path) -> tuple[Path, Path, dict[str, str]]:
    """Stage a minimal Linux install-local fixture: (scripts_dir, fake_bin, env)."""
    fixture_root = tmp_path / "cua-driver"
    scripts_dir = fixture_root / "scripts"
    rust_dir = fixture_root / "rust"
    scripts_dir.mkdir(parents=True)
    rust_dir.mkdir()
    for script in (INSTALL_LOCAL, LOCAL_SIGNING, DISPATCHER):
        shutil.copy2(script, scripts_dir / script.name)

    fake_bin = tmp_path / "fake-bin"
    _write_executable(
        fake_bin / "cargo",
        """set -eu
mkdir -p "$CARGO_TARGET_DIR/debug"
printf 'fresh driver\n' > "$CARGO_TARGET_DIR/debug/cua-driver"
printf 'fresh cursor theme compiler\n' > "$CARGO_TARGET_DIR/debug/cua-cursor-theme"
chmod +x "$CARGO_TARGET_DIR/debug/cua-driver"
chmod +x "$CARGO_TARGET_DIR/debug/cua-cursor-theme"
""",
    )
    _write_executable(
        fake_bin / "uname",
        """case "${1:-}" in
    -s) printf 'Linux\n' ;;
    -m) printf 'x86_64\n' ;;
    *) exit 2 ;;
esac
""",
    )
    _write_executable(fake_bin / "systemctl", "exit 0")
    _write_executable(fake_bin / "pkill", "exit 0")

    env = os.environ.copy()
    env.pop("SUDO_USER", None)
    env.pop("CARGO_TARGET_DIR", None)
    env.pop("CUA_DRIVER_LOCAL_INSTALL_DIR", None)
    env.update(
        {
            "HOME": str(tmp_path / "home"),
            "PATH": f"{fake_bin}:/usr/bin:/bin",
            "CUA_DRIVER_SOURCE_SHA": "a" * 40,
            "CUA_DRIVER_LOCAL_HOME": str(tmp_path / "local-home"),
        }
    )
    return scripts_dir, fake_bin, env


@pytest.mark.parametrize(
    "flag_form",
    [["--bin-dir", "{bin}"], ["--bin-dir={bin}"]],
    ids=["separate", "equals"],
)
def test_dispatcher_forwards_bin_dir_override(tmp_path: Path, flag_form: list[str]) -> None:
    """--bin-dir is documented by install-local.sh and forwarded verbatim; the
    helper must accept both spellings and honor them over the env default."""
    scripts_dir, _, env = _linux_fixture(tmp_path)
    flag_bin = tmp_path / "flag-bin"
    env["CUA_DRIVER_LOCAL_INSTALL_DIR"] = str(tmp_path / "env-bin")
    args = [arg.format(bin=flag_bin) for arg in flag_form]

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / DISPATCHER.name), *args],
        cwd=scripts_dir.parent,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert (flag_bin / "cua-driver-local").read_text() == "fresh driver\n"
    assert not (tmp_path / "env-bin").exists()


def test_relative_bin_dir_is_rejected(tmp_path: Path) -> None:
    """A relative bin dir would land inside the Cargo workspace (the symlink is
    created after cd'ing there) and uninstall-local.sh could never remove it."""
    scripts_dir, _, env = _linux_fixture(tmp_path)

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / DISPATCHER.name), "--bin-dir", "relative/bin"],
        cwd=scripts_dir.parent,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 2, result.stdout + result.stderr
    assert "absolute path" in result.stderr


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="ETXTBSY on a running binary is Linux-specific"
)
def test_reinstall_over_a_running_driver(tmp_path: Path) -> None:
    """Staging must replace the versioned binary by rename, not write through it.

    The version tag is stable per build config, so every rebuild targets the
    same path. If a previous cua-driver-local is still executing out of it, a
    write-in-place `cp` fails with ETXTBSY ("Text file busy") and the install
    dies mid-stage. Reproduce that with a real running executable.
    """
    scripts_dir, _, env = _linux_fixture(tmp_path)
    versioned = (
        tmp_path / "local-home/packages/releases/0.0.0-local-debug-x86_64-unknown-linux-gnu"
    )
    versioned.mkdir(parents=True)
    busy = versioned / "cua-driver-local"
    shutil.copy2("/bin/sleep", busy)

    running = subprocess.Popen([str(busy), "60"])
    try:
        result = subprocess.run(
            ["/bin/bash", str(scripts_dir / DISPATCHER.name)],
            cwd=scripts_dir.parent,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
    finally:
        running.terminate()
        running.wait(timeout=10)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "Text file busy" not in result.stderr
    assert busy.read_text() == "fresh driver\n"
    # The rename must not leave the temp file behind.
    assert not list(versioned.glob("*.stage.*"))


def test_bin_dir_without_value_is_rejected(tmp_path: Path) -> None:
    scripts_dir, _, env = _linux_fixture(tmp_path)

    result = subprocess.run(
        ["/bin/bash", str(scripts_dir / DISPATCHER.name), "--bin-dir"],
        cwd=scripts_dir.parent,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 2, result.stdout + result.stderr
    assert "--bin-dir requires a value" in result.stderr
