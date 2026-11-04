"""Hermetic tests: execution target resolution.

One flag set (``--on``, ``--kind``, ``--runtime``) picks the location, the
kind and the engine, the same as every cua surface. ``--on`` is ``local``
(``cloud`` ended and fails); ``--kind`` is ``container``/``vm``; ``--runtime``
is the engine (``gvisor``, ``runc``, ``qemu``, ``lume``) and implies its kind.
"""

import pytest

from cua_bench.targets import (
    Target,
    TargetError,
    parse_memory_mb,
    resolve_env_spec,
    resolve_target,
)

IMG = "registry.example/desktop:docker-1"


@pytest.fixture(autouse=True)
def _fixed_default_image(monkeypatch):
    monkeypatch.setattr("cua_bench.targets.default_container_image", lambda: "sdk/default:docker")


def native(os_type="linux", **setup):
    return {"provider": "native", "setup_config": {"os_type": os_type, **setup}}


class TestResolveTarget:
    def test_defaults_to_local(self):
        t = resolve_target(environ={})
        assert (t.on, t.kind, t.runtime, t.concurrency) == ("local", None, None, 4)

    def test_an_unknown_location_is_rejected(self):
        with pytest.raises(TargetError, match="--on must be one of local, cloud"):
            resolve_target("azure", environ={})
        with pytest.raises(TargetError, match="from CUA_DEFAULT_ON"):
            resolve_target(environ={"CUA_DEFAULT_ON": "azure"})

    def test_cli_flags(self):
        import argparse

        from cua_bench.cli.commands.run import add_target_args

        parser = argparse.ArgumentParser()
        add_target_args(parser)
        assert parser.parse_args([]).on is None
        assert parser.parse_args(["--on", "cloud"]).on == "cloud"
        assert parser.parse_args(["--kind", "vm"]).kind == "vm"
        assert parser.parse_args(["--runtime", "qemu"]).runtime == "qemu"
        # argparse rejects a location that is not one of the choices.
        with pytest.raises(SystemExit):
            parser.parse_args(["--on", "azure"])

    def test_hetzner_is_your_own_cloud(self):
        import argparse

        from cua_bench.cli.commands.run import add_target_args

        parser = argparse.ArgumentParser()
        add_target_args(parser)
        assert parser.parse_args(["--on", "hetzner"]).on == "hetzner"
        t = resolve_target("hetzner", environ={})
        assert (t.on, t.contrib) == ("hetzner", True)

    def test_env_fallbacks(self):
        t = resolve_target(
            environ={"CUA_DEFAULT_ON": "local", "CUA_DEFAULT_KIND": "vm", "CUA_BENCH_IMAGE": IMG}
        )
        assert (t.on, t.kind, t.image) == ("local", "vm", IMG)
        assert (t.on_source, t.kind_source) == ("CUA_DEFAULT_ON", "CUA_DEFAULT_KIND")

    def test_flags_beat_env(self):
        t = resolve_target("local", "container", environ={"CUA_DEFAULT_ON": "cloud"})
        assert (t.on, t.kind) == ("local", "container")

    def test_runtime_implies_kind(self):
        t = resolve_target("local", runtime="qemu", environ={})
        assert (t.on, t.kind, t.runtime) == ("local", "vm", "qemu")

    def test_resources(self):
        t = resolve_target("local", cpu="2", memory="4G", concurrency=8, environ={})
        assert (t.cpu, t.memory_mb, t.concurrency) == (2, 4096, 8)

    @pytest.mark.parametrize(
        "kwargs",
        [
            {"on": "azure"},  # not a location (aws, gcp and hetzner are your own cloud)
            {"runtime": "kubevirt"},  # not an engine
            {"kind": "gvisor"},  # gvisor is an engine, not a kind
            {"cpu": "0"},
            {"cpu": "two"},
            {"concurrency": 0},
            {"memory": "12"},
        ],
    )
    def test_rejects_bad_values(self, kwargs):
        with pytest.raises(TargetError):
            resolve_target(**kwargs, environ={})

    def test_parsers(self):
        assert parse_memory_mb("8GB") == 8192
        assert parse_memory_mb("512M") == 512
        assert parse_memory_mb(2048) == 2048
        assert parse_memory_mb(None) is None


class TestResolveEnvSpec:
    def test_simulated_runs_on_the_linux_container(self, monkeypatch):
        import warnings

        from cua_bench import targets

        monkeypatch.setattr(targets, "_warned_simulated", False)
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            spec = resolve_env_spec(
                {"provider": "simulated", "setup_config": {"os_type": "win11", "width": 800}},
                Target(),
                environ={},
            )
        assert (spec.provider, spec.os_type, spec.kind) == ("native", "linux", "container")
        assert spec.simulated and spec.needs_sandbox and spec.width == 800
        assert spec.backend("local") == "local-gvisor"
        assert any("simulated provider was removed" in str(w.message) for w in caught)
        # Once per process.
        with warnings.catch_warnings(record=True) as again:
            warnings.simplefilter("always")
            resolve_env_spec({"provider": "webtop"}, Target(), environ={})
        assert not again

    def test_simulated_runs_on_the_bench_ui_desktop(self):
        from cua_bench.images import BENCH_WEB

        spec = resolve_env_spec({"provider": "webtop"}, Target(), environ={})
        assert spec.image == BENCH_WEB
        spec = resolve_env_spec(
            {"provider": "simulated"}, Target(), environ={}, default_image="reg/desk:1"
        )
        assert spec.image == "reg/desk:1"

    def test_default_image_applies_only_when_the_task_names_none(self):
        spec = resolve_env_spec(native(), Target(), default_image="reg/desk:1")
        assert (spec.kind, spec.image) == ("container", "reg/desk:1")
        spec = resolve_env_spec(native(image="task/own:1"), Target(), default_image="reg/desk:1")
        assert spec.image == "task/own:1"
        spec = resolve_env_spec(native(), Target(image="flag/img:1"), default_image="reg/desk:1")
        assert spec.image == "flag/img:1"

    def test_simulated_strict_is_an_error(self):
        with pytest.raises(TargetError, match="simulated provider was removed"):
            resolve_env_spec({"provider": "simulated"}, Target(), environ={"CUA_BENCH_STRICT": "1"})

    def test_linux_defaults_to_container_with_sdk_image(self):
        spec = resolve_env_spec(native(), Target())
        assert (spec.os_type, spec.kind, spec.image) == (
            "linux",
            "container",
            "sdk/default:docker",
        )
        assert spec.backend("local") == "local-gvisor"

    def test_image_precedence(self):
        assert resolve_env_spec(native(image="task/img"), Target(image=IMG)).image == IMG
        assert resolve_env_spec(native(image="task/img"), Target()).image == "task/img"

    def test_windows_is_a_vm_with_builtin_image(self):
        spec = resolve_env_spec(native("win11"), Target())
        assert (spec.os_type, spec.kind, spec.image) == ("windows", "vm", None)
        assert spec.backend("local") == "local-qemu"

    def test_kind_precedence(self):
        assert resolve_env_spec(native(kind="vm"), Target()).kind == "vm"
        assert resolve_env_spec(native(kind="vm"), Target(kind="container")).kind == "container"
        vm = resolve_env_spec(native(), Target(kind="vm"))
        assert (vm.kind, vm.image) == ("vm", None)  # built-in Linux VM disk

    def test_runtime_engine_is_recorded(self):
        spec = resolve_env_spec(native(), Target(on="local", kind="vm", runtime="qemu"))
        assert (spec.kind, spec.runtime, spec.backend("local")) == ("vm", "qemu", "local-qemu")

    def test_server_port_is_parsed_and_range_checked(self):
        plain = resolve_env_spec(native(image=IMG), Target())
        served = resolve_env_spec(native(image=IMG, server_port="8000"), Target())
        assert (plain.server_port, served.server_port) == (None, 8000)
        with pytest.raises(TargetError):
            resolve_env_spec(native(server_port=70000), Target())

    def test_kind_container_rejected_for_windows(self):
        with pytest.raises(TargetError, match="VM-only"):
            resolve_env_spec(native("windows"), Target(kind="container"))

    def test_setup_config_runtime_is_kind_now(self):
        with pytest.raises(TargetError, match="setup_config.kind now"):
            resolve_env_spec(native(runtime="vm"), Target())
        with pytest.raises(TargetError, match="setup_config.kinds now"):
            resolve_env_spec(native(runtimes=["vm"]), Target())

    def test_macos_runs_on_lume(self):
        assert resolve_env_spec(native("macos"), Target()).backend("local") == "local-lume"

    def test_unknown_provider(self):
        with pytest.raises(TargetError):
            resolve_env_spec({"provider": "daytona"}, Target())

    def test_task_object_with_attributes(self):
        class Computer:
            provider = "native"
            setup_config = {"os_type": "linux", "image": IMG, "width": 800, "height": 600}

        spec = resolve_env_spec(Computer(), Target())
        assert (spec.image, spec.width, spec.height) == (IMG, 800, 600)


class TestVariantSelection:
    """--kind auto|container|vm against the OS, the task and the image index."""

    def spec(self, setup=None, target=None, resolver=None):
        return resolve_env_spec(
            {"provider": "native", "setup_config": setup or {}},
            target or Target(),
            variant_resolver=resolver,
        )

    def test_auto_is_accepted_and_means_no_override(self):
        assert resolve_target(kind="auto", environ={}).kind is None
        assert resolve_target(environ={"CUA_DEFAULT_KIND": "auto"}).kind is None

    def test_linux_auto_defaults_to_container(self):
        s = self.spec({"os_type": "linux"})
        assert (s.kind, s.image_variant, s.kind_source) == ("container", "rootfs", "default")

    def test_linux_auto_follows_the_image_index(self):
        calls = []

        def resolver(ref, os_type):
            calls.append((ref, os_type))
            return "vm"

        s = self.spec({"os_type": "linux", "image": "ghcr.io/acme/desk:1"}, resolver=resolver)
        assert (s.kind, s.image_variant, s.kind_source) == ("vm", "containerdisk", "index")
        assert calls == [("ghcr.io/acme/desk:1", "linux")]
        # Unreadable index: the OS default.
        s = self.spec({"image": "ghcr.io/acme/desk:1"}, resolver=lambda r, o: None)
        assert s.kind == "container"

    def test_explicit_kind_wins_for_linux(self):
        s = self.spec({"kind": "container"}, Target(kind="vm"), resolver=lambda r, o: 1 / 0)
        assert (s.kind, s.kind_source) == ("vm", "cli")

    @pytest.mark.parametrize("os_type", ["windows", "macos"])
    def test_windows_and_macos_are_vm_only(self, os_type):
        assert self.spec({"os_type": os_type}).kind == "vm"
        with pytest.raises(TargetError) as err:
            self.spec({"os_type": os_type}, Target(kind="container"))
        assert str(err.value) == f"{os_type} is VM-only: drop --kind or use --kind vm"

    def test_task_requirement_conflicts_with_cli(self):
        assert self.spec({"kinds": ["vm"]}).kind == "vm"
        assert self.spec({"kinds": ["vm"]}).kind_source == "task"
        with pytest.raises(TargetError, match="requires --kind vm"):
            self.spec({"kinds": ["vm"]}, Target(kind="container"))
        with pytest.raises(TargetError, match="kinds"):
            self.spec({"kinds": ["kvm"]})

    def test_task_preference_within_requirement(self):
        s = self.spec({"kinds": ["container", "vm"], "kind": "vm"})
        assert s.kind == "vm"

    @pytest.mark.parametrize(
        "image,expected",
        [
            ("windows", ("windows", None, None, "vm")),
            ("macos:tahoe", ("macos", "tahoe", None, "vm")),
            ("ubuntu:24.04", ("linux", "24.04", None, "container")),
            (
                "ghcr.io/trycua/linux:24.04",
                ("linux", None, "ghcr.io/trycua/linux:24.04", "container"),
            ),
            ("python:3.12-slim", ("linux", None, "python:3.12-slim", "container")),
            ("repo@sha256:abc", ("linux", None, "repo@sha256:abc", "container")),
        ],
    )
    def test_image_forms(self, image, expected):
        s = self.spec({"os_type": "linux"}, Target(image=image))
        assert (s.os_type, s.os_version, s.image, s.kind) == expected
