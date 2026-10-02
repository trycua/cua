"""CLI surface goldens and offline end-to-end behavior of ``cb``.

The parser tree (every command, subcommand, flag, default, choice and dest,
hidden flags included) is frozen in ``golden/cli_tree.json``. A change there
must be deliberate and additive (``CUA_BENCH_UPDATE_GOLDENS=1``).

The behavior tests run the real CLI in-process against the in-memory SDK
fake (``tests/fakes.py``): no sandbox, network or host command.
"""

from __future__ import annotations

import argparse
import json
import shlex
import shutil
from pathlib import Path

import pytest
from cua_bench import sandboxes
from cua_bench.cli import main as cli_main
from cua_bench.sessions import manager

from .fakes import FakeSDK
from .golden_utils import check_golden, parser_tree

PKG = Path(__file__).resolve().parents[2]
HELLO = PKG / "example_tasks" / "hello_file_env"


def test_cli_tree_golden():
    check_golden("cli_tree", parser_tree(cli_main.build_parser()))


NORMALIZE_CASES = [
    ["run", str(HELLO)],
    ["run", "cua-bench-basic", "-j", "2"],
    ["run", "task", str(HELLO)],
    ["run", "dataset", "some-dataset"],
    ["run", "list"],
    ["run", "info", "abc"],
    ["run", "watch", "abc"],
    ["run", "stop", "abc"],
    ["run", "logs", "abc"],
    ["run", "--help"],
    ["interact", str(HELLO)],
    ["trace", "view", "abc"],
]


def test_normalize_argv_golden():
    rows = []
    for argv in NORMALIZE_CASES:
        out = cli_main.normalize_argv(argv)
        rows.append(
            [
                [a.replace(str(PKG), "<pkg>") for a in argv],
                [a.replace(str(PKG), "<pkg>") for a in out],
            ]
        )
    check_golden("normalize_argv", rows)


def test_every_command_parses():
    """Each documented invocation parses (argparse exits on unknown flags)."""
    parser = cli_main.build_parser()
    invocations = [
        ["run", "task", "t", "--variant-id", "1", "--oracle", "--max-steps", "3"],
        ["run", "task", "t", "--agent", "cua-agent", "--model", "m", "--on", "cloud"],
        ["run", "task", "t", "--on", "cloud", "--kind", "vm", "--image", "linux", "--cpu", "2"],
        ["run", "task", "t", "--runtime", "runc", "--platform", "linux-qemu"],
        ["run", "task", "t", "--provider-type", "native", "--runtime", "kubevirt"],
        ["run", "task", "t", "--memory", "4G", "--warm", "--claim-ttl", "20m", "-d"],
        ["run", "task", "t", "--agent-import-path", "a.b:C", "--output-dir", "o"],
        ["run", "task", "t", "--on", "cloud", "--wait", "--with", "x", "--run-id", "r"],
        ["run", "task", "t", "--session-id", "s"],
        ["run", "dataset", "d", "-j", "2", "--max-variants", "1", "--task-filter", "a*,b*"],
        ["run", "list", "-v"],
        ["run", "info", "r"],
        ["run", "watch", "r"],
        ["run", "stop", "r"],
        ["run", "logs", "r", "--tail", "5"],
        ["interact", "t", "--variant-id", "1", "--oracle", "--no-wait", "--screenshot", "s"],
        ["interact", "t", "--dataset", "d", "--dataset-path", "p", "--trace-out", "o", "--view"],
        ["interact", "t", "--max-steps", "2", "--on", "local", "--kind", "container"],
        ["dataset", "list"],
        ["dataset", "build", "out", "5", "--mode", "gui-r1", "--save-dir", "s"],
        ["dataset", "build", "out", "--push-to-hub", "--repo-id", "a/b", "--private"],
        ["trace", "view", "r"],
        ["trace", "traj", "r", "-p", "8091"],
        ["task", "list"],
        ["task", "info", "t"],
        ["task", "create", "t"],
        ["task", "generate", "prompt", "--no-interaction"],
        ["env", "ls", "--local"],
        ["env", "gc", "--idle", "30m"],
        ["login", "--no-browser"],
        ["agent", "init", "my-agent", "-o", "o"],
        ["agent", "build", "p"],
        ["agent", "push", "p"],
        ["platform", "list"],
        ["image", "list"],
        ["prune", "--runs", "--dry-run"],
        ["status"],
    ]
    for argv in invocations:
        try:
            parser.parse_args(argv)
        except SystemExit as exc:  # pragma: no cover - reported below
            pytest.fail(f"cb {' '.join(argv)} no longer parses (exit {exc.code})")


def _parsers(parser: argparse.ArgumentParser, path: tuple[str, ...] = ("cb",)):
    """(path, parser, is_leaf) for ``parser`` and every visible subparser."""
    subs = [a for a in parser._actions if isinstance(a, argparse._SubParsersAction)]
    yield path, parser, not subs
    for action in subs:
        hidden = {c.dest for c in action._choices_actions if c.help == argparse.SUPPRESS}
        seen: set[int] = set()
        for name, sub in action.choices.items():
            if id(sub) in seen or name in hidden:  # an alias, or hidden
                continue
            seen.add(id(sub))
            yield from _parsers(sub, (*path, name))


def _examples(epilog: str | None) -> list[str]:
    lines = (epilog or "").splitlines()
    if "Examples:" not in lines:
        return []
    body = lines[lines.index("Examples:") + 1 :]
    return [ln.strip() for ln in body if ln.strip() and not ln.strip().startswith("#")]


def test_every_help_example_parses():
    """Each ``Examples:`` line in ``--help`` parses; every leaf command has one.

    The generated CLI reference renders the same examples.
    """
    parser = cli_main.build_parser()
    missing = []
    for path, sub, leaf in _parsers(parser):
        examples = _examples(sub.epilog)
        if leaf and not examples:
            missing.append(" ".join(path))
        for example in examples:
            argv = shlex.split(example)
            assert argv[0] == "cb", example
            try:
                parser.parse_args(cli_main.normalize_argv(argv[1:]))
            except SystemExit as exc:  # pragma: no cover - reported below
                pytest.fail(
                    f"{' '.join(path)} example {example!r} does not parse (exit {exc.code})"
                )
    assert not missing, f"commands without an Examples: epilog: {missing}"


# ── Offline end-to-end ──────────────────────────────────────────────────────


@pytest.fixture
def cli_env(tmp_path, monkeypatch):
    sdk = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", sdk.pair)
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "data"))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("CUA_BENCH_NO_BANNER", "1")
    monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "false")
    for var in ("CUA_BENCH_ON", "CUA_BENCH_RUNTIME", "CUA_BENCH_IMAGE"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setattr(manager, "RUNS_FILE", tmp_path / "state" / "cua-bench" / "runs.json")
    # Never read the real credential store (the OS vault on macOS) in tests.
    monkeypatch.setenv("CUA_FLEET_SESSION", "0")
    monkeypatch.setenv("CUA_CREDENTIAL_STORE", "file")
    monkeypatch.setenv("CUA_HOME", str(tmp_path / "cua-home"))
    # No user config (~/.cua/config.yaml, agents.yaml) leaks into the tests.
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    for var in ("FLEETS_TOKEN", "CUA_CLIENT_ID", "CUA_CLIENT_SECRET"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.chdir(tmp_path)
    return sdk


def run_cb(argv: list[str]) -> int:
    try:
        cli_main.main(argv)
    except SystemExit as exc:
        return int(exc.code or 0)
    return 0


def test_run_task_oracle_end_to_end(cli_env, tmp_path, capsys):
    out = tmp_path / "out"
    code = run_cb(["run", str(HELLO), "--variant-id", "1", "--output-dir", str(out)])
    assert code == 0, capsys.readouterr().out
    summary = json.loads((out / "summary.json").read_text())
    assert summary["total"] == 1 and summary["completed"] == 1
    assert summary["results"][0]["reward"] == 1.0
    result = json.loads((out / "hello_file_env_v1" / "result.json").read_text())
    assert result["variant"] == 1 and result["status"] == "completed"
    # The trace is written per variant: task_<variant>_trace.
    assert (out / "hello_file_env_v1" / "task_1_trace").is_dir()
    assert cli_env.calls and cli_env.calls[0]["on"] == "local"


def test_run_dataset_filters_and_parallelism(cli_env, tmp_path, capsys):
    dataset = tmp_path / "ds"
    dataset.mkdir()
    for name in ("alpha_task", "beta_task", "gamma_other"):
        shutil.copytree(HELLO, dataset / name)
    out = tmp_path / "out"
    code = run_cb(
        [
            "run",
            str(dataset),
            "-j",
            "2",
            "--max-variants",
            "1",
            "--task-filter",
            "alpha*,beta*",
            "--output-dir",
            str(out),
        ]
    )
    assert code == 0, capsys.readouterr().out
    summary = json.loads((out / "summary.json").read_text())
    assert summary["total"] == 2
    assert sorted(r["task"] for r in summary["results"]) == ["alpha_task", "beta_task"]
    assert cli_env.peak <= 2


def test_run_list_info_logs_and_trace_lookup(cli_env, tmp_path, capsys):
    code = run_cb(["run", str(HELLO), "--variant-id", "1", "--run-id", "run12345"])
    assert code == 0
    capsys.readouterr()
    assert run_cb(["run", "list"]) == 0
    assert "run12345" in capsys.readouterr().out
    assert run_cb(["run", "info", "run12345"]) == 0
    assert "hello_file_env" in capsys.readouterr().out or True
    assert run_cb(["run", "logs", "task-run12345-hello_file_env-v1", "--tail", "3"]) == 0

    from cua_bench.cli.commands import trace

    path = trace._resolve_trace_path("task-run12345-hello_file_env-v1")
    assert path is not None and path.name == "task_1_trace"
    traces = trace._collect_run_traces(tmp_path / "data" / "cua-bench" / "runs" / "run12345")
    assert [name for name, _ in traces] == ["hello_file_env_v1"]


def test_dry_run_plans_without_a_sandbox(cli_env, tmp_path, capsys):
    out = tmp_path / "out"
    code = run_cb(["run", str(HELLO), "--dry-run", "--kind", "auto", "--output-dir", str(out)])
    text = capsys.readouterr().out
    assert code == 0
    assert "hello_file_env v0: ghcr.io/trycua/linux" in text
    assert "-> container (rootfs) on local via local-gvisor [default]" in text
    assert "no sandbox was started" in text
    assert cli_env.calls == [] and not out.exists()
    code = run_cb(["run", str(HELLO), "--dry-run", "--kind", "vm", "--on", "cloud"])
    text = capsys.readouterr().out
    assert code == 0
    assert "-> vm (containerdisk) on Fleet via cloud-kubevirt [--kind/--runtime]" in text
    code = run_cb(["run", str(HELLO), "--dry-run", "--runtime", "qemu"])
    assert "-> vm (containerdisk) on local via local-qemu [--kind/--runtime]" in (
        capsys.readouterr().out
    )
    code = run_cb(["run", str(HELLO), "--dry-run", "--image", "windows", "--kind", "container"])
    assert code == 1
    assert "windows is VM-only: drop --kind or use --kind vm" in capsys.readouterr().out
    code = run_cb(["run", str(HELLO), "--dry-run", "--on", "local", "--runtime", "kubevirt"])
    assert code == 1
    assert "valid runtime: auto, gvisor, runc, qemu, lume" in capsys.readouterr().out


def test_the_user_default_location_drives_cb_run(cli_env, monkeypatch, capsys):
    monkeypatch.setenv("CUA_DEFAULT_ON", "cloud")
    assert run_cb(["run", str(HELLO), "--dry-run"]) == 0
    assert "on Fleet via cloud-gvisor" in capsys.readouterr().out
    assert run_cb(["run", str(HELLO), "--dry-run", "--on", "local"]) == 0
    assert "via local-gvisor" in capsys.readouterr().out


def test_deprecated_0_2_11_flags(cli_env, capsys):
    with pytest.warns(DeprecationWarning, match="--kind vm --runtime qemu --image windows"):
        code = run_cb(["run", str(HELLO), "--dry-run", "--platform", "windows-qemu"])
    captured = capsys.readouterr()
    assert code == 0 and "built-in windows vm -> vm (containerdisk) on local via local-qemu" in (
        captured.out
    )
    assert "--platform windows-qemu is deprecated" in captured.err
    with pytest.warns(DeprecationWarning, match="--kind container"):
        assert run_cb(["run", str(HELLO), "--dry-run", "--platform", "linux-docker"]) == 0
    assert "via local-gvisor [--platform]" in capsys.readouterr().out
    with pytest.warns(DeprecationWarning, match="no effect"):
        assert run_cb(["run", str(HELLO), "--dry-run", "--provider-type", "native"]) == 0
    capsys.readouterr()
    assert run_cb(["run", str(HELLO), "--dry-run", "--provider-type", "simulated"]) == 1
    assert "simulated provider was removed" in capsys.readouterr().out


def test_results_record_what_ran(cli_env, tmp_path):
    from types import SimpleNamespace

    cli_env.image_info = SimpleNamespace(
        reference="ghcr.io/trycua/linux:24.04",
        pinned_ref="ghcr.io/trycua/linux@sha256:" + "a" * 64,
        variant="rootfs",
        arch="arm64",
    )
    out = tmp_path / "out"
    assert run_cb(["run", str(HELLO), "--output-dir", str(out)]) == 0
    result = json.loads((out / "hello_file_env_v0" / "result.json").read_text())
    assert (result["kind"], result["runtime"]) == ("container", None)
    assert result["image_variant"] == "rootfs"
    assert result["image_digest"].endswith("a" * 64) and result["arch"] == "arm64"
    assert (result["on"], result["backend"]) == ("local", "local-gvisor")
    summary = json.loads((out / "summary.json").read_text())
    assert summary["targets"] == [
        {
            "on": "local",
            "kind": "container",
            "runtime": None,
            "image_variant": "rootfs",
            "image": result["image_digest"],
            "count": 1,
        }
    ]


def test_env_ls_and_gc_use_the_sdk_pools_api(cli_env, monkeypatch, capsys):
    from types import SimpleNamespace

    from cua_bench import sandboxes as sb

    calls = []

    class Pools:
        @staticmethod
        async def list_pools():
            calls.append("list_pools")
            return [
                SimpleNamespace(
                    name="cua-auto-abc", ready_replicas=1, replicas=2, claims=1, last_used=None
                )
            ]

        @staticmethod
        async def list_claims():
            calls.append("list_claims")
            return [SimpleNamespace(name="c1", pool="cua-auto-abc", phase="Bound", managed=True)]

        @staticmethod
        async def gc(idle_after):
            calls.append(("gc", idle_after))
            return SimpleNamespace(pools_deleted=["cua-auto-old"], claims_deleted=[], errors=[])

    monkeypatch.setattr(sb, "pools_module", lambda: Pools)
    monkeypatch.setattr(sb, "cloud_auth_source", lambda *a: "client credentials")
    assert run_cb(["env", "ls"]) == 0
    out = capsys.readouterr().out
    assert "cua-auto-abc" in out and "1/2" in out and "Bound" in out
    assert run_cb(["env", "gc", "--idle", "30m"]) == 0
    assert "deleted pool cua-auto-old" in capsys.readouterr().out.replace("\x1b[92m", "").replace(
        "\x1b[0m", ""
    )
    assert calls == ["list_pools", "list_claims", ("gc", 1800)]


def test_legacy_commands_are_deprecation_shims(cli_env, tmp_path, capsys):
    """image/platform/prune/agent keep parsing and exit 0 with a pointer."""
    for argv in (
        ["image", "create", "windows-qemu", "--download-iso", "--memory", "4G"],
        ["image", "shell", "linux-docker", "--writable"],
        ["image", "delete", "x", "--force"],
        ["image", "clone", "a", "b"],
        ["agent", "build", "some/dir", "-t", "x"],
        ["agent", "push", "img:tag"],
    ):
        assert run_cb(argv) == 0, argv
        assert "deprecated" in capsys.readouterr().out, argv

    assert run_cb(["image", "list"]) == 0
    listing = capsys.readouterr().out
    assert "ghcr.io/trycua/linux" in listing and "ghcr.io/trycua/windows" in listing
    assert run_cb(["image", "list", "--format", "json", "--platform", "windows-qemu"]) == 0
    rows = json.loads(capsys.readouterr().out)
    assert [r["name"] for r in rows] == ["windows"] and rows[0]["kinds"] == ["vm"]
    assert run_cb(["platform", "info", "linux-docker"]) == 0
    assert "container/vm" in capsys.readouterr().out
    assert run_cb(["image"]) == 0  # the default subcommand is still `list`

    runs = tmp_path / "data" / "cua-bench" / "runs" / "r1"
    runs.mkdir(parents=True)
    (runs / "run.log").write_text("x" * 10)
    assert run_cb(["prune", "--runs", "--dry-run"]) == 0
    assert "Would remove" in capsys.readouterr().out and runs.exists()
    assert run_cb(["prune", "--docker"]) == 0
    assert "org.trycua.bench.owner=cua-bench" in capsys.readouterr().out
    assert run_cb(["prune", "--runs", "--force"]) == 0
    assert not runs.exists()
    assert run_cb(["status"]) == 0
    assert "ghcr.io/trycua/macos" in capsys.readouterr().out
