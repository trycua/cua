"""Regression tests for the exposed_ports state bug shipped in 0.3.1.

#3130 passed exposed_ports= to sandbox_state.save(), which accepted no such
argument, so every NON-ephemeral local sandbox raised TypeError after the guest
had booted. Ephemeral sandboxes skip that branch entirely, which is exactly why
the released path looked healthy.
"""

import inspect


class TestExposedPortsSurviveAReconnect:
    def test_state_save_accepts_the_argument_the_runtime_passes(self):
        """The runtime's save() call named a parameter that did not exist, so a
        non-ephemeral local sandbox crashed after booting."""
        from cua_sandbox import sandbox_state

        params = inspect.signature(sandbox_state.save).parameters
        assert "exposed_ports" in params, "runtime passes exposed_ports= but save() rejects it"

        src = inspect.getsource(
            __import__("cua_sandbox.runtime.qemu", fromlist=["x"]).QEMUBaremetalRuntime.start
        )
        assert "sandbox_state.save(" in src
        assert "exposed_ports=exposed_ports" in src

    def test_every_kwarg_the_runtime_passes_is_accepted(self):
        """Guards the whole call, not just the one argument that broke."""
        from cua_sandbox import sandbox_state

        src = inspect.getsource(
            __import__("cua_sandbox.runtime.qemu", fromlist=["x"]).QEMUBaremetalRuntime.start
        )
        call = src.split("sandbox_state.save(", 1)[1].split(")", 1)[0]
        passed = {
            line.strip().split("=", 1)[0].strip()
            for line in call.splitlines()
            if "=" in line and not line.strip().startswith("#")
        }
        accepted = set(inspect.signature(sandbox_state.save).parameters)
        assert passed <= accepted, f"save() rejects: {sorted(passed - accepted)}"

    def test_save_round_trips_the_mapping(self, tmp_path, monkeypatch):
        from cua_sandbox import sandbox_state

        monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path)
        monkeypatch.setattr(sandbox_state, "_state_path", lambda n: tmp_path / f"{n}.json")
        sandbox_state.save(
            "demo",
            runtime_type="qemu-baremetal",
            image={},
            host="localhost",
            api_port=41000,
            exposed_ports={3000: 41041},
        )
        loaded = sandbox_state.load("demo")
        assert loaded is not None
        # JSON turns int keys into strings; the accessor is what restores them.
        assert {int(k): v for k, v in loaded["exposed_ports"].items()} == {3000: 41041}

    def test_public_accessor_exists_so_docs_need_no_private_attributes(self):
        """The guide had to print sb._runtime_info.exposed_ports[3000]."""
        from cua_sandbox.sandbox import Sandbox

        assert isinstance(getattr(Sandbox, "exposed_ports", None), property)


class TestTunnelErrorNamesEverySupportedTransport:
    """The message hardcoded a list that fell out of date: FleetCloudTransport
    implements forward_tunnel but was not named, so a local caller was told Fleet
    could not tunnel either — contradicting the documented Fleet path."""

    def test_message_lists_exactly_the_transports_that_implement_it(self):
        import importlib
        import inspect
        import pkgutil

        import cua_sandbox.transport as pkg
        from cua_sandbox.transport.base import Transport

        for mod in pkgutil.iter_modules(pkg.__path__):
            importlib.import_module(f"cua_sandbox.transport.{mod.name}")

        implementers = {
            cls.__name__ for cls in _subclasses(Transport) if "forward_tunnel" in vars(cls)
        }
        assert implementers, "no transport implements forward_tunnel"

        message = inspect.getsource(Transport.forward_tunnel)
        missing = sorted(n for n in implementers if n not in message)
        assert not missing, f"error message omits transports that support tunnelling: {missing}"


def _subclasses(cls):
    for sub in cls.__subclasses__():
        yield sub
        yield from _subclasses(sub)


class TestPersistentLocalSandboxesAreNotTreatedAsEphemeral:
    """`Sandbox.create` reached `runtime.start` without `ephemeral=`, and the
    runtime defaults it to True. A persistent local sandbox was therefore
    ephemeral to the runtime: no state written, and `_session_disk` set so the
    overlay is deleted on stop. It also made the bad `save(..., exposed_ports=)`
    call unreachable, which is why the TypeError never surfaced in practice."""

    def test_create_passes_ephemeral_through_to_the_runtime(self):
        import inspect

        from cua_sandbox.sandbox import Sandbox

        src = inspect.getsource(Sandbox._create)
        assert "runtime.start(image, sb_name, ephemeral=" in src, (
            "runtime.start must be told whether the sandbox is ephemeral; "
            "without it the runtime defaults to ephemeral=True"
        )

    def test_runtime_still_defaults_to_ephemeral_when_unspecified(self):
        """Callers that genuinely omit it keep the old behaviour."""
        import inspect

        from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime

        assert 'opts.pop("ephemeral", True)' in inspect.getsource(QEMUBaremetalRuntime.start)
