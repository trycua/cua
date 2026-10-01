"""BenchAdapter base class and the WAA adapter built on it (hermetic)."""

from __future__ import annotations

import asyncio
import importlib.util
import sys
import types
from pathlib import Path

import cua_bench as cb
import pytest
from cua_bench import sandboxes
from cua_bench.adapters import BenchAdapter, Endpoints, ServerSpec, unmet_requirements
from cua_bench.environment import Environment
from cua_bench.targets import Target, TargetError, check_requirements, resolve_env_spec

from .fakes import FakeSDK, FakeSandbox

PKG = Path(__file__).resolve().parents[2]


class Toy(BenchAdapter):
    id, version = "toy", "1.0"
    image = "ghcr.io/trycua/bench-toy@sha256:" + "0" * 64
    requires = frozenset({"openai"})
    server = ServerSpec(port=8877, health="/healthz")
    timeouts = {"setup": 5.0, "evaluate": 0.2, "oracle": 5.0}

    def load_tasks(self, split):
        return [cb.Task(description=f"t{i}", metadata={"i": i}) for i in range(2)]

    async def setup(self, task, session, ep):
        await session.write_file("/tmp/i", str(task.metadata["i"]))

    async def evaluate(self, task, session, ep):
        if task.metadata["i"] == 1:
            await asyncio.sleep(5)  # longer than the evaluate timeout
        return [1.0 if await session.read_file("/tmp/i") == "0" else 0.0]

    async def oracle(self, task, session, ep):
        await session.write_file("/tmp/solved", "yes")


def test_register_defines_the_task_functions():
    ns: dict = {}
    adapter = Toy().register(ns)
    assert set(ns) >= {"load", "start", "evaluate", "solve", "ADAPTER"}
    assert ns["ADAPTER"] is adapter
    env = Environment.make_from_module(types.SimpleNamespace(**ns), env_path="toy")
    assert env.tasks_config_fn and env.setup_task_fn and env.evaluate_task_fn and env.solve_task_fn
    tasks = ns["load"]()
    setup = tasks[0].computer["setup_config"]
    assert tasks[0].computer["provider"] == "native"
    assert setup["image"] == Toy.image and setup["server_port"] == 8877
    assert setup["kinds"] == ["container", "vm"] and setup["requires"] == ["openai"]
    assert tasks[0].metadata["benchmark"] == {"id": "toy", "version": "1.0"}
    assert tasks[1].task_id == "toy-1"


def test_no_oracle_means_no_solve_function():
    class NoOracle(Toy):
        oracle = None

    ns: dict = {}
    NoOracle().register(ns)
    assert "solve" not in ns


def test_timeouts_bound_each_phase():
    ns: dict = {}
    Toy().register(ns)
    sb = FakeSandbox()
    from cua_bench.computers.remote import RemoteDesktopSession

    session = RemoteDesktopSession.attach(sb)
    t0, t1 = ns["load"]()

    async def go():
        await ns["start"](t0, session)
        assert await ns["evaluate"](t0, session) == [1.0]
        await ns["solve"](t0, session)
        with pytest.raises(asyncio.TimeoutError):
            await ns["evaluate"](t1, session)

    asyncio.run(go())
    assert sb.files.data["/tmp/solved"] == b"yes"


def test_requirements_fail_before_any_claim():
    assert unmet_requirements(["openai"], cloud=False, environ={}, has_kvm=True) == [
        "needs OPENAI_API_KEY in the environment"
    ]
    assert unmet_requirements(["kvm"], cloud=True, environ={}, has_kvm=False) == []
    assert "KVM" in unmet_requirements(["kvm"], cloud=False, environ={}, has_kvm=False)[0]
    spec = resolve_env_spec(Toy().environment(), Target())
    assert spec.requires == ("openai",)
    with pytest.raises(TargetError, match="OPENAI_API_KEY"):
        check_requirements([spec], Target(), environ={}, has_kvm=True)
    check_requirements([spec], Target(), environ={"OPENAI_API_KEY": "k"}, has_kvm=True)


def test_endpoints_resolve_ports_locally_and_by_url():
    class Svc:
        def __init__(self, name):
            self.name = name

        async def url(self):
            return f"https://svc/{self.name}"

        async def request(self, method, path, **kw):
            return (self.name, method, path)

    sb = FakeSandbox()
    sb.exposed_ports = {9222: 40123}
    sb.service = Svc
    from cua_bench.computers.remote import RemoteDesktopSession

    ep = Endpoints(RemoteDesktopSession.attach(sb), ServerSpec(port=8877))

    async def go():
        assert await ep.url() == "https://svc/server"
        assert await ep.url(8877) == "https://svc/server"
        assert await ep.url(9222) == "http://127.0.0.1:40123"
        assert await ep.url(5555) == "https://svc/port-5555"
        assert await ep.request("server", "POST", "/init") == ("server", "POST", "/init")
        legacy = Endpoints(RemoteDesktopSession(api_url="http://10.0.0.9:5000"), ServerSpec(8877))
        assert await legacy.url() == "http://10.0.0.9:8877"

    asyncio.run(go())


# ── WAA ─────────────────────────────────────────────────────────────────────


@pytest.fixture
def waa(monkeypatch):
    """The WAA task module, with its heavy evaluator deps stubbed."""
    calls = {"setup": [], "evaluate": []}

    class Controller:
        def __init__(self, session):
            self.session = session

        async def setup(self, config):
            calls["setup"].append(config)

    class Evaluator:
        def __init__(self, session):
            self.session = session

        async def evaluate(self, config):
            calls["evaluate"].append(config)
            return 1.0

    for name, attrs in (
        ("winarena_adapter.setup_controller", {"WAASetupController": Controller}),
        ("winarena_adapter.evaluator", {"WAAEvaluator": Evaluator}),
    ):
        module = types.ModuleType(name)
        module.__dict__.update(attrs)
        monkeypatch.setitem(sys.modules, name, module)
    monkeypatch.syspath_prepend(str(PKG / "tasks"))
    monkeypatch.delenv("CUA_BENCH_WAA_IMAGE", raising=False)
    spec = importlib.util.spec_from_file_location(
        "waa_main", PKG / "tasks/winarena_adapter/main.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module, calls


def test_waa_is_a_vm_only_bench_adapter(waa):
    module, _ = waa
    assert isinstance(module.ADAPTER, BenchAdapter)
    tasks = module.load()
    assert len(tasks) == 154
    spec = resolve_env_spec(tasks[0].computer, Target())
    assert (spec.os_type, spec.kind, spec.image) == ("windows", "vm", None)
    assert spec.requires == ("kvm",)
    with pytest.raises(TargetError, match="windows is VM-only"):
        resolve_env_spec(tasks[0].computer, Target(kind="container"))
    with pytest.raises(TargetError, match="KVM"):
        check_requirements([spec], Target(), environ={}, has_kvm=False)
    check_requirements([spec], Target(on="cloud"), environ={}, has_kvm=False)
    assert "solve" not in vars(module)  # WAA has no oracle


def test_waa_image_comes_from_the_environment(waa, monkeypatch):
    module, _ = waa
    monkeypatch.setenv("CUA_BENCH_WAA_IMAGE", "registry.example/waa-win11:1")
    spec = importlib.util.spec_from_file_location(
        "waa_main2", PKG / "tasks/winarena_adapter/main.py"
    )
    fresh = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fresh)
    env = resolve_env_spec(fresh.load()[0].computer, Target(on="cloud"))
    assert env.image == "registry.example/waa-win11:1" and env.backend("cloud") == "cloud-kubevirt"


def test_waa_runs_through_cb_run_on_fake_sandboxes(waa, monkeypatch, tmp_path):
    from cua_bench.runner import AgentOptions, BatchRunner, Job

    module, calls = waa
    sdk = FakeSDK()
    monkeypatch.setattr(sandboxes, "_sdk", sdk.pair)
    task = module.load()[0]
    spec = resolve_env_spec(task.computer, Target())
    runner = BatchRunner(Target(), AgentOptions(), env_factory=lambda _p: _env(module))
    (result,) = asyncio.run(
        runner.run([Job(PKG / "tasks/winarena_adapter", 0, spec, "s", tmp_path / "v0")])
    )
    assert result.status == "completed" and result.reward == 1.0
    assert calls["evaluate"] and sdk.calls[0]["image"].builtin == "windows"
    assert sdk.calls[0]["image"].kind == "vm"


def _env(module):
    return Environment.make_from_module(module, env_path="winarena_adapter")
