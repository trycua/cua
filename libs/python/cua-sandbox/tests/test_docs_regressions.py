"""Regression tests for bugs found while verifying the sandbox images guide.

Each test here fails against the code as it was before the accompanying fix.
"""

import inspect

import pytest
from cua_sandbox.image import Image


def _build_helpers():
    """Imported lazily so this module still collects against a tree without the fix."""
    from cua_sandbox.builder.build import build_hash, has_build_work

    return build_hash, has_build_work


class TestCloudTransportGetsItsImage:
    """`_make_transport` dropped `image`, so every API-key cloud create raised
    "Cannot create a cloud VM without an image"."""

    def test_make_transport_accepts_an_image(self):
        from cua_sandbox.sandbox import _make_transport

        assert "image" in inspect.signature(_make_transport).parameters

    def test_cloud_transport_receives_the_image(self):
        from cua_sandbox.sandbox import _make_transport

        img = Image.linux()
        transport = _make_transport(api_key="sk-test", name="probe", image=img)
        assert transport._image is img

    def test_create_passes_the_image_to_every_make_transport_that_creates_a_vm(self):
        """Guards the exact regression: the Fleet branch passed image=image while
        the api-key branch right below it silently did not."""
        from cua_sandbox.sandbox import Sandbox

        src = inspect.getsource(Sandbox._create)
        creating_call = "_make_transport(\n                    api_key=api_key,\n"
        assert creating_call in src, "the VM-creating _make_transport call moved"
        start = src.index(creating_call)
        call = src[start : src.index(")", start)]
        assert "image=image" in call


class TestEnvAndFilesAreBuildInputs:
    """`.env()` and `.copy()` were dropped on the QEMU VM path: the builder only
    looked at `_layers`, so the variables and files never reached the guest."""

    def test_env_alone_counts_as_build_work(self):
        _, has_build_work = _build_helpers()
        assert has_build_work(Image.linux().env(MY_TOKEN="abc123"))

    def test_copy_alone_counts_as_build_work(self):
        _, has_build_work = _build_helpers()
        assert has_build_work(Image.linux().copy("./config.json", "/app/config.json"))

    def test_layers_alone_still_counts(self):
        _, has_build_work = _build_helpers()
        assert has_build_work(Image.linux().apt_install("curl"))

    def test_a_plain_image_has_no_build_work(self):
        _, has_build_work = _build_helpers()
        assert not has_build_work(Image.linux())

    def test_env_participates_in_the_cache_key(self):
        """Two images differing only by an env var must not share a built disk."""
        build_hash, _ = _build_helpers()
        a = Image.linux().apt_install("curl").env(TOKEN="a")
        b = Image.linux().apt_install("curl").env(TOKEN="b")
        assert build_hash(a) != build_hash(b)

    def test_copied_file_contents_participate_in_the_cache_key(self, tmp_path):
        build_hash, _ = _build_helpers()
        src = tmp_path / "config.json"
        src.write_text('{"v": 1}')
        first = build_hash(Image.linux().copy(str(src), "/app/config.json"))
        src.write_text('{"v": 2}')
        second = build_hash(Image.linux().copy(str(src), "/app/config.json"))
        assert first != second, "editing a copied file must rebuild the image"

    def test_layers_only_images_keep_their_historical_cache_key(self):
        """Existing cached user images stay valid — the key only changes when
        env or files are actually present."""
        from cua_sandbox.builder.overlay import layers_hash

        build_hash, _ = _build_helpers()
        img = Image.linux().apt_install("curl").pip_install("requests")
        assert build_hash(img) == layers_hash(list(img._layers))


class TestConcurrentLocalVMsDoNotCollide:
    """A second concurrent bare-metal sandbox died on fixed VNC/QMP ports and
    corrupted the first VM's UEFI variables through a shared efivars.fd."""

    def test_vnc_display_skips_a_busy_port(self):
        import socket

        from cua_sandbox.runtime.qemu import _find_free_vnc_display

        with socket.socket() as taken:
            taken.bind(("", 5900))
            taken.listen(1)
            assert _find_free_vnc_display(0) != 0

    def test_efivars_is_per_vm_not_shared(self):
        """Every session disk lives in one directory; a shared efivars.fd would
        be mapped writable into every concurrent UEFI guest at once."""
        from pathlib import Path

        src = inspect.getsource(
            __import__("cua_sandbox.runtime.qemu", fromlist=["x"]).QEMUBaremetalRuntime.start
        )
        assert 'Path(disk_path).parent / "efivars.fd"' not in src
        assert 'with_suffix(".efivars.fd")' in src

        a = Path("/s/alpha.qcow2").with_suffix(".efivars.fd")
        b = Path("/s/beta.qcow2").with_suffix(".efivars.fd")
        assert a != b

    def test_qmp_port_is_allocated_not_fixed(self):
        src = inspect.getsource(
            __import__("cua_sandbox.runtime.qemu", fromlist=["x"]).QEMUBaremetalRuntime.start
        )
        assert "_find_free_port(self.qmp_port)" in src
        assert 'f"tcp:127.0.0.1:{self.qmp_port},server,nowait"' not in src


class TestLayerExecutorKnowsTheGuestOS:
    """The builder constructed LayerExecutor without os_type, so Windows `run`
    layers were wrapped in `sudo bash -c`."""

    def test_build_user_image_passes_os_type(self):
        from cua_sandbox.builder import build as build_mod

        src = inspect.getsource(build_mod.build_user_image)
        # Assert on the LayerExecutor construction itself — build_user_image also
        # calls Image.from_file(..., os_type=image.os_type), which must not count.
        start = src.index("LayerExecutor(")
        construction = src[start : src.index(")", src.index("api_port", start))]
        assert "os_type=image.os_type" in construction


@pytest.mark.parametrize(
    "image, expected_kind",
    [
        (Image.linux(), "vm"),
        (Image.linux(kind="container"), "container"),
        (Image.windows(), "vm"),
    ],
)
def test_to_dict_reports_the_real_kind(image, expected_kind):
    """The guide printed 'kind': 'container' for Image.linux(), which is a VM."""
    assert image.to_dict()["kind"] == expected_kind


class TestDeletingNothingIsNotSuccess:
    """`Sandbox.delete(local=True)` dispatched on runtime_type from a state file
    and took no branch when there was none, so deleting a name that never
    existed reported success — and a container from a launch that timed out
    before writing state could not be deleted at all."""

    def test_unknown_name_raises(self, monkeypatch):
        import asyncio

        import importlib

        # cua_sandbox exports a `sandbox()` function that shadows the submodule.
        sandbox_mod = importlib.import_module("cua_sandbox.sandbox")
        state_mod = importlib.import_module("cua_sandbox.sandbox_state")

        monkeypatch.setattr(state_mod, "load", lambda name: None)
        monkeypatch.setattr(sandbox_mod, "_remove_orphan_container", lambda name: False)

        with pytest.raises(ValueError, match="No local sandbox named"):
            asyncio.run(sandbox_mod.Sandbox._delete_local("never-existed"))

    def test_orphan_container_is_removed(self, monkeypatch):
        import asyncio

        import importlib

        # cua_sandbox exports a `sandbox()` function that shadows the submodule.
        sandbox_mod = importlib.import_module("cua_sandbox.sandbox")
        state_mod = importlib.import_module("cua_sandbox.sandbox_state")

        deleted_state = []
        monkeypatch.setattr(state_mod, "load", lambda name: None)
        monkeypatch.setattr(state_mod, "delete", lambda name: deleted_state.append(name))
        monkeypatch.setattr(sandbox_mod, "_remove_orphan_container", lambda name: True)

        asyncio.run(sandbox_mod.Sandbox._delete_local("timed-out-launch"))
        assert deleted_state == ["timed-out-launch"]

    def test_orphan_probe_reports_missing_container(self, monkeypatch):
        import subprocess

        from cua_sandbox.sandbox import _remove_orphan_container

        def fake_run(cmd, **kwargs):
            assert cmd[:2] == ["docker", "inspect"]
            return subprocess.CompletedProcess(cmd, 1)

        monkeypatch.setattr(subprocess, "run", fake_run)
        assert _remove_orphan_container("nope") is False

    def test_orphan_probe_survives_missing_docker(self, monkeypatch):
        import subprocess

        from cua_sandbox.sandbox import _remove_orphan_container

        def fake_run(cmd, **kwargs):
            raise FileNotFoundError("docker")

        monkeypatch.setattr(subprocess, "run", fake_run)
        assert _remove_orphan_container("nope") is False


class TestLocalRuntimeSelection:
    """`Image.linux()` says kind='vm', but started locally with no explicit
    runtime it lands on the same XFCE container as kind='container'."""

    def test_linux_vm_and_container_resolve_to_the_same_docker_image(self):
        from cua_sandbox.runtime.images import UBUNTU_XFCE, resolve_image

        assert resolve_image("linux") == UBUNTU_XFCE

    def test_only_a_disk_path_reaches_bare_metal_qemu(self):
        from cua_sandbox.runtime.qemu import QEMUBaremetalRuntime
        from cua_sandbox.sandbox import _auto_runtime

        with_disk = Image.from_file("/tmp/does-not-need-to-exist.qcow2", os_type="linux")
        assert isinstance(_auto_runtime(with_disk), QEMUBaremetalRuntime)

        # Documented as a QEMU VM, but auto-selection gives Docker-wrapped QEMU.
        assert not isinstance(_auto_runtime(Image.linux()), QEMUBaremetalRuntime)
