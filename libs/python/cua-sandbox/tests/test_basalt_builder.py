"""Unit tests for BasaltQEMUBuilder layer → step translation.

Tests the _layers_to_basalt_steps() function which translates Image layers
into basalt JSON step files. Does NOT require a basalt binary or root access.
"""
from __future__ import annotations

import json

import pytest


def _make_step_file(layers):
    from cua_sandbox.builder.basalt import _layers_to_basalt_steps
    return _layers_to_basalt_steps(layers, os_type="linux")


class TestLayersToBasaltSteps:
    """Tests for _layers_to_basalt_steps() translation."""

    def test_empty_layers_produces_no_steps(self):
        sf = _make_step_file([])
        assert sf["steps"] == {}
        assert sf["targets"] == []

    def test_single_apt_install(self):
        sf = _make_step_file([{"type": "apt_install", "packages": ["curl", "git"]}])
        assert len(sf["steps"]) == 1
        step_name = list(sf["steps"].keys())[0]
        assert "apt_install" in step_name
        step = sf["steps"][step_name]
        # run should contain apt-get install
        run = step["run"]
        cmd_str = json.dumps(run)
        assert "apt-get install" in cmd_str
        assert "curl" in cmd_str
        assert "git" in cmd_str

    def test_run_layer(self):
        sf = _make_step_file([{"type": "run", "command": "echo hello"}])
        step = list(sf["steps"].values())[0]
        cmd_str = json.dumps(step["run"])
        assert "echo hello" in cmd_str

    def test_sequential_deps(self):
        """Each step should depend on the previous one."""
        layers = [
            {"type": "apt_install", "packages": ["curl"]},
            {"type": "run", "command": "curl --version"},
            {"type": "run", "command": "echo done"},
        ]
        sf = _make_step_file(layers)
        steps = sf["steps"]
        assert len(steps) == 3

        names = list(steps.keys())
        # step[1] should have a step dep on steps[0]
        step1_deps = steps[names[1]]["deps"]
        step_dep_names = [d["name"] for d in step1_deps if d.get("type") == "step"]
        assert names[0] in step_dep_names

        # step[2] should depend on step[1]
        step2_deps = steps[names[2]]["deps"]
        step_dep_names2 = [d["name"] for d in step2_deps if d.get("type") == "step"]
        assert names[1] in step_dep_names2

    def test_first_step_has_no_step_dep(self):
        """The first step should have no step deps (only the layer-hash dep)."""
        sf = _make_step_file([{"type": "run", "command": "true"}])
        step = list(sf["steps"].values())[0]
        step_deps = [d for d in step["deps"] if d.get("type") == "step"]
        assert step_deps == []

    def test_target_is_last_step(self):
        layers = [
            {"type": "run", "command": "true"},
            {"type": "run", "command": "false || true"},
        ]
        sf = _make_step_file(layers)
        last_step = list(sf["steps"].keys())[-1]
        assert sf["targets"] == [last_step]

    def test_layer_hash_dep_present(self):
        """Every step should have an exec_output dep that encodes the layer hash."""
        sf = _make_step_file([{"type": "run", "command": "whoami"}])
        step = list(sf["steps"].values())[0]
        hash_deps = [
            d for d in step["deps"]
            if d.get("type") == "exec_output" and "layer-hash:" in " ".join(d.get("args", []))
        ]
        assert len(hash_deps) == 1

    def test_layer_hash_changes_with_content(self):
        """Different layer content → different hash dep value."""
        sf1 = _make_step_file([{"type": "run", "command": "echo foo"}])
        sf2 = _make_step_file([{"type": "run", "command": "echo bar"}])
        step1 = list(sf1["steps"].values())[0]
        step2 = list(sf2["steps"].values())[0]
        hash1 = [d["args"] for d in step1["deps"] if d.get("type") == "exec_output"][0]
        hash2 = [d["args"] for d in step2["deps"] if d.get("type") == "exec_output"][0]
        assert hash1 != hash2

    def test_expose_layer_is_noop(self):
        """expose layers should generate a 'true' no-op command."""
        sf = _make_step_file([{"type": "expose", "port": 8080}])
        step = list(sf["steps"].values())[0]
        cmd_str = json.dumps(step["run"])
        assert "true" in cmd_str

    def test_env_layer(self):
        sf = _make_step_file([{"type": "env", "variables": {"FOO": "bar"}}])
        step = list(sf["steps"].values())[0]
        cmd_str = json.dumps(step["run"])
        assert "FOO" in cmd_str
        assert "bar" in cmd_str

    def test_empty_env_layer_is_noop(self):
        sf = _make_step_file([{"type": "env", "variables": {}}])
        step = list(sf["steps"].values())[0]
        cmd_str = json.dumps(step["run"])
        assert "true" in cmd_str

    def test_pip_install_layer(self):
        sf = _make_step_file([{"type": "pip_install", "packages": ["requests"]}])
        step = list(sf["steps"].values())[0]
        cmd_str = json.dumps(step["run"])
        assert "pip3" in cmd_str
        assert "requests" in cmd_str

    def test_unknown_layer_type_is_noop(self):
        """Unknown layer types should not raise — they should be no-ops."""
        sf = _make_step_file([{"type": "totally_unknown_layer", "data": "x"}])
        step = list(sf["steps"].values())[0]
        cmd_str = json.dumps(step["run"])
        assert "true" in cmd_str

    def test_step_file_is_json_serialisable(self):
        layers = [
            {"type": "apt_install", "packages": ["curl"]},
            {"type": "run", "command": "curl --version"},
            {"type": "env", "variables": {"PATH": "/usr/local/bin:$PATH"}},
        ]
        sf = _make_step_file(layers)
        # Should not raise
        serialised = json.dumps(sf)
        restored = json.loads(serialised)
        assert restored["steps"] == sf["steps"]


class TestBasaltBuilderInit:
    """Tests for BasaltQEMUBuilder initialization (no I/O)."""

    def test_default_nbd_device(self):
        from cua_sandbox.builder.basalt import BasaltQEMUBuilder
        b = BasaltQEMUBuilder()
        assert b.nbd_device == "/dev/nbd0"

    def test_custom_nbd_device(self):
        from cua_sandbox.builder.basalt import BasaltQEMUBuilder
        b = BasaltQEMUBuilder(nbd_device="/dev/nbd1")
        assert b.nbd_device == "/dev/nbd1"

    def test_custom_images_dir(self, tmp_path):
        from cua_sandbox.builder.basalt import BasaltQEMUBuilder
        b = BasaltQEMUBuilder(images_dir=tmp_path)
        assert b.images_dir == tmp_path


class TestCloudBuilderStubs:
    """Tests that cloud builder stubs raise NotImplementedError."""

    @pytest.mark.asyncio
    async def test_qemu_cloud_builder_raises(self):
        from cua_sandbox.builder.basalt import QEMUCloudBuilder
        from cua_sandbox import Image
        builder = QEMUCloudBuilder()
        with pytest.raises(NotImplementedError, match="qemu/cloud"):
            await builder.build(Image.from_registry("myorg/img:latest"))

    @pytest.mark.asyncio
    async def test_docker_cloud_builder_raises(self):
        from cua_sandbox.builder.basalt import DockerCloudBuilder
        from cua_sandbox import Image
        builder = DockerCloudBuilder()
        with pytest.raises(NotImplementedError, match="docker/cloud"):
            await builder.build(Image.from_registry("myorg/app:latest"))
