"""Execution targets: where and how a task's environment runs.

One flag set drives every command:

* ``--on local|cloud``: where. ``local`` runs sandboxes on this machine
  through the cua SDK; ``cloud`` claims them from managed Fleet pools.
* ``--kind auto|container|vm``: what kind. ``auto`` (the default) runs Linux
  as a container unless the task or the image's variant index says VM;
  Windows, macOS and Android are VM-only.
* ``--runtime auto|gvisor|runc|qemu|lume|kubevirt``: which engine. Locally
  ``gvisor``/``runc`` run containers and ``qemu``/``lume`` VMs; in the cloud
  ``gvisor`` runs containers and ``kubevirt`` VMs. An engine implies its
  kind; a combination that does not exist is an error listing the valid
  values.
* ``--cpu``, ``--memory``, ``--image``, ``--warm``, ``--claim-ttl`` and the
  batch concurrency.

Unset ``--on``/``--kind``/``--runtime`` come from the user's defaults, the
same as every cua surface: ``CUA_DEFAULT_ON``/``CUA_DEFAULT_KIND``/
``CUA_DEFAULT_RUNTIME``, then ``$CUA_HOME/config.toml`` (``cua config set
default.on cloud``), then ``local``/``auto``/``auto``. A default kind or
engine that does not fit a task is skipped for it.

The 0.2.11 flags still parse, with a deprecation notice: ``--platform
linux-docker|linux-qemu|windows-qemu|android-qemu`` (see
:data:`LEGACY_PLATFORMS`) and ``--provider-type native``.

A task's image reference resolves the same way on both targets: the CLI
``--image`` wins, then the task's ``setup_config.image``, then
``CUA_BENCH_IMAGE``, then the SDK default for the OS and kind. An image
may be an OS alias (``linux``, ``ubuntu``, ``windows``, ``macos:tahoe``),
the canonical ``ghcr.io/trycua/<os>`` repo, any registry ref or a digest.

Everything here is pure (no I/O) except :func:`user_default`, which reads
the user's config through the cua SDK; tests pass ``environ`` and
``config`` instead.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from typing import Any, Iterable, Mapping, Optional

from cua_bench.images import CANONICAL

#: Contrib providers (third-party platforms) the cua SDK knows; mirrors
#: ``cua.sandbox_locations()`` (``cua_sandbox_core::CONTRIB_LOCATIONS``). The
#: SDK validates the word and says which build has the provider; cua-bench
#: has no provider-specific code and passes the location through as is.
CONTRIB_ON = (
    "e2b",
    "daytona",
    "modal",
    "cloudflare",
    "vercel",
    "morph",
    "runloop",
    "fly",
    "blaxel",
    "codesandbox",
    "northflank",
)
#: Your own cloud account (``cua cloud connect <provider>``), through the
#: cua SDK's ``byoc`` providers; ``modal`` is in both lists (the ``byoc``
#: build runs it joined to the cua.ai relay). Mirrors
#: ``cua_sandbox_core::CLOUD_LOCATIONS``.
YOUR_CLOUD_ON = ("aws", "gcp")
#: Every ``--on`` location: this machine, Cua cloud, a contrib provider or
#: your own cloud.
ON_CHOICES = ("local", "cloud", *CONTRIB_ON, *YOUR_CLOUD_ON)
KIND_CHOICES = ("container", "vm")
#: What ``--kind`` accepts; ``auto`` (or no flag) picks per task.
KIND_FLAG_CHOICES = ("auto", *KIND_CHOICES)
#: The engines each location offers per kind (the cua SDK's rule,
#: ``cua.locations()``).
ENGINES = {
    "local": {"container": ("gvisor", "runc"), "vm": ("qemu", "lume")},
    "cloud": {"container": ("gvisor",), "vm": ("kubevirt",)},
}
#: What ``--runtime`` accepts; ``auto`` (or no flag) lets the SDK pick.
RUNTIME_FLAG_CHOICES = ("auto", "gvisor", "runc", "qemu", "lume", "kubevirt")
#: The kind an engine runs.
ENGINE_KIND = {
    "gvisor": "container",
    "runc": "container",
    "qemu": "vm",
    "lume": "vm",
    "kubevirt": "vm",
}
#: The OSes that only run as VMs.
VM_ONLY_OS = ("windows", "macos", "android")

#: ``--platform`` from cua-bench 0.2.11 (deprecated): ``(image, kind,
#: runtime)``. They all ran on this machine.
LEGACY_PLATFORMS = {
    "linux-docker": (None, "container", None),
    "linux-qemu": ("linux", "vm", "qemu"),
    "windows-qemu": ("windows", "vm", "qemu"),
    # The Android emulator (the only Android path): an Android VM.
    "android-qemu": ("android", "vm", None),
}

#: The user defaults (``cua config``) and the variable that overrides each.
DEFAULT_ENV = {
    "default.on": "CUA_DEFAULT_ON",
    "default.kind": "CUA_DEFAULT_KIND",
    "default.runtime": "CUA_DEFAULT_RUNTIME",
}

#: Fallback when the SDK is not importable: the canonical Linux image
#: (``cua_sandbox.image.canonical_image("linux")``; generated constants).
_DEFAULT_LINUX_CONTAINER_IMAGE = CANONICAL["linux"]

_OS_ALIASES = {
    "linux": "linux",
    "ubuntu": "linux",
    "windows": "windows",
    "win11": "windows",
    "win10": "windows",
    "win7": "windows",
    "winxp": "windows",
    "win98": "windows",
    "macos": "macos",
    "mac": "macos",
    "android": "android",
}

#: Retired in cua-bench 0.3 (the Playwright simulation); tasks that still
#: declare it run on the bench-web Linux desktop, which has bench-ui for their
#: pywebview windows (see :func:`resolve_env_spec`).
_SIMULATED = ("simulated", "webtop")
_NATIVE = ("native", "computer")
#: Tasks that need no environment (static datasets such as the grounding
#: sets): no sandbox, a DatasetSession that shows the item's screenshot.
_DATASET = ("dataset",)

SIMULATED_REMOVED = (
    "the simulated provider was removed in cua-bench 0.3; running this task on a Linux "
    "desktop sandbox ({image}) instead. Its app opens as a desktop window through "
    "session.launch_window (bench-ui, pywebview), which the image provides. Declare "
    "provider='native' to silence this, or set CUA_BENCH_STRICT=1 to make it an error."
)
_warned_simulated = False


class TargetError(ValueError):
    """An execution target or task environment that cannot run as asked."""


def parse_memory_mb(value: Any) -> Optional[int]:
    """``"8G"``, ``"8GB"``, ``"512M"``, ``"4096"`` or an int (MiB) to MiB."""
    if value is None or value == "":
        return None
    if isinstance(value, bool):
        raise TargetError(f"invalid memory size: {value!r}")
    if isinstance(value, (int, float)):
        mb = int(value)
    else:
        match = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*([kmgt]?)(?:i?b)?\s*", str(value).lower())
        if not match:
            raise TargetError(f"invalid memory size: {value!r} (use e.g. 4096, 512M or 8G)")
        amount = float(match.group(1))
        unit = match.group(2) or "m"
        mb = int(amount * {"k": 1 / 1024, "m": 1, "g": 1024, "t": 1024 * 1024}[unit])
    if mb < 256:
        raise TargetError(f"memory must be at least 256 MiB (got {value!r})")
    return mb


def parse_duration_s(value: Any) -> Optional[int]:
    """``900``, ``"90s"``, ``"15m"``, ``"1h"`` to seconds."""
    if value is None or value == "":
        return None
    if isinstance(value, bool):
        raise TargetError(f"invalid duration: {value!r}")
    if isinstance(value, (int, float)):
        return int(value)
    match = re.fullmatch(r"\s*(\d+)\s*([smhd]?)\s*", str(value).lower())
    if not match:
        raise TargetError(f"invalid duration: {value!r} (use e.g. 900, 90s, 15m or 1h)")
    return int(match.group(1)) * {"s": 1, "m": 60, "h": 3600, "d": 86400}[match.group(2) or "s"]


@dataclass(frozen=True)
class Target:
    """Where tasks run. Built with :func:`resolve_target`."""

    on: str = "local"
    #: ``container``/``vm``; None ("auto"): per task and image (see resolve_env_spec).
    kind: Optional[str] = None
    #: The engine (``gvisor``, ``runc``, ``qemu``, ``lume``, ``kubevirt``);
    #: None ("auto"): the SDK picks.
    runtime: Optional[str] = None
    image: Optional[str] = None  # overrides every task's image
    cpu: Optional[int] = None
    memory_mb: Optional[int] = None
    concurrency: int = 4
    warm: bool = False
    claim_ttl_s: Optional[int] = None
    #: Where ``on`` came from: ``cli``, ``env``, ``config`` or ``default``.
    on_source: str = "cli"
    #: Where ``kind`` / ``runtime`` came from (``cli``, ``platform``, ``env``,
    #: ``config``); a defaulted one that does not fit a task is skipped.
    kind_source: str = "cli"
    runtime_source: str = "cli"

    @property
    def cloud(self) -> bool:
        """Cua cloud (Fleet): managed pools, warm capacity, claim TTLs."""
        return self.on == "cloud"

    @property
    def contrib(self) -> bool:
        """A third-party platform (contrib) or your own cloud account: a
        provider of the cua SDK, which picks the engine."""
        return self.on in CONTRIB_ON or self.on in YOUR_CLOUD_ON


def _where(on: str) -> str:
    return "locally" if on == "local" else "in the cloud"


def check_placement(on: str, kind: Optional[str], runtime: Optional[str]) -> None:
    """The cua SDK's placement rule (``cua.check_placement``), offline:
    :class:`TargetError` listing the valid values when ``runtime`` does not
    exist ``on`` that location or runs another kind."""
    if runtime is None:
        return
    if on not in ENGINES:
        return  # a contrib provider validates its own engines in the cua SDK
    offered = ENGINES[on]
    if kind is not None:
        valid = offered[kind]
        if runtime in valid:
            return
        engine_kind = ENGINE_KIND.get(runtime)
        if engine_kind is not None and runtime in offered.get(engine_kind, ()):
            reason = f"runtime {runtime} runs {engine_kind} sandboxes, not {kind} ones {_where(on)}"
        else:
            reason = f"runtime {runtime} is not available {_where(on)}"
        raise TargetError(
            f"invalid placement: {reason}; valid runtime: {', '.join(('auto', *valid))}"
        )
    every = [r for k in KIND_CHOICES for r in offered[k]]
    if runtime not in every:
        raise TargetError(
            f"invalid placement: runtime {runtime} is not available {_where(on)}; "
            f"valid runtime: {', '.join(('auto', *every))}"
        )


def user_default(
    key: str,
    *,
    environ: Optional[Mapping[str, str]] = None,
    config: Optional[Mapping[str, str]] = None,
) -> tuple[Optional[str], str]:
    """``(value, source)`` of a user default (``default.on``, ``default.kind``,
    ``default.runtime``): its environment variable, then
    ``$CUA_HOME/config.toml``, else ``(None, "default")``.

    ``config`` stands in for the config file (tests); without it and with
    the real environment, the cua SDK reads it (``cua.config_get``).
    """
    env = os.environ if environ is None else environ
    variable = DEFAULT_ENV[key]
    value = str(env.get(variable) or "").strip().lower()
    if value:
        return value, "env"
    if config is not None:
        value = str(config.get(key) or "").strip().lower()
        return (value, "config") if value else (None, "default")
    if environ is not None and environ is not os.environ:
        return None, "default"  # an explicit environment: pure, no config read
    try:
        import cua

        entry = cua.config_get(key)
    except Exception:  # noqa: BLE001 - no SDK, or an unreadable config: built-in
        return None, "default"
    if str(entry.source) == "config":
        return str(entry.value).strip().lower() or None, "config"
    return None, "default"


def _legacy_platform(value: str) -> tuple[Optional[str], str, Optional[str]]:
    import sys
    import warnings

    name = value.strip().lower()
    if name not in LEGACY_PLATFORMS:
        raise TargetError(
            f"--platform must be one of {', '.join(LEGACY_PLATFORMS)} (got {value!r}); "
            "it is deprecated: use --kind and --runtime"
        )
    image, kind, runtime = LEGACY_PLATFORMS[name]
    parts = [f"--kind {kind}"]
    if runtime:
        parts.append(f"--runtime {runtime}")
    if image:
        parts.append(f"--image {image}")
    message = f"--platform {name} is deprecated: use {' '.join(parts)} (with --on local)"
    warnings.warn(message, DeprecationWarning, stacklevel=3)
    print(f"warning: {message}", file=sys.stderr)
    return image, kind, runtime


def _choice(value: Any, flag: str, choices: tuple, source: str) -> Optional[str]:
    """A flag value (``auto``/empty is ``None``), validated."""
    word = str(value or "").strip().lower()
    if not word:
        return None
    if word not in choices:
        origin = "" if source == "cli" else f" (from {source})"
        raise TargetError(f"{flag} must be one of {', '.join(choices)} (got {word!r}{origin})")
    return None if word == "auto" else word


def resolve_target(
    on: Optional[str] = None,
    kind: Optional[str] = None,
    *,
    runtime: Optional[str] = None,
    platform: Optional[str] = None,
    image: Optional[str] = None,
    cpu: Any = None,
    memory: Any = None,
    concurrency: Any = None,
    warm: bool = False,
    claim_ttl: Any = None,
    environ: Optional[Mapping[str, str]] = None,
    config: Optional[Mapping[str, str]] = None,
) -> Target:
    """Validate CLI flags into a Target. Unset ``on``/``kind``/``runtime``
    come from :func:`user_default`; ``CUA_BENCH_IMAGE`` is the image
    fallback. ``platform`` is the deprecated 0.2.11 ``--platform``."""
    env = os.environ if environ is None else environ
    image = (image or env.get("CUA_BENCH_IMAGE") or "").strip() or None
    kind_source = runtime_source = "cli"
    legacy_image_name = image is not None and image.lower() in LEGACY_PLATFORMS
    if platform or legacy_image_name:
        # 0.2.11: `--platform windows-qemu` (and `--image windows-qemu`, the
        # image names of that release).
        legacy_image, legacy_kind, legacy_runtime = _legacy_platform(platform or image or "")
        if legacy_image_name:
            image = None
        image = image or legacy_image
        if not kind:
            kind, kind_source = legacy_kind, "platform"
        if not runtime and legacy_runtime:
            runtime, runtime_source = legacy_runtime, "platform"
        on = on or "local"

    def default(key: str) -> tuple[Optional[str], str]:
        value, source = user_default(key, environ=environ, config=config)
        return value, ({"env": DEFAULT_ENV[key], "config": "config.toml"}.get(source, source))

    on_source = "cli"
    if not on:
        on, on_source = default("default.on")
        on = on or "local"
    on = _choice(on, "--on", ON_CHOICES, on_source) or "local"

    kind = _choice(kind, "--kind", KIND_FLAG_CHOICES, kind_source)
    runtime = _choice(runtime, "--runtime", RUNTIME_FLAG_CHOICES, runtime_source)
    if runtime is not None:
        # Explicit: must exist here and fit an explicit kind.
        check_placement(on, kind, runtime)
        if kind is None:
            kind = ENGINE_KIND[runtime]
    if kind is None:
        value, source = default("default.kind")
        value = _choice(value, "--kind", KIND_FLAG_CHOICES, source)
        if value is not None:
            kind, kind_source = value, source
    if runtime is None:
        value, source = default("default.runtime")
        value = _choice(value, "--runtime", RUNTIME_FLAG_CHOICES, source)
        try:
            check_placement(on, kind, value)
        except TargetError:
            value = None  # a default that does not fit is skipped
        if value is not None:
            runtime, runtime_source = value, source

    cpu_n: Optional[int] = None
    if cpu not in (None, ""):
        try:
            cpu_n = int(cpu)
        except (TypeError, ValueError):
            raise TargetError(f"--cpu must be an integer (got {cpu!r})") from None
        if cpu_n < 1:
            raise TargetError("--cpu must be at least 1")

    n = 4 if concurrency in (None, "") else concurrency
    try:
        n = int(n)
    except (TypeError, ValueError):
        raise TargetError(f"--max-parallel must be an integer (got {concurrency!r})") from None
    if n < 1:
        raise TargetError("--max-parallel must be at least 1")

    ttl = parse_duration_s(claim_ttl)
    if ttl is not None and not 60 <= ttl <= 7 * 86400:
        raise TargetError("--claim-ttl must be between 60s and 7d")

    return Target(
        on=on,
        kind=kind,
        runtime=runtime,
        image=image,
        cpu=cpu_n,
        memory_mb=parse_memory_mb(memory),
        concurrency=n,
        warm=bool(warm),
        claim_ttl_s=ttl,
        on_source=on_source,
        kind_source=kind_source,
        runtime_source=runtime_source,
    )


@dataclass(frozen=True)
class EnvSpec:
    """The environment one task variant needs, resolved against a Target."""

    provider: str  # "native" (a sandbox) or "dataset" (no environment)
    os_type: str = "linux"  # linux | windows | macos | android
    kind: str = "container"  # container | vm | none (dataset)
    image: Optional[str] = None  # registry ref; None: the SDK's built-in for os/kind
    width: Optional[int] = None
    height: Optional[int] = None
    # A port the image serves itself (its own daemon): exposed and used as the
    # readiness probe. Unset: daemon-agnostic readiness, spacesd on 3211.
    server_port: Optional[int] = None
    #: Version of the built-in image when ``image`` is None (``--image macos:tahoe``).
    os_version: Optional[str] = None
    #: Why this kind: "cli" (--kind or the --runtime engine), "platform"
    #: (--platform), "task" (setup_config), "index" (the image's variant
    #: index), "env"/"config" (a user default), "default" (OS default).
    kind_source: str = "default"
    #: The engine (gvisor, runc, qemu, lume, kubevirt); None: the SDK picks.
    runtime: Optional[str] = None
    #: What the task needs from the target (``setup_config.requires``): kvm,
    #: egress, openai, hf-gated, env:NAME. Checked by ``cb run`` up front.
    requires: tuple = ()
    #: ``--image pool:<name>``: claim from this existing Fleet pool (its
    #: template decides image, kind and runtime) instead of a managed pool.
    pool: Optional[str] = None
    #: More guest ports the task reaches (``setup_config.ports``): exposed
    #: like ``server_port`` (``sb.exposed_ports`` locally, a ``port-N``
    #: service in the cloud), without readiness probing.
    ports: tuple = ()

    #: The task declared the retired simulated provider (mapped to native).
    simulated: bool = False

    @property
    def needs_sandbox(self) -> bool:
        return self.provider != "dataset"

    @property
    def pool_key(self) -> tuple:
        """Sandboxes with equal keys are interchangeable (one managed pool)."""
        return (
            self.os_type,
            self.kind,
            self.runtime,
            self.image,
            self.server_port,
            self.os_version,
            self.pool,
            self.ports,
        )

    def backend(self, on: str) -> str:
        """What runs the sandbox, as ``<location>-<engine>``: ``local-gvisor``,
        ``local-qemu``, ``cloud-gvisor``, ``cloud-kubevirt``, ..."""
        if not self.needs_sandbox:
            return "none"
        if self.pool:
            return f"cloud-pool:{self.pool}"
        if on not in ("local", "cloud"):
            # A contrib provider: the SDK picks the engine.
            return f"{on}-{self.runtime or 'auto'}"
        if on == "cloud":
            engine = self.runtime or ("gvisor" if self.kind == "container" else "kubevirt")
            return f"cloud-{engine}"
        if self.runtime:
            return f"local-{self.runtime}"
        if self.kind == "container":
            return "local-gvisor"
        return {"macos": "local-lume", "android": "local-android-emulator"}.get(
            self.os_type, "local-qemu"
        )

    @property
    def image_label(self) -> str:
        if not self.needs_sandbox:
            return "no environment (dataset)"
        if self.pool:
            return f"pool:{self.pool}"
        if self.image:
            return self.image
        version = f" {self.os_version}" if self.os_version else ""
        return f"built-in {self.os_type}{version} {self.kind}"

    @property
    def image_variant(self) -> str:
        """The image variant this kind pulls: rootfs, containerdisk or lume."""
        if not self.needs_sandbox:
            return "none"
        if self.kind == "container":
            return "rootfs"
        return "lume" if self.os_type == "macos" or self.runtime == "lume" else "containerdisk"


def parse_image_alias(value: Any) -> Optional[tuple[str, Optional[str]]]:
    """``(os_type, version)`` for an OS alias image (``windows``, ``macos:tahoe``).

    ``None`` for anything else (a registry ref: ``ghcr.io/...``, ``python:3.12``,
    ``repo@sha256:...``). Only bare OS names count, so ``ubuntu:24.04`` is an
    alias for the canonical Linux image, not docker.io/library/ubuntu.
    """
    if not isinstance(value, str) or not value or "/" in value or "@" in value:
        return None
    name, _, version = value.strip().partition(":")
    os_type = _OS_ALIASES.get(name.lower())
    if os_type is None:
        return None
    return os_type, (version or None)


def parse_pool_image(value: Any) -> Optional[str]:
    """The pool name of a ``pool:<name>`` image (``fleet:<name>``: deprecated).

    ``pool:`` names an existing Fleet pool (the unified refs scheme);
    ``fleet:`` never names an image any more but still parses, with a warning.
    """
    if not isinstance(value, str):
        return None
    for prefix in ("pool:", "fleet:"):
        if value.startswith(prefix) and "/" not in value:
            name = value[len(prefix) :].strip()
            if not name:
                raise TargetError(f"{value!r}: missing pool name")
            if prefix == "fleet:":
                import warnings

                warnings.warn(
                    f"--image {value} is deprecated: use pool:{name}",
                    DeprecationWarning,
                    stacklevel=3,
                )
            return name
    return None


VariantResolver = Any  # Callable[[str, str], Optional[str]]: (ref, os_type) -> kind


def normalize_os(value: Any) -> str:
    os_type = str(value or "linux").strip().lower()
    return _OS_ALIASES.get(os_type, os_type)


def _computer_dict(computer: Any) -> dict:
    if computer is None:
        return {}
    if isinstance(computer, Mapping):
        return dict(computer)
    return {
        k: getattr(computer, k)
        for k in ("provider", "setup_config")
        if getattr(computer, k, None) is not None
    }


def _simulated_to_native(
    setup: Mapping[str, Any],
    target: Target,
    environ: Optional[Mapping[str, str]],
    default_image: Optional[str] = None,
) -> EnvSpec:
    """A retired ``simulated``/``webtop`` task: a Linux desktop, with a warning.

    The simulation only drew an OS look around the task's window, so its
    ``os_type`` is ignored: the task's pywebview window opens on the bench-web
    Linux desktop, which ships bench-ui (container unless the target asks for
    a VM).
    """
    import warnings

    global _warned_simulated
    env = os.environ if environ is None else environ
    image = target.image or setup.get("image") or default_image or _bench_ui_image()
    message = SIMULATED_REMOVED.format(image=image)
    if str(env.get("CUA_BENCH_STRICT", "")).strip().lower() in ("1", "true", "yes", "on"):
        raise TargetError(message)
    if not _warned_simulated:
        _warned_simulated = True
        warnings.warn(message, DeprecationWarning, stacklevel=3)
    kind = target.kind or "container"
    return EnvSpec(
        provider="native",
        os_type="linux",
        kind=kind,
        image=image,
        width=setup.get("width"),
        height=setup.get("height"),
        simulated=True,
        runtime=_engine_for(target, kind),
    )


def resolve_env_spec(
    computer: Any,
    target: Target,
    *,
    environ: Optional[Mapping[str, str]] = None,
    variant_resolver: Optional[VariantResolver] = None,
    default_image: Optional[str] = None,
) -> EnvSpec:
    """The EnvSpec for a task's ``computer`` declaration on ``target``.

    ``default_image`` is the image for a task that names none (a registry
    dataset's desktop image); ``--image`` and the task's own image win.

    ``computer`` is the task's ``computer={"provider": ..., "setup_config": {...}}``.
    ``setup_config`` may carry ``os_type``, ``image`` (a registry ref or OS
    alias), ``kind`` (container|vm, a preference), ``kinds`` (the kinds the
    task supports, a requirement: ``["vm"]`` for tasks that need a kernel),
    ``server_port`` (a port the image serves itself, used for readiness),
    ``width`` and ``height``.

    Kind (``--kind auto`` or no flag): the task's requirement, then its
    preference, then VM for Windows/macOS/Android, then a user default
    kind, then for Linux what the image's variant index offers
    (``variant_resolver(ref, os_type)``, when given) and otherwise a
    container. An explicit ``--kind`` (or ``--runtime`` engine) wins unless
    it conflicts with the OS or the task's requirement, which is an error.
    The engine is ``--runtime`` (or a user default that fits the kind).
    """
    comp = _computer_dict(computer)
    provider = str(comp.get("provider") or "native").strip().lower()
    setup = dict(comp.get("setup_config") or {})
    width = setup.get("width")
    height = setup.get("height")
    for old, new in (("runtime", "kind"), ("runtimes", "kinds")):
        if old in setup:
            raise TargetError(
                f"setup_config.{old} is setup_config.{new} now (container|vm); "
                f"setup_config has no engine"
            )

    if provider in _SIMULATED:
        return _simulated_to_native(setup, target, environ, default_image)
    if provider in _DATASET:
        # No environment: --kind, --runtime and --image do not apply.
        return EnvSpec(
            provider="dataset",
            os_type=normalize_os(setup.get("os_type")),
            kind="none",
            image=None,
            width=width,
            height=height,
            kind_source="dataset",
            requires=tuple(r for r in _task_requires(setup) if r != "kvm"),
        )
    if provider not in _NATIVE:
        raise TargetError(
            f"unknown task provider {provider!r}: use 'native' (a real sandbox) or 'dataset' "
            "(no environment)"
        )

    os_type = normalize_os(setup.get("os_type"))
    if os_type not in ("linux", "windows", "macos", "android"):
        raise TargetError(f"unsupported os_type {setup.get('os_type')!r}")

    image = target.image or setup.get("image") or default_image or None
    pool = parse_pool_image(image)
    if pool is not None:
        # `pool:<name>`: an existing Fleet pool; its template decides the rest.
        if not target.cloud:
            raise TargetError(f"pool:{pool} names a Fleet pool: run it with --on cloud")
        kind = (
            target.kind
            or str(setup.get("kind") or "").strip().lower()
            or ("container" if os_type == "linux" else "vm")
        )
        return EnvSpec(
            provider="native",
            os_type=os_type,
            kind=kind if kind in KIND_CHOICES else "container",
            image=None,
            width=width,
            height=height,
            kind_source="task" if setup.get("kind") else "default",
            requires=_task_requires(setup),
            pool=pool,
        )
    os_version = None
    alias = parse_image_alias(image)
    if alias is not None:
        # `--image windows`, `macos:tahoe`: the canonical image of that OS.
        os_type, os_version = alias
        image = None

    kind, source = _pick_kind(setup, target, os_type, image, variant_resolver)
    if target.cloud and os_type in ("macos", "android"):
        raise TargetError(
            f"{os_type} sandboxes are not available on --on cloud; run them with --on local "
            + ("(Lume on Apple Silicon)" if os_type == "macos" else "(Android emulator)")
        )

    if image is None and kind == "container" and os_version is None:
        image = default_container_image()
    if target.cloud and image is None and os_type not in ("linux", "windows"):
        raise TargetError(f"no built-in cloud image for {os_type}; pass --image")

    server_port = setup.get("server_port")
    if server_port is not None:
        try:
            server_port = int(server_port)
        except (TypeError, ValueError):
            raise TargetError("setup_config.server_port must be a port number") from None
        if not 1 <= server_port <= 65535:
            raise TargetError(f"setup_config.server_port out of range: {server_port}")

    return EnvSpec(
        provider="native",
        os_type=os_type,
        kind=kind,
        image=image,
        width=width,
        height=height,
        server_port=server_port,
        os_version=os_version,
        kind_source=source,
        requires=_task_requires(setup),
        ports=_task_ports(setup, server_port),
        runtime=_engine_for(target, kind),
    )


def _engine_for(target: Target, kind: str) -> Optional[str]:
    """The target's engine for a sandbox of ``kind``: an explicit ``--runtime``
    must fit (its kind decided the task's kind); a defaulted one that does not
    fit is skipped."""
    if target.runtime is None:
        return None
    try:
        check_placement(target.on, kind, target.runtime)
    except TargetError:
        if target.runtime_source in ("cli", "platform"):
            raise
        return None
    return target.runtime


def _task_ports(setup: Mapping[str, Any], server_port: Optional[int]) -> tuple:
    raw = setup.get("ports") or ()
    values = [raw] if isinstance(raw, (int, str)) else list(raw)
    ports = set()
    for v in values:
        try:
            port = int(v)
        except (TypeError, ValueError):
            raise TargetError(f"setup_config.ports must be port numbers (got {v!r})") from None
        if not 1 <= port <= 65535:
            raise TargetError(f"setup_config.ports out of range: {port}")
        if port != server_port:
            ports.add(port)
    return tuple(sorted(ports))


def _task_requires(setup: Mapping[str, Any]) -> tuple:
    raw = setup.get("requires") or ()
    values = [raw] if isinstance(raw, str) else list(raw)
    return tuple(sorted({str(v).strip() for v in values if str(v).strip()}))


def check_requirements(
    specs: Iterable[EnvSpec],
    target: Target,
    *,
    environ: Optional[Mapping[str, str]] = None,
    has_kvm: Optional[bool] = None,
) -> None:
    """Raise :class:`TargetError` when a task's ``requires`` is not met on ``target``."""
    from .adapters.base import unmet_requirements

    env = os.environ if environ is None else environ
    if has_kvm is None:
        has_kvm = os.path.exists("/dev/kvm")
    problems: set[str] = set()
    for spec in specs:
        problems.update(
            unmet_requirements(spec.requires, cloud=target.cloud, environ=env, has_kvm=has_kvm)
        )
    if problems:
        raise TargetError("; ".join(sorted(problems)))


def vm_only_error(os_type: str) -> TargetError:
    return TargetError(f"{os_type} is VM-only: drop --kind or use --kind vm")


def _task_kinds(setup: Mapping[str, Any]) -> tuple[str, ...]:
    """``setup_config.kinds``: the kinds a task can run as (a requirement)."""
    raw = setup.get("kinds")
    if raw in (None, "", [], ()):
        return ()
    values = [raw] if isinstance(raw, str) else list(raw)
    kinds = tuple(str(v).strip().lower() for v in values)
    bad = [k for k in kinds if k not in KIND_CHOICES]
    if bad:
        raise TargetError(f"setup_config.kinds must list container and/or vm (got {raw!r})")
    return kinds


def _pick_kind(
    setup: Mapping[str, Any],
    target: Target,
    os_type: str,
    image: Optional[str],
    variant_resolver: Optional[VariantResolver],
) -> tuple[str, str]:
    """``(kind, source)`` for a task on ``target`` (see :func:`resolve_env_spec`)."""
    vm_only = os_type in VM_ONLY_OS
    required = _task_kinds(setup)
    preferred = str(setup.get("kind") or "").strip().lower() or None
    if preferred in ("auto", "any"):
        preferred = None
    if preferred is not None and preferred not in KIND_CHOICES:
        raise TargetError(f"setup_config.kind must be container or vm (got {preferred!r})")
    if vm_only and "container" in required and "vm" not in required:
        raise TargetError(f"{os_type} tasks cannot require a container (setup_config.kinds)")

    explicit = target.kind is not None and target.kind_source in ("cli", "platform")
    if explicit:
        flag = f"--runtime {target.runtime}" if target.runtime else f"--kind {target.kind}"
        if vm_only and target.kind == "container":
            if target.kind_source == "cli" and target.runtime:
                raise TargetError(f"{os_type} is VM-only: {flag} runs containers")
            raise vm_only_error(os_type)
        if required and target.kind not in required:
            raise TargetError(
                f"this task requires --kind {' or '.join(required)} "
                f"(setup_config.kinds); drop {flag}"
            )
        return target.kind, target.kind_source
    if vm_only:
        if preferred == "container":
            raise vm_only_error(os_type)
        return "vm", "default" if preferred is None and not required else "task"
    if required:
        return (preferred if preferred in required else required[0]), "task"
    if preferred is not None:
        return preferred, "task"
    if target.kind is not None:
        # A user default (CUA_DEFAULT_KIND / default.kind).
        return target.kind, target.kind_source
    if image is not None and variant_resolver is not None:
        # Linux on auto: run what the image offers (a rootfs as a container,
        # a containerDisk-only index as a VM).
        kind = variant_resolver(image, os_type)
        if kind in KIND_CHOICES:
            return kind, "index"
    return "container", "default"


def _bench_ui_image() -> str:
    """The Linux desktop with bench-ui (pywebview) for task windows: bench-web."""
    from cua_bench.images import image

    return image("BENCH_WEB")


def default_container_image() -> str:
    """The canonical Linux image (``CUA_IMAGE_LINUX`` overrides it)."""
    try:
        from cua_sandbox.image import canonical_image

        return canonical_image("linux")
    except Exception:  # noqa: BLE001 - SDK not importable: use the known default
        return (
            os.environ.get("CUA_IMAGE_LINUX")
            or os.environ.get("CUA_SANDBOX_LINUX_CONTAINER_IMAGE")
            or _DEFAULT_LINUX_CONTAINER_IMAGE
        )


@dataclass
class PoolPlan:
    """One group of interchangeable sandboxes (one managed pool in the cloud)."""

    spec: EnvSpec
    tasks: int
    max_pool_size: int
    job_ids: list = field(default_factory=list)


def plan_claims(jobs: Iterable[tuple[Any, EnvSpec]], concurrency: int) -> list[PoolPlan]:
    """Group jobs by pool key and size each pool for the batch.

    Each group becomes one managed pool; its ``max_pool_size`` is the number
    of sandboxes the batch can hold at once from it, ``min(concurrency,
    tasks in the group)``, so KEDA scales the pool to the batch and back to
    zero afterwards.
    """
    groups: dict[tuple, PoolPlan] = {}
    for job_id, spec in jobs:
        if not spec.needs_sandbox:
            continue  # dataset tasks claim nothing
        plan = groups.get(spec.pool_key)
        if plan is None:
            plan = groups[spec.pool_key] = PoolPlan(spec=spec, tasks=0, max_pool_size=0)
        plan.tasks += 1
        plan.job_ids.append(job_id)
    for plan in groups.values():
        plan.max_pool_size = max(1, min(int(concurrency), plan.tasks))
    return list(groups.values())
