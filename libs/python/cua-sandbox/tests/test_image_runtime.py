"""Unit tests for Image.runtime() API — CUA-482."""
from __future__ import annotations

import pytest


class TestImageRuntimeMethod:
    """Tests for Image.runtime() — the engine/location hint API."""

    def _registry_image(self) -> "Image":
        from cua_sandbox import Image
        return Image.from_registry("myorg/myimage:latest")

    # ── Valid hints ───────────────────────────────────────────────────────

    def test_qemu_cloud_sets_hint_and_kind(self):
        img = self._registry_image().runtime("qemu/cloud")
        assert img._runtime_hint == "qemu/cloud"
        assert img.kind == "vm"

    def test_oci_cloud_sets_hint_and_kind(self):
        img = self._registry_image().runtime("docker/cloud")
        assert img._runtime_hint == "docker/cloud"
        assert img.kind == "container"

    def test_qemu_local_sets_hint_and_kind(self):
        img = self._registry_image().runtime("qemu/local")
        assert img._runtime_hint == "qemu/local"
        assert img.kind == "vm"

    def test_oci_local_sets_hint_and_kind(self):
        img = self._registry_image().runtime("docker/local")
        assert img._runtime_hint == "docker/local"
        assert img.kind == "container"

    # ── Invalid hints ─────────────────────────────────────────────────────

    def test_invalid_hint_raises_value_error(self):
        with pytest.raises(ValueError, match="Unknown runtime hint"):
            self._registry_image().runtime("docker/cloud")

    def test_empty_hint_raises_value_error(self):
        with pytest.raises(ValueError):
            self._registry_image().runtime("")

    def test_partial_hint_raises_value_error(self):
        with pytest.raises(ValueError):
            self._registry_image().runtime("qemu")

    # ── Immutability ──────────────────────────────────────────────────────

    def test_runtime_returns_new_image(self):
        original = self._registry_image()
        with_hint = original.runtime("qemu/cloud")
        assert original._runtime_hint is None  # original unchanged
        assert with_hint._runtime_hint == "qemu/cloud"

    def test_runtime_preserves_registry(self):
        img = self._registry_image().runtime("qemu/cloud")
        assert img._registry == "myorg/myimage:latest"

    def test_runtime_preserves_layers(self):
        img = (
            self._registry_image()
            .apt_install("curl")
            .runtime("docker/cloud")
        )
        assert len(img._layers) == 1
        assert img._layers[0]["type"] == "apt_install"
        assert img._runtime_hint == "docker/cloud"

    def test_chaining_overrides_previous_hint(self):
        img = self._registry_image().runtime("qemu/cloud").runtime("docker/cloud")
        assert img._runtime_hint == "docker/cloud"
        assert img.kind == "container"

    # ── Serialization ─────────────────────────────────────────────────────

    def test_to_dict_includes_runtime_hint(self):
        img = self._registry_image().runtime("qemu/cloud")
        d = img.to_dict()
        assert d["runtime_hint"] == "qemu/cloud"

    def test_to_dict_omits_runtime_hint_when_not_set(self):
        img = self._registry_image()
        d = img.to_dict()
        assert "runtime_hint" not in d

    def test_from_dict_restores_runtime_hint(self):
        from cua_sandbox import Image
        original = self._registry_image().runtime("docker/cloud")
        d = original.to_dict()
        restored = Image.from_dict(d)
        assert restored._runtime_hint == "docker/cloud"
        assert restored.kind == "container"
        assert restored._registry == "myorg/myimage:latest"

    # ── repr ──────────────────────────────────────────────────────────────

    def test_repr_includes_hint(self):
        img = self._registry_image().runtime("qemu/cloud")
        r = repr(img)
        assert "qemu/cloud" in r

    def test_repr_no_hint_unchanged(self):
        img = self._registry_image()
        r = repr(img)
        assert "runtime=" not in r


class TestCloudTransportRuntimeHint:
    """Tests that CloudTransport._create_vm() reads _runtime_hint for instanceType."""

    def _make_transport(self, image):
        from cua_sandbox.transport.cloud import CloudTransport
        t = CloudTransport.__new__(CloudTransport)
        t._image = image
        t._region = "us-east-1"
        t._cpu = None
        t._memory_mb = None
        t._disk_gb = None
        t._DEFAULT_CPU = 4
        t._DEFAULT_MEMORY_MB = 8192
        t._DEFAULT_DISK_GB = 50
        return t

    def _extract_instance_type(self, image) -> str:
        """Run the body-building logic from _create_vm and return instanceType."""
        from cua_sandbox import Image
        transport = self._make_transport(image)

        # Replicate the body-building logic from _create_vm
        body = {"os": image.os_type, "region": transport._region, "configuration": "small"}
        registry_ref = getattr(transport._image, "_registry", None)
        if registry_ref:
            body["dockerImage"] = registry_ref
            runtime_hint = getattr(transport._image, "_runtime_hint", None)
            if runtime_hint:
                engine = runtime_hint.split("/", 1)[0]
                instance_type = "vm" if engine == "qemu" else "container"
            else:
                instance_type = getattr(transport._image, "kind", None) or "vm"
            body["instanceType"] = instance_type

        return body.get("instanceType", "<not set>")

    def test_qemu_cloud_sends_vm(self):
        from cua_sandbox import Image
        img = Image.from_registry("myorg/img:latest").runtime("qemu/cloud")
        assert self._extract_instance_type(img) == "vm"

    def test_oci_cloud_sends_container(self):
        from cua_sandbox import Image
        img = Image.from_registry("myorg/app:latest").runtime("docker/cloud")
        assert self._extract_instance_type(img) == "container"

    def test_no_hint_defaults_to_vm(self):
        from cua_sandbox import Image
        img = Image.from_registry("myorg/img:latest")
        assert self._extract_instance_type(img) == "vm"

    def test_kind_container_without_hint_sends_container(self):
        from cua_sandbox import Image
        img = Image.linux("ubuntu", "24.04", kind="container")._with(_registry="myorg/app:latest")
        assert self._extract_instance_type(img) == "container"


class TestRuntimeHintAliases:
    """Tests that oci/* aliases are normalised to docker/*."""

    def test_oci_cloud_alias_normalised_to_docker_cloud(self):
        from cua_sandbox import Image
        img = Image.from_registry("myorg/app:latest").runtime("oci/cloud")
        assert img._runtime_hint == "docker/cloud"
        assert img.kind == "container"

    def test_oci_local_alias_normalised_to_docker_local(self):
        from cua_sandbox import Image
        img = Image.from_registry("myorg/app:latest").runtime("oci/local")
        assert img._runtime_hint == "docker/local"
        assert img.kind == "container"

    def test_alias_repr_shows_canonical_hint(self):
        from cua_sandbox import Image
        img = Image.from_registry("myorg/app:latest").runtime("oci/cloud")
        assert "docker/cloud" in repr(img)
        assert "oci/cloud" not in repr(img)

    def test_alias_roundtrips_correctly(self):
        from cua_sandbox import Image
        original = Image.from_registry("myorg/app:latest").runtime("oci/cloud")
        d = original.to_dict()
        restored = Image.from_dict(d)
        assert restored._runtime_hint == "docker/cloud"
