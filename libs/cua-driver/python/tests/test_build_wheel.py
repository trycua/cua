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

    assert build_wheel.get_wheel_tag("darwin", "universal") == "py3-none-macosx_11_0_universal2"
    assert build_wheel.get_wheel_tag("linux", "x86_64") == "py3-none-manylinux_2_31_x86_64"
    assert build_wheel.get_wheel_tag("linux", "arm64") == "py3-none-manylinux_2_31_aarch64"
    assert build_wheel.get_wheel_tag("windows", "x86_64") == "py3-none-win_amd64"
    assert build_wheel.get_wheel_tag("windows", "arm64") == "py3-none-win_arm64"


def test_license_metadata_stays_legacy_upload_compatible():
    pyproject = Path(__file__).resolve().parents[1] / "pyproject.toml"
    pyproject_text = pyproject.read_text()

    assert 'license = { text = "MIT" }' in pyproject_text
    assert 'license = "MIT"' not in pyproject_text
