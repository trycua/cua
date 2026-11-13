"""Tests for cua-driver wheel build helpers."""

import importlib.util
from pathlib import Path


def load_build_wheel_module():
    module_path = Path(__file__).resolve().parents[1] / "build_wheel.py"
    spec = importlib.util.spec_from_file_location("build_wheel", module_path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_wheel_tags_are_platform_specific():
    build_wheel = load_build_wheel_module()

    assert build_wheel.get_wheel_tag("darwin", "universal") == "py3-none-macosx_13_0_universal2"
    assert build_wheel.get_wheel_tag("linux", "x86_64") == "py3-none-manylinux_2_31_x86_64"
    assert build_wheel.get_wheel_tag("linux", "arm64") == "py3-none-manylinux_2_31_aarch64"
    assert build_wheel.get_wheel_tag("windows", "x86_64") == "py3-none-win_amd64"
    assert build_wheel.get_wheel_tag("windows", "arm64") == "py3-none-win_arm64"


def test_release_archives_include_cli_and_uniffi_library():
    build_wheel = load_build_wheel_module()

    _, darwin_files = build_wheel.get_release_url("1.2.3", "darwin", "universal")
    _, linux_files = build_wheel.get_release_url("1.2.3", "linux", "x86_64")
    _, windows_files = build_wheel.get_release_url("1.2.3", "windows", "arm64")

    assert darwin_files == ["cua-driver", "libcua_driver_sdk.dylib"]
    assert linux_files == ["cua-driver", "libcua_driver_sdk.so"]
    assert windows_files == [
        "cua-driver.exe",
        "cua-driver-uia.exe",
        "cua_driver_sdk.dll",
    ]


def test_license_metadata_stays_legacy_upload_compatible():
    pyproject = Path(__file__).resolve().parents[1] / "pyproject.toml"
    pyproject_text = pyproject.read_text()

    assert 'license = { text = "MIT" }' in pyproject_text
    assert 'license = "MIT"' not in pyproject_text


def test_thin_wheel_manifest_names_the_signed_archive_of_each_platform(tmp_path):
    import json

    build_wheel = load_build_wheel_module()

    # macOS: the archive that holds the notarized CuaDriver.app, not the bare CLI.
    assert build_wheel.get_cli_asset("1.2.3", "darwin", "universal") == (
        "darwin-universal",
        "cua-driver-rs-1.2.3-darwin-universal.tar.gz",
    )
    assert build_wheel.get_cli_asset("1.2.3", "linux", "arm64") == (
        "linux-arm64",
        "cua-driver-rs-1.2.3-linux-arm64-binary.tar.gz",
    )
    assert build_wheel.get_cli_asset("1.2.3", "windows", "x86_64") == (
        "windows-x86_64",
        "cua-driver-rs-1.2.3-windows-x86_64-binary.zip",
    )

    dest = tmp_path / "_release.json"
    manifest = build_wheel.write_release_manifest("1.2.3", "linux", "x86_64", "ab" * 32, 1234, dest)
    assert json.loads(dest.read_text()) == manifest
    assert manifest["base_url"] == "https://github.com/trycua/cua/releases/download/cua-driver-rs-v1.2.3/"
    assert manifest["assets"] == {
        "linux-x86_64": {
            "name": "cua-driver-rs-1.2.3-linux-x86_64-binary.tar.gz",
            "size": 1234,
            "sha256": "ab" * 32,
        }
    }


def test_manifest_keys_match_the_runtime_platform_keys():
    """build_wheel writes the key that fetch.platform_key() looks up."""
    import platform
    import sys

    spec = importlib.util.spec_from_file_location(
        "fetch_src", Path(__file__).resolve().parents[1] / "src" / "cua_driver" / "fetch.py"
    )
    fetch = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fetch)
    build_wheel = load_build_wheel_module()
    for system, plat, machine, arch in [
        ("darwin", "darwin", "arm64", "universal"),
        ("linux", "linux", "x86_64", "x86_64"),
        ("linux", "linux", "aarch64", "arm64"),
        ("windows", "win32", "AMD64", "x86_64"),
        ("windows", "win32", "ARM64", "arm64"),
    ]:
        original = (sys.platform, platform.machine)
        sys.platform, platform.machine = plat, lambda m=machine: m
        try:
            runtime_key = fetch.platform_key()
        finally:
            sys.platform, platform.machine = original
        assert build_wheel.get_cli_asset("1.2.3", system, arch)[0] == runtime_key
