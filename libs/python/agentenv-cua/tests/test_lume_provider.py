"""The lume provider against a fake lume: no Mac, no VM."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from agent_env.providers.sandbox_providers.sandbox_provider import SandboxNotFoundError
from agentenv_cua.lume import provider as provider_module
from agentenv_cua.lume.cli import Completed, Lume
from agentenv_cua.lume.provider import GUEST_BOOT_FILE, LumeSandboxProvider

provider_module._IP_POLL_S = 0


class FakeLume:
    """Just enough of lume's CLI: VMs by name, each with a status, an address and a config.json under ``storage``."""

    def __init__(self, storage: Path, vms: dict[str, str] | None = None) -> None:
        self.storage = storage
        self.vms: dict[str, dict] = {}
        self.calls: list[list[str]] = []
        self._next_ip = 10
        for name, status in (vms or {}).items():
            self._add(name, status)

    def _add(self, name: str, status: str = "stopped", mac: str | None = None) -> None:
        self.vms[name] = {"status": status, "ip": None}
        folder = self.storage / name
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "config.json").write_text(
            json.dumps({"macAddress": mac or f"ee:00:00:00:00:{len(self.vms):02x}"})
        )

    async def __call__(self, argv: list[str], *, timeout: float) -> Completed:
        self.calls.append(argv)
        cmd, args = argv[1], argv[2:]
        if cmd == "ls":
            return Completed(
                0,
                json.dumps(
                    [
                        {"name": n, "status": v["status"], "ipAddress": v["ip"]}
                        for n, v in self.vms.items()
                    ]
                ),
                "",
            )
        if cmd == "get":
            vm = self.vms.get(args[0])
            if vm is None:
                return Completed(1, "", f"Virtual machine not found: {args[0]}")
            return Completed(
                0,
                json.dumps([{"name": args[0], "status": vm["status"], "ipAddress": vm["ip"]}]),
                "",
            )
        if cmd == "clone":
            if args[0] not in self.vms:
                return Completed(1, "", f"Virtual machine not found: {args[0]}")
            self._add(args[1])
            return Completed(0, "", "")
        if cmd == "run":
            vm = self.vms[args[0]]
            mac = json.loads((self.storage / args[0] / "config.json").read_text())["macAddress"]
            vm["status"], vm["ip"] = "running", f"192.168.64.{int(mac.split(':')[-1], 16) + 100}"
            return Completed(0, "", "")
        if cmd == "stop":
            if args[0] in self.vms:
                self.vms[args[0]].update(status="stopped", ip=None)
            return Completed(0, "", "")
        if cmd == "delete":
            if self.vms.pop(args[0], None) is None:
                return Completed(1, "", f"Virtual machine not found: {args[0]}")
            return Completed(0, "", "")
        if cmd == "set":
            return Completed(0, "", "")
        if cmd == "ssh":
            return Completed(0, f"ran: {args[1]}\n", "")
        return Completed(2, "", f"unexpected {argv}")


@pytest.fixture
def lume(tmp_path):
    return FakeLume(tmp_path / "lume", {"bench-base": "stopped"})


@pytest.fixture
def provider(tmp_path, lume):
    return LumeSandboxProvider(
        base="bench-base",
        state_dir=str(tmp_path / "state"),
        lume_cli=Lume(storage=str(lume.storage), runner=lume),
    )


async def test_create_vm_clones_the_base_boots_it_with_its_settings_shared_and_reaches_the_driver_port(
    provider, lume
):
    sandbox = await provider.create_vm(exposed_ports=[18765], env={"AGENTENV_CONTROL_TOKEN": "t0k"})

    assert (
        sandbox.sandbox_id.startswith("agentenv-")
        and lume.vms[sandbox.sandbox_id]["status"] == "running"
    )
    assert sandbox.tunnel_urls == {18765: f"http://{sandbox.ip}:18765"}
    assert sandbox.type == "lume" and sandbox.platform == "darwin/arm64"
    run = next(call for call in lume.calls if call[1] == "run")
    share = Path(run[run.index("--shared-dir") + 1].removesuffix(":ro"))
    assert run[run.index("--shared-dir") + 1].endswith(":ro") and "--detach" in run
    boot = share / "agentenv" / "boot.json"
    assert json.loads(boot.read_text()) == {"env": {"AGENTENV_CONTROL_TOKEN": "t0k"}}
    assert boot.stat().st_mode & 0o077 == 0
    assert GUEST_BOOT_FILE.endswith("agentenv/boot.json")


async def test_a_third_running_vm_on_the_mac_is_refused(provider, lume):
    lume._add("someone-elses-vm", "running")
    await provider.create_vm(exposed_ports=[18765])

    with pytest.raises(RuntimeError, match="2 macOS VMs already run"):
        await provider.create_vm(exposed_ports=[18765])


def test_more_than_two_per_mac_cant_be_configured(tmp_path):
    with pytest.raises(ValueError, match="licence"):
        LumeSandboxProvider(base="b", max_running=3, state_dir=str(tmp_path))


async def test_reset_reclones_under_the_same_name_and_mac_so_the_url_stays(provider, lume):
    sandbox = await provider.create_vm(exposed_ports=[18765], env={"A": "1"})
    url = sandbox.tunnel_urls[18765]

    await provider.reset_vm(sandbox, image="", env={"A": "2"})

    again = await provider.get_sandbox(sandbox.sandbox_id)
    assert again.tunnel_urls[18765] == url
    assert [c[1] for c in lume.calls].count("clone") == 2
    share = provider._share_dir(sandbox.sandbox_id)
    assert json.loads((share / "agentenv" / "boot.json").read_text()) == {"env": {"A": "2"}}


async def test_terminate_deletes_only_what_it_made(provider, lume):
    sandbox = await provider.create_vm(exposed_ports=[18765])
    await sandbox.terminate()
    assert sandbox.sandbox_id not in lume.vms

    await provider.terminate_sandbox("bench-base")
    assert "bench-base" in lume.vms

    with pytest.raises(SandboxNotFoundError):
        await provider.get_sandbox(sandbox.sandbox_id)


async def test_a_failed_boot_leaves_no_vm_behind(provider, lume, monkeypatch):
    async def no_address(name):
        raise TimeoutError(f"{name} had no address")

    monkeypatch.setattr(provider, "_wait_for_ip", no_address)
    with pytest.raises(TimeoutError):
        await provider.create_vm(exposed_ports=[18765])
    assert set(lume.vms) == {"bench-base"}


async def test_a_registry_image_is_pulled_once_into_a_local_base(tmp_path, lume):
    pulled = []

    async def runner(argv, *, timeout):
        if argv[1] == "pull":
            pulled.append(argv)
            lume._add(argv[3])
            return Completed(0, "", "")
        return await lume(argv, timeout=timeout)

    provider = LumeSandboxProvider(
        image="ghcr.io/trycua/agentenv-macos-bench:0.1.0",
        state_dir=str(tmp_path / "s"),
        lume_cli=Lume(storage=str(lume.storage), runner=runner),
    )
    first = await provider.create_vm(exposed_ports=[18765])
    await first.terminate()
    await provider.create_vm(exposed_ports=[18765])

    assert len(pulled) == 1
    assert pulled[0][2:3] == ["agentenv-macos-bench:0.1.0"]
    assert pulled[0][pulled[0].index("--organization") + 1] == "trycua"


async def test_exec_runs_over_lumes_ssh_as_the_images_user(provider, lume):
    sandbox = await provider.create_vm(exposed_ports=[18765])
    code, out, _ = await sandbox.exec_with_output("echo", "hi there")

    assert code == 0 and out == "ran: echo 'hi there'\n"
    ssh = next(call for call in lume.calls if call[1] == "ssh")
    assert ssh[ssh.index("--user") + 1] == "lume"


async def test_restricted_egress_is_refused_not_approximated(provider):
    from agent_env.providers.sandbox_providers.sandbox import NetworkMode, NetworkPolicy

    with pytest.raises(ValueError, match="egress"):
        await provider.create_vm(
            exposed_ports=[18765],
            network_policy=NetworkPolicy(mode=NetworkMode.ALLOWLIST, allow_hosts=("x.test",)),
        )


def test_the_provider_registers_as_an_agentenv_plugin_class():
    from agent_env.providers.sandbox_providers.sandbox_provider import _unimplemented

    assert _unimplemented(LumeSandboxProvider) is None
    assert LumeSandboxProvider.native_platforms() == frozenset({"darwin/arm64"})
