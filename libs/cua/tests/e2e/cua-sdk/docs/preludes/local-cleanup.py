# Hidden docs prelude `local-cleanup`: a guide that keeps a local sandbox
# (`Sandbox.create(..., local=True)` without deleting it) must not leave
# containers or VMs behind. Only sandboxes this block created are deleted:
# `Sandbox.create` and `Sandbox._create` (behind the legacy `sandbox()`
# helper) are wrapped to record each one (`Sandbox.ephemeral` deletes its
# own), and the exit hook deletes exactly those. It never lists and deletes: a reader's
# (or another run's) sandboxes are never touched, whatever CUA_HOME says.
import asyncio as _cua_docs_asyncio
import atexit as _cua_docs_atexit

import cua_sandbox as _cua_docs_mod

_cua_docs_created: list = []


def _cua_docs_track(name: str):
    original = getattr(_cua_docs_mod.Sandbox, name)

    async def tracked(*args, **kwargs):
        sandbox = await original(*args, **kwargs)
        _cua_docs_created.append(sandbox.name)
        return sandbox

    setattr(_cua_docs_mod.Sandbox, name, staticmethod(tracked))


for _cua_docs_name in ("create", "_create"):
    if hasattr(_cua_docs_mod.Sandbox, _cua_docs_name):
        _cua_docs_track(_cua_docs_name)


def _cua_docs_cleanup() -> None:
    async def _go() -> None:
        for _name in list(dict.fromkeys(_cua_docs_created)):
            try:
                await _cua_docs_mod.Sandbox.delete(_name, local=True)
            except Exception as _e:  # noqa: BLE001 - best effort, reported
                if "not found" not in str(_e).lower():
                    print(f"docs cleanup: {_name}: {_e}")

    if _cua_docs_created:
        _cua_docs_asyncio.run(_go())


_cua_docs_atexit.register(_cua_docs_cleanup)
