"""Platform requirement tests for the Windows Arena adapter.

Windows Arena boots a Windows guest through QEMU with KVM acceleration, so it
needs an x86_64 host with /dev/kvm. Only the supported-host test exercises the
real machine and is skipped elsewhere; the rejection tests monkeypatch the host
so they run on every platform, not just hosts where the guard already passes.

This test lives under ``cua_bench/tests`` rather than next to the adapter: the
adapter's ``__init__.py`` eagerly imports evaluator dependencies, so pytest
cannot collect a test inside ``tasks/winarena_adapter`` once the whole-module
skip is removed. Loading ``cli.py`` by path keeps the guard testable with only
the standard library available.
"""

import importlib.util
import os
import platform
from pathlib import Path

import pytest

CLI_PATH = Path(__file__).resolve().parents[2] / "tasks" / "winarena_adapter" / "cli.py"


def _load_cli():
    spec = importlib.util.spec_from_file_location("winarena_adapter_cli", CLI_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.skipif(
    platform.machine().lower() not in ("x86_64", "amd64") or not os.path.exists("/dev/kvm"),
    reason="Windows Arena requires an x86_64 host with KVM (/dev/kvm)",
)
def test_supported_host_passes_requirements():
    cli = _load_cli()
    cli.check_platform_requirements()


def test_non_x86_64_hosts_are_rejected(monkeypatch):
    cli = _load_cli()
    monkeypatch.setattr(cli.platform, "machine", lambda: "arm64")
    with pytest.raises(RuntimeError, match="x86_64"):
        cli.check_platform_requirements()


def test_missing_kvm_is_rejected(monkeypatch):
    cli = _load_cli()
    monkeypatch.setattr(cli.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(cli.os.path, "exists", lambda path: False)
    with pytest.raises(RuntimeError, match="KVM"):
        cli.check_platform_requirements()


def test_no_kvm_flag_accepts_software_emulation(monkeypatch):
    cli = _load_cli()
    monkeypatch.setattr(cli.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(cli.os.path, "exists", lambda path: False)
    cli.check_platform_requirements(no_kvm=True)
