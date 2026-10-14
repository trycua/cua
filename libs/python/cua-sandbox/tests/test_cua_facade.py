"""`from cua import Sandbox, Image` keeps working now that `cua` is the SDK.

The former `cua` meta-package (libs/python/cua, 0.1.x) was folded into the
cua SDK package (libs/cua/python, 0.2): with cua-sandbox installed (the
`sandbox` extra), the meta-package names resolve to cua-sandbox again.
"""

import cua_sandbox
from cua_sandbox.runtime import QEMURuntime

import cua


def test_meta_package_names_resolve_to_cua_sandbox():
    from cua import Image, Pool, Sandbox

    assert Sandbox is cua_sandbox.Sandbox
    assert Image is cua_sandbox.Image
    assert Pool is cua_sandbox.Pool
    assert cua.QEMURuntime is QEMURuntime
    assert cua.configure is cua_sandbox.configure


def test_the_sdk_handle_stays_reachable():
    assert cua.SandboxHandle is cua._native.Sandbox
    assert cua.Sandbox is not cua.SandboxHandle


def test_runtime_submodule_re_exports():
    from cua.runtime import DockerRuntime, LumeRuntime

    assert DockerRuntime is cua_sandbox.runtime.DockerRuntime
    assert LumeRuntime is cua_sandbox.runtime.LumeRuntime


def test_localhost_is_removed():
    # Local-machine control moved to cua-driver (plan 8.15).
    import cua_sandbox.agent
    import cua_sandbox.sync
    import cua_sandbox.transport

    for name in ("Localhost", "localhost"):
        assert not hasattr(cua_sandbox, name)
        assert name not in cua_sandbox.__all__
        assert not hasattr(cua, name)
    assert not hasattr(cua_sandbox.sync, "localhost")
    assert not hasattr(cua_sandbox.transport, "LocalTransport")
    assert not hasattr(cua_sandbox.agent, "LocalhostHandler")
