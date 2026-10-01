"""Harbor-compatible outputs, attempts/pass@k, retries and the versioned registry.

Hermetic: sandboxes are the in-memory fake; the registry tests fetch from a
local git repository in a temp dir (file:// URL), never the network.
``HARBOR_SRC=<harbor checkout>/src`` additionally validates trajectory.json
with Harbor's own ATIF model.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
import types
from pathlib import Path
from types import SimpleNamespace

import pytest
from cua_bench import registry, sandboxes
from cua_bench.results import build_atif, eligible_k, pass_at_k

from .fakes import FakeSDK
from .test_cli_golden import HELLO, cli_env, run_cb  # noqa: F401 - fixture reuse


def _check_atif(doc: dict, variant_dir: Path) -> None:
    assert doc["schema_version"] == "ATIF-v1.8"
    assert doc["agent"]["name"] and doc["agent"]["version"]
    steps = doc["steps"]
    assert [s["step_id"] for s in steps] == list(range(1, len(steps) + 1))
    for step in steps:
        assert step["source"] in ("system", "user", "agent")
        if step["source"] != "agent":
            for field in ("model_name", "reasoning_content", "tool_calls", "metrics"):
                assert field not in step, (field, step)
        parts = step["message"] if isinstance(step["message"], list) else []
        for part in parts:
            if part["type"] == "image":
                assert (variant_dir / part["source"]["path"]).is_file()
    harbor_src = os.environ.get("HARBOR_SRC")
    if harbor_src:
        pkg = types.ModuleType("harbor")
        pkg.__path__ = [str(Path(harbor_src) / "harbor")]
        sys.modules.setdefault("harbor", pkg)
        from harbor.models.trajectories.trajectory import Trajectory

        Trajectory.model_validate(doc)


def test_result_json_has_harbor_trial_fields_and_atif(cli_env, tmp_path):  # noqa: F811
    out = tmp_path / "out"
    assert run_cb(["run", str(HELLO), "--output-dir", str(out)]) == 0
    variant_dir = out / "hello_file_env_v0"
    result = json.loads((variant_dir / "result.json").read_text())
    assert result["task_name"] == "hello_file_env" and result["trial_name"] == "hello_file_env_v0"
    assert result["verifier_result"] == {"rewards": {"reward": 1.0}}
    assert result["exception_info"] is None
    assert result["agent_info"]["name"] == "oracle"
    for phase in ("environment_setup", "agent_execution", "verifier"):
        assert result[phase]["started_at"] <= result[phase]["finished_at"], phase
    assert result["started_at"] <= result["finished_at"]
    # cua-bench's own keys are untouched.
    assert (result["task"], result["variant"], result["reward"]) == ("hello_file_env", 0, 1.0)

    if os.environ.get("HARBOR_SRC"):
        _check_atif(
            {
                "schema_version": "ATIF-v1.8",
                "agent": {"name": "x", "version": "1"},
                "steps": [{"step_id": 1, "source": "user", "message": "m"}],
            },
            variant_dir,
        )
        try:  # Harbor's trial models need its own deps (toml, ...)
            from harbor.models.trial.result import AgentInfo, TimingInfo
            from harbor.models.verifier.result import VerifierResult
        except ModuleNotFoundError:
            AgentInfo = None
        if AgentInfo is not None:
            AgentInfo.model_validate(result["agent_info"])
            VerifierResult.model_validate(result["verifier_result"])
            for phase in ("environment_setup", "agent_execution", "verifier"):
                TimingInfo.model_validate(result[phase])

    doc = json.loads((variant_dir / "trajectory.json").read_text())
    _check_atif(doc, variant_dir)
    sources = [s["source"] for s in doc["steps"]]
    assert sources[0] == "user" and "agent" in sources and sources[-1] == "system"
    assert doc["final_metrics"]["extra"]["reward"] == 1.0


def test_exception_info_on_failure(cli_env, tmp_path, monkeypatch):  # noqa: F811
    boom = FakeSDK()

    class Broken(boom.Sandbox):
        @classmethod
        def ephemeral(cls, image=None, **kwargs):
            raise RuntimeError("no capacity")

    monkeypatch.setattr(sandboxes, "_sdk", lambda: (boom.Image, Broken))
    out = tmp_path / "out"
    assert run_cb(["run", str(HELLO), "--output-dir", str(out)]) == 1
    result = json.loads((out / "hello_file_env_v0" / "result.json").read_text())
    assert result["status"] == "failed" and result["verifier_result"] is None
    assert result["exception_info"]["exception_type"] == "RuntimeError"
    assert "no capacity" in result["exception_info"]["exception_message"]


def test_attempts_layout_and_pass_at_k(cli_env, tmp_path):  # noqa: F811
    out = tmp_path / "out"
    assert run_cb(["run", str(HELLO), "--attempts", "3", "--output-dir", str(out)]) == 0
    names = sorted(p.name for p in out.iterdir() if p.is_dir())
    assert names == ["hello_file_env_v0", "hello_file_env_v0_a1", "hello_file_env_v0_a2"]
    summary = json.loads((out / "summary.json").read_text())
    assert summary["n_total_trials"] == 3 and summary["stats"]["n_completed"] == 3
    assert summary["pass_at_k"] == {"1": 1.0, "2": 1.0}
    assert sorted(r["attempt"] for r in summary["results"]) == [0, 1, 2]


def test_pass_at_k_math():
    def r(task, variant, reward):
        return SimpleNamespace(task=task, variant=variant, reward=reward)

    results = [r("a", 0, 1.0), r("a", 0, 0.0), r("a", 0, 0.0), r("a", 0, 0.0)]
    got = pass_at_k(results)
    assert got["1"] == pytest.approx(0.25) and got["2"] == pytest.approx(0.5)
    assert got["4"] == pytest.approx(1.0)
    assert pass_at_k([r("a", 0, 1.0)]) is None  # one attempt
    assert pass_at_k([r("a", 0, 0.5), r("a", 0, 1.0)]) is None  # not binary
    assert pass_at_k([r("a", 0, None), r("a", 0, 1.0)]) == {"1": 0.5, "2": 1.0}
    assert eligible_k(10) == [1, 2, 4, 5, 8, 10]


def test_retries_start_over_in_a_fresh_sandbox(tmp_path, monkeypatch):
    from cua_bench.runner import AgentOptions, BatchRunner, Job
    from cua_bench.targets import Target, resolve_env_spec

    sdk = FakeSDK()
    calls = {"n": 0}
    real = sdk.Sandbox.ephemeral

    class Flaky(sdk.Sandbox):
        @classmethod
        def ephemeral(cls, image=None, **kwargs):
            calls["n"] += 1
            if calls["n"] == 1:
                raise ConnectionError("sandbox did not become ready")
            return real(image, **kwargs)

    monkeypatch.setattr(sandboxes, "_sdk", lambda: (sdk.Image, Flaky))
    spec = resolve_env_spec({"provider": "native"}, Target())
    job = Job(HELLO, 0, spec, "s0", tmp_path / "v0")

    def run(retries):
        runner = BatchRunner(Target(), AgentOptions(), retries=retries, retry_backoff_s=0)
        return asyncio.run(runner.run([job]))[0]

    failed = run(0)
    assert failed.status == "failed" and failed.retries == 0
    calls["n"] = 0
    ok = run(1)
    assert ok.status == "completed" and ok.retries == 1 and ok.reward == 1.0
    log = (tmp_path / "v0" / "run.log").read_text()
    assert "Retrying hello_file_env v0 (1/1)" in log
    assert sdk.live == 0


def test_atif_maps_agent_steps():
    rows = [
        {
            "event_name": "reset",
            "data_json": "{}",
            "data_images": [],
            "timestamp": "2026-09-23T10:00:00.000Z",
        },
        {
            "event_name": "agent_step",
            "data_json": json.dumps(
                {
                    "model": "anthropic/claude",
                    "usage": {"prompt_tokens": 10, "completion_tokens": 2},
                    "output": [
                        {"type": "message", "content": [{"type": "output_text", "text": "hi"}]},
                        {"type": "computer_call", "call_id": "c1", "action": {"type": "click"}},
                    ],
                }
            ),
            "data_images": [],
            "timestamp": "2026-09-23T10:00:01.000Z",
        },
        {"event_name": "evaluate", "data_json": '{"result": [1.0]}', "timestamp": "not-a-time"},
    ]
    doc = build_atif(
        rows,
        Path("/nonexistent"),
        session_id="s",
        agent_name="cua-agent",
        model_name="anthropic/claude",
        description="do it",
        reward=1.0,
    )
    agent = doc["steps"][1]
    assert agent["source"] == "agent" and agent["message"] == "hi"
    assert agent["tool_calls"][0] == {
        "tool_call_id": "c1",
        "function_name": "computer",
        "arguments": {"type": "click"},
    }
    assert doc["final_metrics"]["total_prompt_tokens"] == 10
    assert doc["steps"][2]["timestamp"] is None  # not ISO 8601: dropped
    _check_atif(doc, Path("/nonexistent"))


# ── Versioned registry ─────────────────────────────────────────────────────


def _git(*args, cwd):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, timeout=60)


@pytest.fixture
def local_registry(tmp_path, monkeypatch):
    """A git repo with two dataset versions and a registry index pointing at it."""
    repo = tmp_path / "upstream"
    (repo / "datasets" / "mini" / "hello").mkdir(parents=True)
    (repo / "datasets" / "mini" / "hello" / "main.py").write_text((HELLO / "main.py").read_text())
    (repo / "other" / "big.bin").parent.mkdir(parents=True)
    (repo / "other" / "big.bin").write_text("not part of the dataset")
    _git("init", "-q", cwd=repo)
    _git("-c", "user.email=t@t", "-c", "user.name=t", "add", "-A", cwd=repo)
    _git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "v1", cwd=repo)
    v1 = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True
    ).stdout.strip()
    (repo / "datasets" / "mini" / "second").mkdir()
    (repo / "datasets" / "mini" / "second" / "main.py").write_text((HELLO / "main.py").read_text())
    _git("-c", "user.email=t@t", "-c", "user.name=t", "add", "-A", cwd=repo)
    _git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "v2", cwd=repo)
    v2 = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True
    ).stdout.strip()
    _git("config", "uploadpack.allowAnySHA1InWant", "true", cwd=repo)
    url = repo.as_uri()
    index = tmp_path / "registry.json"
    index.write_text(
        json.dumps(
            {
                "datasets": [
                    {
                        "name": "mini",
                        "version": "1.0",
                        "git_url": url,
                        "git_commit_id": v1,
                        "path": "datasets/mini",
                    },
                    {
                        "name": "mini",
                        "version": "1.1",
                        "git_url": url,
                        "git_commit_id": v2,
                        "path": "datasets/mini",
                    },
                    {
                        "name": "desk",
                        "version": "1.0",
                        "git_url": url,
                        "git_commit_id": v1,
                        "path": "datasets/mini",
                        "image": "BENCH_WEB",
                    },
                    {
                        "name": "harbor-mini",
                        "version": "2.0",
                        "description": "harbor shape",
                        "tasks": [
                            {
                                "name": "hello",
                                "git_url": url,
                                "git_commit_id": v1,
                                "path": "datasets/mini/hello",
                            }
                        ],
                    },
                ]
            }
        )
    )
    monkeypatch.setenv("CUA_BENCH_REGISTRY", str(index))
    monkeypatch.setenv("CUA_BENCH_REGISTRY_CACHE", str(tmp_path / "cache"))
    monkeypatch.delenv("CUA_REGISTRY_HOME", raising=False)
    return SimpleNamespace(repo=repo, v1=v1, v2=v2)


def test_registry_pins_versions_and_latest(local_registry):
    v1 = registry.resolve("mini@1.0")
    assert sorted(p.name for p in v1.iterdir()) == ["hello"]
    latest = registry.resolve("mini")
    assert sorted(p.name for p in latest.iterdir()) == ["hello", "second"]
    # Sparse: only the dataset path was checked out.
    assert not (latest.parents[1] / "other").exists()
    # Pinned and cached: a second resolve does no git work.
    (local_registry.repo / "datasets").rename(local_registry.repo / "moved")
    assert registry.resolve("mini@1.0") == v1
    with pytest.raises(registry.RegistryError, match="not in the registry"):
        registry.resolve("mini@9.9")


def test_registry_reads_harbor_entries(local_registry):
    tasks = registry.resolve("harbor-mini@2.0")
    assert (tasks / "hello" / "main.py").is_file()
    entries = {e["ref"]: e for e in registry.list_entries()}
    assert entries["harbor-mini@2.0"]["format"] == "harbor"
    assert entries["mini@1.1"]["format"] == "cua-bench"


def test_cb_run_dataset_by_name_at_version(cli_env, local_registry, tmp_path, capsys):  # noqa: F811
    out = tmp_path / "out"
    assert run_cb(["run", "mini@1.0", "--output-dir", str(out)]) == 0
    summary = json.loads((out / "summary.json").read_text())
    assert [r["task"] for r in summary["results"]] == ["hello", "hello"]
    assert run_cb(["dataset", "list"]) == 0
    listing = capsys.readouterr().out
    assert "mini@1.0" in listing and "mini@1.1" in listing and "harbor-mini@2.0" in listing


def test_registry_entry_image_is_the_default_desktop(cli_env, local_registry, capsys):  # noqa: F811
    """A registry dataset's ``image`` runs its tasks that name none (hello names none)."""
    from cua_bench.images import BENCH_WEB

    path, entry = registry.resolve_entry("desk")
    assert (path / "hello" / "main.py").is_file()
    assert entry.image == "BENCH_WEB" and entry.default_image == BENCH_WEB
    assert registry.resolve_entry("mini@1.0")[1].default_image is None

    assert run_cb(["run", "desk", "--dry-run"]) == 0
    out = capsys.readouterr().out
    assert f"hello v0: {BENCH_WEB} -> container" in out
    # --image still wins over the registry default.
    assert run_cb(["run", "desk", "--dry-run", "--image", "ghcr.io/example/desk:1"]) == 0
    assert "hello v0: ghcr.io/example/desk:1 -> container" in capsys.readouterr().out
    # A dataset without an image keeps the SDK's default Linux image.
    assert run_cb(["run", "mini@1.0", "--dry-run"]) == 0
    assert BENCH_WEB not in capsys.readouterr().out


def test_unknown_dataset_points_at_the_registry(cli_env, local_registry, capsys):  # noqa: F811
    assert run_cb(["run", "dataset", "no-such-set"]) == 1
    out = capsys.readouterr().out
    assert "not in the registry" in out and registry.REGISTRY_URL in out
    assert run_cb(["dataset", "list"]) == 0
    assert registry.REGISTRY_URL in capsys.readouterr().out


def test_bundled_index_is_pinned():
    entries = registry.load_index(str(registry.BUNDLED_INDEX))
    assert {e.name for e in entries} >= {"cua-bench-basic", "cua-bench-kicad"}
    for entry in entries:
        assert len(entry.git_commit_id or "") == 40 and entry.path, entry
    # cua-bench-basic's pinned tasks name no image; their pywebview windows
    # need the bench-web desktop (bench-ui).
    basic = registry.find_entry("cua-bench-basic", entries)
    assert basic.image == "BENCH_WEB"
