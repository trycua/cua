"""Hermetic tests for the unified runner: SDK adapter and batch execution.

No sandbox, VM, container, network or keychain: cua_sandbox is replaced by a
fake whose ``Sandbox.ephemeral`` records every sandbox it opens.
"""

import asyncio
import json
import sys
import textwrap
from contextlib import asynccontextmanager
from types import SimpleNamespace

import pytest
from cua_bench import sandboxes
from cua_bench.cli.main import normalize_argv
from cua_bench.runner import AgentOptions, BatchRunner, Job
from cua_bench.targets import EnvSpec, Target

IMG = "registry.example/desktop:docker-1"


# ── Fake SDK ────────────────────────────────────────────────────────────────


class FakeFiles:
    def __init__(self):
        self.data = {}

    async def write_text(self, path, content):
        self.data[path] = content

    async def read_text(self, path):
        return self.data[path]

    async def exists(self, path):
        return path in self.data


class FakeShell:
    def __init__(self, files):
        self.files = files

    async def run(self, command, timeout=None, background=False):
        return SimpleNamespace(returncode=0, stdout="", stderr="")


class FakeSandbox:
    def __init__(self, name, pool):
        self.name = name
        self.pool_name = pool
        self.files = FakeFiles()
        self.shell = FakeShell(self.files)

    async def screenshot(self, *a, **k):
        return b"PNG"

    async def disconnect(self):
        pass


class FakeImage:
    def __init__(self, ref=None, os_type="linux", kind=None, builtin=None):
        self.ref, self.os_type, self.kind, self.builtin = ref, os_type, kind, builtin

    @classmethod
    def from_registry(cls, ref, *, os_type="linux", kind=None):
        return cls(ref, os_type, kind)

    @classmethod
    def linux(cls, kind="vm"):
        return cls(None, "linux", kind, "linux")

    @classmethod
    def windows(cls, kind="vm"):
        return cls(None, "windows", kind, "windows")

    macos = android = linux


class FakeSDK:
    """Records every sandbox it opens and how many are live at once."""

    def __init__(self, delay=0.02):
        self.calls = []
        self.live = 0
        self.peak = 0
        self.released = 0
        self.delay = delay
        sdk = self

        class Sandbox:
            @classmethod
            @asynccontextmanager
            async def ephemeral(
                cls,
                image=None,
                *,
                on=None,
                local=False,
                cpu=None,
                memory_mb=None,
                time_to_start=None,
                telemetry_enabled=True,
            ):
                sdk.calls.append(
                    dict(image=image, on=on, local=local, cpu=cpu, memory_mb=memory_mb)
                )
                sdk.live += 1
                sdk.peak = max(sdk.peak, sdk.live)
                try:
                    await asyncio.sleep(sdk.delay)
                    yield FakeSandbox(f"sandbox-{len(sdk.calls)}", None)
                finally:
                    sdk.live -= 1
                    sdk.released += 1

        self.Sandbox = Sandbox


@pytest.fixture
def fake_sdk(monkeypatch):
    sdk = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", lambda: (FakeImage, sdk.Sandbox))
    return sdk


# ── Adapter ─────────────────────────────────────────────────────────────────


class TestAdapter:
    def test_build_image_same_ref_everywhere(self):
        spec = EnvSpec(provider="native", kind="container", image=IMG)
        image = sandboxes.build_image(spec, FakeImage)
        assert (image.ref, image.kind, image.os_type) == (IMG, "container", "linux")
        vm = sandboxes.build_image(
            EnvSpec(provider="native", os_type="windows", kind="vm"), FakeImage
        )
        assert (vm.builtin, vm.kind) == ("windows", "vm")

    def test_kwargs_carry_the_sizing_and_the_location(self):
        spec = EnvSpec(provider="native", image=IMG)
        kw = sandboxes.ephemeral_kwargs(spec, Target(on="local", cpu=2, memory_mb=2048, warm=True))
        # The SDK sizes local containers and VMs from cpu/memory_mb itself.
        assert kw["on"] == "local" and kw["memory_mb"] == 2048 and kw["cpu"] == 2
        for pool_option in ("max_pool_size", "warm", "claim_ttl", "progress"):
            assert pool_option not in kw

    def test_explain_pull_error_names_the_registry(self):
        spec = EnvSpec(provider="native", image=IMG)
        err = RuntimeError("start failed: ErrImagePull manifest unknown")
        local = sandboxes.explain_error(err, spec, Target(on="local"))
        assert "not available locally" in local and "docker build" in local
        assert "registry.example" in local

    def test_other_errors_pass_through(self):
        err = RuntimeError("401 Unauthorized")
        assert sandboxes.explain_error(err, None, Target(on="local")) == (
            "RuntimeError: 401 Unauthorized"
        )

    def test_cloud_targets_and_pool_images_end_with_cua_cloud(self):
        from cua_bench.targets import CloudEnded

        async def go(spec, target):
            async with sandboxes.open_sandbox(spec, target):
                pass

        spec = EnvSpec(provider="native", image=IMG)
        with pytest.raises(CloudEnded, match="Cua Cloud has ended"):
            asyncio.run(go(spec, Target(on="cloud")))
        with pytest.raises(CloudEnded):
            asyncio.run(go(EnvSpec(provider="native", pool="bench-p"), Target(on="local")))


# ── Batch runner ────────────────────────────────────────────────────────────

TASK = textwrap.dedent(
    """
    import cua_bench as cb

    @cb.tasks_config(split="train")
    def load():
        return [
            cb.Task(description=f"write {i}", metadata={"i": i},
                    computer={"provider": "native", "setup_config": {"os_type": "linux",
                              "image": "%s"}})
            for i in range(3)
        ]

    @cb.setup_task(split="train")
    async def setup(task_cfg, session):
        await session.write_file("/tmp/goal", str(task_cfg.metadata["i"]))

    @cb.solve_task(split="train")
    async def solve(task_cfg, session):
        await session.write_file("/tmp/answer", await session.read_file("/tmp/goal"))

    @cb.evaluate_task(split="train")
    async def evaluate(task_cfg, session):
        ok = await session.read_file("/tmp/answer") == str(task_cfg.metadata["i"])
        if task_cfg.metadata["i"] == 2:
            raise RuntimeError("boom in evaluate")
        return [1.0 if ok else 0.0]
    """
    % IMG
)


@pytest.fixture
def task_dir(tmp_path):
    path = tmp_path / "write_env"
    path.mkdir()
    (path / "main.py").write_text(TASK)
    return path


def _jobs(task_dir, out, n=3):
    spec = EnvSpec(provider="native", os_type="linux", kind="container", image=IMG)
    return [
        Job(
            task_path=task_dir,
            variant=i,
            spec=spec,
            session_id=f"s{i}",
            output_dir=out / f"write_env_v{i}",
        )
        for i in range(n)
    ]


class Recorder:
    def __init__(self):
        self.events = []

    def plan(self, target, jobs, plans):
        self.events.append(("plan", [(p.tasks, p.max_pool_size) for p in plans]))

    def started(self, job, index, total):
        self.events.append(("started", job.session_id))

    def progress(self, job, event):
        self.events.append(("progress", job.session_id, event.stage))

    def finished(self, result, index, total):
        self.events.append(("finished", result.session_id, result.status))


class TestBatchRunner:
    def test_local_batch_opens_one_sandbox_per_variant_up_to_the_concurrency(
        self, fake_sdk, task_dir, tmp_path
    ):
        statuses = []
        recorder = Recorder()
        target = Target(on="local", concurrency=2)
        runner = BatchRunner(
            target,
            AgentOptions(oracle=True),
            reporter=recorder,
            on_status=lambda job, s, f: statuses.append((job.session_id, s)),
        )
        results = asyncio.run(runner.run(_jobs(task_dir, tmp_path / "out")))

        assert len(fake_sdk.calls) == 3  # one sandbox per variant
        assert all(c["on"] == "local" for c in fake_sdk.calls)
        assert {c["image"].ref for c in fake_sdk.calls} == {IMG}
        assert fake_sdk.peak <= 2  # never more sandboxes than concurrency
        assert fake_sdk.released == 3  # every sandbox released
        assert ("plan", [(3, 2)]) in recorder.events

        by_variant = {r.variant: r for r in results}
        assert by_variant[0].status == by_variant[1].status == "completed"
        assert by_variant[0].reward == 1.0 and by_variant[0].pool is None
        assert by_variant[2].status == "failed" and "boom in evaluate" in by_variant[2].error

        v0 = tmp_path / "out" / "write_env_v0"
        log = (v0 / "run.log").read_text()
        assert "✓ Evaluation result: [1.0]" in log and "completed successfully" in log
        saved = json.loads((v0 / "result.json").read_text())
        assert (saved["status"], saved["reward"], saved["on"], saved["backend"]) == (
            "completed",
            1.0,
            "local",
            "local-gvisor",
        )
        assert ("s0", "running") in statuses and ("s2", "failed") in statuses

    def test_local_batch_uses_local_sandboxes(self, fake_sdk, task_dir, tmp_path):
        runner = BatchRunner(
            Target(on="local", concurrency=3, memory_mb=2048), AgentOptions(oracle=True)
        )
        results = asyncio.run(runner.run(_jobs(task_dir, tmp_path / "out", n=2)))
        assert [r.status for r in results] == ["completed", "completed"]
        assert all(c["on"] == "local" and c["memory_mb"] == 2048 for c in fake_sdk.calls)
        assert results[0].backend == "local-gvisor"

    def test_cancel_releases_every_sandbox(self, fake_sdk, task_dir, tmp_path):
        fake_sdk.delay = 30  # hold the sandboxes until cancelled
        runner = BatchRunner(Target(on="local", concurrency=3), AgentOptions(oracle=True))

        async def go():
            task = asyncio.create_task(runner.run(_jobs(task_dir, tmp_path / "out")))
            for _ in range(200):  # bounded wait for all three sandboxes
                if fake_sdk.live == 3:
                    break
                await asyncio.sleep(0.01)
            assert fake_sdk.live == 3
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task

        asyncio.run(go())
        assert fake_sdk.live == 0 and fake_sdk.released == 3
        saved = json.loads((tmp_path / "out" / "write_env_v0" / "result.json").read_text())
        assert saved["status"] == "cancelled"

    def test_output_is_routed_per_task(self, fake_sdk, task_dir, tmp_path, capsys):
        from cua_bench.runner.output import routed_stdio

        async def go():
            with routed_stdio():
                runner = BatchRunner(Target(on="local", concurrency=2), AgentOptions())
                return await runner.run(_jobs(task_dir, tmp_path / "out", n=2))

        asyncio.run(go())
        for i in range(2):
            log = (tmp_path / "out" / f"write_env_v{i}" / "run.log").read_text()
            assert f"Running task {i}: write {i}" in log
            assert f"Running task {1 - i}:" not in log
        assert "Running task" not in capsys.readouterr().out


def test_screenshot_is_bounded_and_skipped_after_failure(monkeypatch):
    from cua_bench.runner import episode

    monkeypatch.setattr(episode, "SCREENSHOT_TIMEOUT_S", 0.05)
    calls = []

    class Hangs:
        async def screenshot(self):
            calls.append(1)
            await asyncio.sleep(30)

    session = Hangs()
    assert asyncio.run(episode._screenshot(session)) is None
    assert asyncio.run(episode._screenshot(session)) is None
    assert len(calls) == 1  # no second wait on a sandbox without a screen


# ── CLI ─────────────────────────────────────────────────────────────────────


def test_normalize_argv_detects_task_or_dataset(task_dir):
    assert normalize_argv(["run", str(task_dir), "--on", "cloud"])[:2] == ["run", "task"]
    assert normalize_argv(["run", str(task_dir.parent)])[:2] == ["run", "dataset"]
    assert normalize_argv(["run", "list"]) == ["run", "list"]
    assert normalize_argv(["run", "task", "x"]) == ["run", "task", "x"]


def test_build_jobs_resolves_specs(task_dir, tmp_path, monkeypatch):
    from cua_bench.cli.commands import run as run_cmd

    args = SimpleNamespace(dataset_path=str(task_dir.parent), task_filter=None, max_variants=2)
    jobs = run_cmd.build_jobs(args, "r1", tmp_path / "out", Target(on="cloud"), "dataset")
    assert [(j.variant, j.spec.image, j.spec.backend("cloud")) for j in jobs] == [
        (0, IMG, "cloud-gvisor"),
        (1, IMG, "cloud-gvisor"),
    ]
    assert jobs[0].session_id == "task-r1-write_env-v0"


def test_session_status_without_docker(tmp_path):
    from cua_bench.sessions.status import session_status

    out = tmp_path / "v0"
    out.mkdir()
    (out / "result.json").write_text(json.dumps({"status": "completed", "reward": 0.5}))
    crashed = {"session_id": "a", "status": "running", "pid": 2**22 + 7, "output_dir": str(out)}
    assert session_status(crashed) == {"session_id": "a", "status": "completed", "reward": 0.5}
    lost = {"session_id": "b", "status": "running", "pid": 2**22 + 7, "output_dir": "/nope"}
    assert session_status(lost)["status"] == "failed"
    assert session_status({"session_id": "c", "status": "queued"})["status"] == "queued"


if sys.platform == "win32":  # pragma: no cover
    pytestmark = pytest.mark.skip("fcntl-based run bookkeeping")
