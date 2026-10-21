"""``cb interact`` shows the sandbox's display: it prints a ``Display:``
line (the cua-spacesd viewer link), opens it unless told not to, and
reports (never swallows) a missing display. In-memory SDK fake only; no
browser is launched (``webbrowser.open`` is replaced)."""

from __future__ import annotations

import builtins

import pytest
from cua_bench import sandboxes
from cua_bench.cli.commands import interact
from cua_bench.targets import EnvSpec, Target

from .test_cli_golden import HELLO, run_cb
from .test_cli_golden import cli_env as _cli_env

cli_env = _cli_env

VIEWER = "http://127.0.0.1:40123/viewer/#ticket=abc"


@pytest.fixture
def browser(monkeypatch):
    opened: list[str] = []

    def fake_open(url, *args, **kwargs):
        opened.append(url)
        return True

    monkeypatch.setattr(interact.webbrowser, "open", fake_open)
    monkeypatch.setattr(builtins, "input", lambda *a: "")
    monkeypatch.delenv("CUA_BENCH_NO_BROWSER", raising=False)
    probed: list[str] = []

    async def fake_probe(url):
        probed.append(url)
        return None

    monkeypatch.setattr(interact, "_probe_display", fake_probe)
    return opened, probed


def test_local_interact_prints_and_opens_the_viewer(cli_env, browser, capsys):
    opened, probed = browser
    cli_env.display = VIEWER
    assert run_cb(["interact", str(HELLO)]) == 0
    out = capsys.readouterr().out
    assert f"Display: \x1b[1m{VIEWER}" in out
    assert opened == [VIEWER] and probed == [VIEWER]
    # The viewer is on the "env" service every sandbox declares: nothing
    # extra is published.
    assert "services" not in cli_env.calls[0]
    assert cli_env.calls[0]["on"] == "local"


@pytest.mark.parametrize("opt_out", ["flag", "env", "no-wait"])
def test_browser_opt_outs(cli_env, browser, capsys, monkeypatch, opt_out):
    opened, _ = browser
    cli_env.display = VIEWER
    argv = ["interact", str(HELLO)]
    if opt_out == "flag":
        argv.append("--no-browser")
    elif opt_out == "env":
        monkeypatch.setenv("CUA_BENCH_NO_BROWSER", "1")
    else:
        argv.append("--no-wait")
    assert run_cb(argv) == 0
    assert "Display: " in capsys.readouterr().out
    assert opened == []


def test_missing_display_is_reported(cli_env, browser, capsys):
    opened, _ = browser
    cli_env.display = NotImplementedError("this sandbox declares no display service")
    assert run_cb(["interact", str(HELLO), "--no-wait"]) == 0
    out = capsys.readouterr().out
    assert "Display: not available (this sandbox declares no display service)" in out
    assert opened == []


def test_display_errors_are_reported(cli_env, browser, capsys):
    cli_env.display = RuntimeError("gateway said no")
    assert run_cb(["interact", str(HELLO), "--no-wait"]) == 0
    assert "could not get the display URL: RuntimeError: gateway said no" in capsys.readouterr().out


def test_vnc_address_is_printed_but_not_opened(cli_env, browser, capsys):
    opened, probed = browser
    cli_env.display = "vnc://127.0.0.1:5901"
    assert run_cb(["interact", str(HELLO)]) == 0
    assert "Display: \x1b[1mvnc://127.0.0.1:5901" in capsys.readouterr().out
    assert opened == [] and probed == []


def test_a_silent_display_is_reported(cli_env, browser, capsys, monkeypatch):
    cli_env.display = VIEWER

    async def failing_probe(url):
        return "ConnectError: refused"

    monkeypatch.setattr(interact, "_probe_display", failing_probe)
    assert run_cb(["interact", str(HELLO), "--no-wait"]) == 0
    assert "not answering (ConnectError: refused)" in capsys.readouterr().out


@pytest.mark.asyncio
async def test_probe_is_bounded(monkeypatch):
    monkeypatch.setattr(interact, "DISPLAY_PROBE_ATTEMPTS", 2)
    monkeypatch.setattr(interact, "DISPLAY_PROBE_INTERVAL_S", 0.0)
    # Port 1 on loopback: nothing listens, the connect is refused at once.
    problem = await interact._probe_display("http://127.0.0.1:1/viewer/")
    assert problem and "Connect" in problem


def test_no_display_service_is_published():
    assert not hasattr(sandboxes, "VIEWER_SERVICE")
    spec = EnvSpec(provider="native")
    windows = EnvSpec(provider="native", os_type="windows", kind="vm")
    for env_spec, on in ((spec, "local"), (spec, "cloud"), (windows, "local")):
        kwargs = sandboxes.ephemeral_kwargs(env_spec, Target(on=on))
        assert "services" not in kwargs


def test_interact_live_bundle_and_legacy_export(cli_env, browser, tmp_path):
    from cua_bench.trace_bundle import TraceBundleReader
    from datasets import load_from_disk

    bundle = tmp_path / "live"
    legacy = tmp_path / "legacy"
    assert (
        run_cb(
            [
                "interact",
                str(HELLO),
                "--no-wait",
                "--trace-bundle",
                str(bundle),
                "--trace-out",
                str(legacy),
            ]
        )
        == 0
    )
    with TraceBundleReader(bundle) as reader:
        assert reader.status()["state"] == "completed"
        assert [event["event_name"] for event in reader.batch()["events"]] == ["reset", "evaluate"]
    assert list(load_from_disk(str(legacy))["event_name"]) == ["reset", "evaluate"]


def test_run_live_bundle_keeps_existing_exports(cli_env, tmp_path):
    from cua_bench.trace_bundle import TraceBundleReader

    output = tmp_path / "run"
    assert run_cb(["run", str(HELLO), "--live-trace", "--output-dir", str(output)]) == 0
    bundles = list(output.rglob("trace.json"))
    assert len(bundles) == 1
    with TraceBundleReader(bundles[0].parent) as reader:
        assert reader.status()["state"] == "completed"
        assert "evaluate" in [event["event_name"] for event in reader.batch()["events"]]
    assert list(output.rglob("dataset_info.json"))
    assert list(output.rglob("trajectory.json"))
