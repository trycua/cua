import subprocess
import sys

from computer_server.cli import parse_args


def test_cua_driver_backend_options_are_parsed():
    args = parse_args(
        [
            "--backend",
            "cua-driver",
            "--driver-mode",
            "daemon",
            "--driver-socket",
            "/tmp/driver.sock",
            "--capture-scope",
            "auto",
        ]
    )

    assert args.backend == "cua-driver"
    assert args.driver_mode == "daemon"
    assert args.driver_socket == "/tmp/driver.sock"
    assert args.capture_scope == "auto"


def test_cua_driver_backend_defaults_to_embedded_mode():
    args = parse_args(["--backend", "cua-driver"])

    assert args.driver_mode is None


def test_package_import_does_not_construct_server_handlers():
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys; import computer_server; "
                "assert 'computer_server.server' not in sys.modules; "
                "assert 'computer_server.main' not in sys.modules"
            ),
        ],
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 0, result.stderr
