"""``Sandbox.image_info``: the image digest and variant a sandbox runs.

Hermetic: fake SDK handles and a fake ``native().resolve_image`` (conftest
switches real registry reads off); no network, VM or container.
"""

from __future__ import annotations

import dataclasses
import sys
from types import SimpleNamespace

import pytest
from cua_sandbox import Image, ImageInfo, Sandbox, _sdk
from cua_sandbox.image import resolve_image_kind
from cua_sandbox.runtime.base import RuntimeInfo
from cua_sandbox.transport.env import EnvTransport

DIGEST = "sha256:" + "a" * 64


def _native_info(**overrides) -> SimpleNamespace:
    fields = dict(
        reference="docker.io/library/python:3.12-slim",
        pinned_ref=f"docker.io/library/python@{DIGEST}",
        digest=DIGEST,
        variant="rootfs",
        arch="arm64",
        os="linux",
        emulated=False,
    )
    fields.update(overrides)
    return SimpleNamespace(**fields)


class _Handle:
    """A fake ``cua.Sandbox``: only ``image_info()``."""

    def __init__(self, answer=None, error: BaseException | None = None):
        self.answer = answer
        self.error = error
        self.calls = 0

    def image_info(self):
        self.calls += 1
        if self.error is not None:
            raise self.error
        return self.answer


def _sandbox(**kwargs) -> Sandbox:
    return Sandbox(EnvTransport(url="http://127.0.0.1:1"), name="sb", **kwargs)


WANT = ImageInfo(
    reference="docker.io/library/python:3.12-slim",
    pinned_ref=f"docker.io/library/python@{DIGEST}",
    digest=DIGEST,
    variant="rootfs",
    arch="arm64",
    os="linux",
    emulated=False,
)


def test_image_info_is_exported_and_frozen():
    import cua_sandbox

    assert cua_sandbox.ImageInfo is ImageInfo
    assert "ImageInfo" in cua_sandbox.__all__
    assert [f.name for f in dataclasses.fields(ImageInfo)] == [
        "reference",
        "pinned_ref",
        "digest",
        "variant",
        "arch",
        "os",
        "emulated",
    ]
    with pytest.raises(dataclasses.FrozenInstanceError):
        WANT.digest = "x"  # type: ignore[misc]


def test_the_native_handle_is_the_source_of_truth():
    handle = _Handle(_native_info())
    sb = _sandbox(_runtime_info=RuntimeInfo(host="127.0.0.1", api_port=1, native=handle))
    # A Python-side value never overrides the SDK's answer.
    sb._image_info_fallback = dataclasses.replace(WANT, digest="sha256:other")
    assert sb.image_info == WANT
    handle.answer = None
    assert sb.image_info is None


def test_fleet_claims_read_the_claim_or_transport_handle():
    sb = _sandbox()
    sb._claim_handle = SimpleNamespace(_native=_Handle(_native_info(variant="containerdisk")))
    assert sb.image_info.variant == "containerdisk"
    # After disconnect the claim handle lets go; the transport keeps one.
    sb._claim_handle = SimpleNamespace(_native=None)
    sb._transport._native_fleet_sandbox = _Handle(_native_info(arch=None, emulated=True))
    info = sb.image_info
    assert info.arch is None and info.emulated is True


def test_never_raises():
    sb = _sandbox(
        _runtime_info=RuntimeInfo(
            host="127.0.0.1", api_port=1, native=_Handle(error=RuntimeError("boom"))
        )
    )
    assert sb.image_info is None
    # An unreadable record is None too.
    sb._runtime_info.native = _Handle(SimpleNamespace(nonsense=True))
    assert sb.image_info is None


def test_direct_connections_report_none():
    sb = _sandbox()
    assert sb.image_info is None
    # A direct SDK handle answers None: nothing was resolved.
    sb._transport._native_sandbox = _Handle(None)
    assert sb.image_info is None


def test_python_side_runtimes_fall_back_to_the_create_time_resolution():
    sb = _sandbox(_runtime_info=RuntimeInfo(host="127.0.0.1", api_port=1))
    sb._image_info_fallback = WANT
    assert sb.image_info == WANT


@pytest.fixture
def resolver(monkeypatch):
    n = _sdk.native()
    calls: list[tuple] = []
    answers: dict[str, object] = {}

    def resolve_image(reference, backend, arch):
        calls.append((reference, backend, arch))
        return answers[reference]

    monkeypatch.setattr(n, "resolve_image", resolve_image)
    return SimpleNamespace(answers=answers, calls=calls)


def test_the_resolver_result_is_kept_on_the_image(resolver):
    resolver.answers["python:3.12-slim"] = _native_info()
    image = resolve_image_kind(Image.from_registry("python:3.12-slim"))
    assert image.kind == "container"
    assert image._resolved == WANT
    # Not part of the image's identity or its serialized form.
    assert image == Image.from_registry("python:3.12-slim")._with(kind="container")
    assert "_resolved" not in image.to_dict()
    # A resolver result without digest/arch/emulated (older SDKs) still maps.
    bare = SimpleNamespace(
        reference="r", pinned_ref=f"r@{DIGEST}", variant="containerdisk", os="linux"
    )
    info = ImageInfo._from_native(bare)
    assert (info.digest, info.arch, info.emulated) == (DIGEST, None, False)


async def test_create_records_the_resolution_once_for_python_side_runtimes(resolver, monkeypatch):
    resolver.answers["registry.example/app:1"] = _native_info(
        reference="registry.example/app:1", pinned_ref=f"registry.example/app@{DIGEST}"
    )

    class Runtime:
        server_port = None

        async def start(self, image, name, **opts):
            return RuntimeInfo(host="127.0.0.1", api_port=1, name=name)

    class Transport:
        async def connect(self):
            pass

    # `cua_sandbox.sandbox` the attribute is the `sandbox()` helper; patch the module.
    module = sys.modules["cua_sandbox.sandbox"]
    monkeypatch.setattr(module, "_env_transport", lambda *a, **k: Transport())
    monkeypatch.setattr(module, "_record_sandbox_create", lambda *a, **k: None)
    sb = await Sandbox.create(
        Image.from_registry("registry.example/app:1"), local=True, runtime=Runtime()
    )
    assert sb.image_info.pinned_ref == f"registry.example/app@{DIGEST}"
    assert sb.image_info.variant == "rootfs"
    # Read from what create resolved: no registry call per access.
    sb.image_info
    assert resolver.calls == [("registry.example/app:1", "local", None)]


def test_the_generated_sdk_record_maps():
    n = _sdk.native()
    assert callable(getattr(n.Sandbox, "image_info", None))
    record = n.ImageInfo(
        reference=WANT.reference,
        pinned_ref=WANT.pinned_ref,
        digest=WANT.digest,
        variant=WANT.variant,
        arch=WANT.arch,
        os=WANT.os,
        emulated=WANT.emulated,
    )
    assert ImageInfo._from_native(record) == WANT
