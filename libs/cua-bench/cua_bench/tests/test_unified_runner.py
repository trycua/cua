"""Hermetic tests for the unified runner: SDK adapter, cloud auth, batch execution.

No sandbox, VM, container, network or keychain: cua_sandbox is replaced by a
fake whose ``Sandbox.ephemeral`` has the managed-pool signature (``warm``,
``max_pool_size``, ``claim_ttl``, ``progress``) and records every claim.
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


class FakeFleetSDK:
    """Records claims; ``max_pool_size`` is the managed-pool contract."""

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
                warm=None,
                max_pool_size=None,
                claim_ttl=None,
                progress=None,
            ):
                sdk.calls.append(
                    dict(
                        image=image,
                        on=on,
                        local=local,
                        cpu=cpu,
                        memory_mb=memory_mb,
                        warm=warm,
                        max_pool_size=max_pool_size,
                        claim_ttl=claim_ttl,
                    )
                )
                pool = f"cua-auto-{abs(hash(image.ref)) % 10000}"
                if progress is not None:
                    progress(
                        SimpleNamespace(
                            stage="cold_start", pool=pool, message="scaling from zero", elapsed=0.0
                        )
                    )
                sdk.live += 1
                sdk.peak = max(sdk.peak, sdk.live)
                try:
                    await asyncio.sleep(sdk.delay)
                    if progress is not None:
                        progress(
                            SimpleNamespace(stage="ready", pool=pool, message="ready", elapsed=1.0)
                        )
                    yield FakeSandbox(f"claim-{len(sdk.calls)}", pool)
                finally:
                    sdk.live -= 1
                    sdk.released += 1

        self.Sandbox = Sandbox


@pytest.fixture
def fake_sdk(monkeypatch):
    sdk = FakeFleetSDK()
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

    def test_kwargs_local_carry_caps_but_never_pool_options(self):
        spec = EnvSpec(provider="native", image=IMG)
        kw = sandboxes.ephemeral_kwargs(
            spec,
            Target(on="local", cpu=2, memory_mb=2048, warm=True),
            max_pool_size=4,
        )
        # The SDK sizes local containers and VMs from cpu/memory_mb itself.
        assert kw["on"] == "local" and kw["memory_mb"] == 2048 and kw["cpu"] == 2
        assert "max_pool_size" not in kw and "warm" not in kw and "progress" not in kw

    def test_kwargs_cloud_managed(self):
        kw = sandboxes.ephemeral_kwargs(
            EnvSpec(provider="native", image=IMG),
            Target(on="cloud", warm=True, claim_ttl_s=600),
            max_pool_size=3,
            progress=print,
        )
        assert (kw["on"], kw["max_pool_size"], kw["warm"], kw["claim_ttl"]) == (
            "cloud",
            3,
            True,
            600,
        )
        assert kw["progress"] is print

    def test_explain_pull_error_cloud_and_local(self):
        spec = EnvSpec(provider="native", image=IMG)
        err = RuntimeError("claim failed: ErrImagePull manifest unknown")
        cloud = sandboxes.explain_error(err, spec, Target(on="cloud"))
        assert "Fleet could not pull" in cloud and "registry.example" in cloud
        local = sandboxes.explain_error(err, spec, Target(on="local"))
        assert "not available locally" in local and "docker build" in local

    def test_explain_auth_error(self):
        err = RuntimeError("401 Unauthorized")
        msg = sandboxes.explain_error(err, None, Target(on="cloud"))
        assert "cua auth login" in msg


# ── Cloud auth (resolved by the SDK) ────────────────────────────────────────


class TestCloudAuth:
    def test_source_comes_from_the_sdk(self):
        assert sandboxes.cloud_auth_source(lambda: "client credentials") == "client credentials"
        assert sandboxes.cloud_auth_source(lambda: "FLEETS_TOKEN") == "FLEETS_TOKEN"

    def test_not_authed_says_cua_auth_login(self):
        with pytest.raises(sandboxes.CloudAuthError, match="cua auth login"):
            sandboxes.cloud_auth_source(lambda: None)

    def test_cloud_run_stops_before_any_claim_without_credentials(
        self, fake_sdk, monkeypatch, tmp_path, capsys
    ):
        from cua_bench.cli import main as cli_main
        from cua_bench.sessions import manager

        monkeypatch.setattr(sandboxes, "cloud_auth_source", _raise_auth)
        monkeypatch.setattr(manager, "RUNS_FILE", tmp_path / "runs.json")
        monkeypatch.setenv("CUA_BENCH_NO_BANNER", "1")
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "false")
        task = tmp_path / "t"
        task.mkdir()
        (task / "main.py").write_text(TASK)
        with pytest.raises(SystemExit) as exit_info:
            cli_main.main(["run", str(task), "--on", "cloud", "--output-dir", str(tmp_path / "o")])
        assert exit_info.value.code == 1
        assert "cua auth login" in capsys.readouterr().out
        assert fake_sdk.calls == []


def _raise_auth(*_args):
    raise sandboxes.CloudAuthError(sandboxes.LOGIN_HINT)


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


def _jobs(task_dir, out, n=3, on="cloud"):
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
    def test_cloud_batch_claims_one_pool_sized_to_concurrency(self, fake_sdk, task_dir, tmp_path):
        statuses = []
        recorder = Recorder()
        target = Target(on="cloud", concurrency=2, warm=True, claim_ttl_s=300)
        runner = BatchRunner(
            target,
            AgentOptions(oracle=True),
            reporter=recorder,
            on_status=lambda job, s, f: statuses.append((job.session_id, s)),
        )
        results = asyncio.run(runner.run(_jobs(task_dir, tmp_path / "out")))

        assert len(fake_sdk.calls) == 3  # one claim per variant
        assert {c["max_pool_size"] for c in fake_sdk.calls} == {2}  # pool sized to the batch
        assert all(
            c["on"] == "cloud" and c["warm"] is True and c["claim_ttl"] == 300
            for c in fake_sdk.calls
        )
        assert {c["image"].ref for c in fake_sdk.calls} == {IMG}  # same pool key
        assert fake_sdk.peak <= 2  # never more claims than concurrency
        assert fake_sdk.released == 3  # every claim released
        assert ("plan", [(3, 2)]) in recorder.events

        by_variant = {r.variant: r for r in results}
        assert by_variant[0].status == by_variant[1].status == "completed"
        assert by_variant[0].reward == 1.0 and by_variant[0].pool.startswith("cua-auto-")
        assert by_variant[2].status == "failed" and "boom in evaluate" in by_variant[2].error

        v0 = tmp_path / "out" / "write_env_v0"
        log = (v0 / "run.log").read_text()
        assert "✓ Evaluation result: [1.0]" in log and "completed successfully" in log
        saved = json.loads((v0 / "result.json").read_text())
        assert (saved["status"], saved["reward"], saved["on"], saved["backend"]) == (
            "completed",
            1.0,
            "cloud",
            "cloud-gvisor",
        )
        assert ("s0", "running") in statuses and ("s2", "failed") in statuses

    def test_first_cloud_claim_per_pool_is_single_flight(self, fake_sdk, task_dir, tmp_path):
        order = []
        real = fake_sdk.Sandbox.ephemeral

        @asynccontextmanager
        async def ephemeral(image=None, **kw):
            progress = kw.get("progress")
            order.append(("enter", len(order)))
            if progress is not None:
                progress(
                    SimpleNamespace(stage="pool", pool=None, message="creating pool", elapsed=0.0)
                )
            await asyncio.sleep(0.05)  # pool creation in flight
            order.append(("pool-created", len(order)))
            async with real(image, **kw) as sb:
                yield sb

        fake_sdk.Sandbox.ephemeral = ephemeral
        runner = BatchRunner(Target(on="cloud", concurrency=3), AgentOptions(oracle=True))
        results = asyncio.run(runner.run(_jobs(task_dir, tmp_path / "out", n=3)))
        assert [r.status for r in results].count("completed") == 2  # v2 raises in evaluate
        # Nobody else entered before the first claim's pool existed.
        assert [kind for kind, _ in order[:2]] == ["enter", "pool-created"]
        assert fake_sdk.peak == 3  # the rest still claim in parallel

    def test_local_batch_uses_local_sandboxes(self, fake_sdk, task_dir, tmp_path):
        runner = BatchRunner(
            Target(on="local", concurrency=3, memory_mb=2048), AgentOptions(oracle=True)
        )
        results = asyncio.run(runner.run(_jobs(task_dir, tmp_path / "out", n=2)))
        assert [r.status for r in results] == ["completed", "completed"]
        assert all(
            c["on"] == "local" and c["max_pool_size"] is None and c["memory_mb"] == 2048
            for c in fake_sdk.calls
        )
        assert results[0].backend == "local-gvisor"

    def test_cancel_releases_every_claim(self, fake_sdk, task_dir, tmp_path):
        fake_sdk.delay = 30  # hold the claims until cancelled
        runner = BatchRunner(Target(on="cloud", concurrency=3), AgentOptions(oracle=True))

        async def go():
            task = asyncio.create_task(runner.run(_jobs(task_dir, tmp_path / "out")))
            for _ in range(200):  # bounded wait for all three claims
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


def test_pool_image_opens_a_claim_from_that_pool(monkeypatch):
    from cua_bench.targets import resolve_env_spec as resolve

    from .fakes import FakeSDK

    sdk = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", sdk.pair)
    spec = resolve({"provider": "native"}, Target(on="cloud", image="pool:bench-p"))

    async def go():
        async with sandboxes.open_sandbox(spec, Target(on="cloud")) as sb:
            return sb.id

    assert asyncio.run(go()) == "cloud:sb-1"
    (call,) = sdk.calls
    assert call["image"] is None and call["pool"] == "bench-p" and call["on"] == "cloud"
