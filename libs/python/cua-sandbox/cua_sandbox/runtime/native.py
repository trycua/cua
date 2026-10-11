"""Local runtimes on the ``cua`` SDK (cua-vmm): containers (gVisor), QEMU, Lume.

``DockerRuntime``, the QEMU runtimes and ``LumeRuntime`` are thin adapters
over :class:`NativeRuntime`: the SDK provisions the backend (``runsc`` for
containers, QEMU firmware/accel, ``lume serve``), starts the instance, publishes
its ports and persists ``~/.cua/sandboxes/<name>.json`` in the format
``sandbox_state`` reads. Readiness is daemon-agnostic: the backend reports
the instance running, plus the image's own ``server_port`` probe when one is
declared.

Image layers (``pip_install``, ``run``, ``copy``, ``env``, ...) on a container
image are built into the local container engine before boot, the same way
the cloud builds them remotely (cached by the same content hash,
``cua-b-<hash>``), so they work on any image. A VM image takes them after
boot through cua-spacesd, and only when it runs one (otherwise
:class:`~cua_sandbox.Unsupported`).
"""

from __future__ import annotations

import logging
import os
import secrets
from typing import Any, Optional

from cua_sandbox._sdk import ENV_SERVICE, SPACESD_PORT, local_runtime, millis, native
from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo

logger = logging.getLogger(__name__)

#: runtime_type values the SDK writes to state files.
NATIVE_RUNTIME_TYPES = frozenset({"container", "qemu", "lume", "qemu-docker", "managed"})


def _require_spacesd_for_layers(ref: str) -> None:
    """VM images take layers after boot through cua-spacesd: refuse one
    that does not run it (or whose registry cannot say) before booting."""
    from cua_sandbox._sdk import Unsupported

    kind, _, reference = ref.partition(":")
    backend = {"vm": "vm", "lume": "lume", "container": "container", "docker": "container"}
    has = None
    if kind in backend and reference:
        n = native()
        try:
            has = n.resolve_image(reference, backend[kind], None).spacesd
        except n.CuaError as error:
            logger.debug("could not read %s for cua-spacesd: %s", reference, error)
    if has is not True:
        what = "a VM image" if kind in ("vm", "lume", "disk") else "these layers"
        raise Unsupported(
            f"image layers on {what} ({ref}) are applied after boot through "
            "cua-spacesd, which this image does not run. Use a container image "
            "with pip_install/uv_install/apt_install/run/copy/env (they build locally), "
            "or bake them into the image"
        )


def is_native_state(state: Optional[dict]) -> bool:
    """Whether a state file was written by the SDK (it records ``services``)."""
    return bool(state) and "services" in state


def _status_word(status: Any) -> str:
    n = native()
    return {
        n.SandboxStatus.RUNNING: "running",
        n.SandboxStatus.SUSPENDED: "suspended",
        n.SandboxStatus.STOPPED: "stopped",
        n.SandboxStatus.PROVISIONING: "provisioning",
    }.get(status, "unknown")


def runtime_info_from_state(
    name: str, state: dict, *, handle: Any = None, environment: Optional[str] = None
) -> RuntimeInfo:
    """``RuntimeInfo`` from an SDK-written state file."""
    exposed = {int(g): int(h) for g, h in (state.get("exposed_ports") or {}).items()}
    token = state.get("env_token")
    # The SDK records `env` only when the image carries cua-spacesd (its
    # port is published); older state files have no services at all.
    services = state["services"] if "services" in state else {ENV_SERVICE: SPACESD_PORT}
    has_env = ENV_SERVICE in services
    env_guest = int(services.get(ENV_SERVICE) or SPACESD_PORT)
    api_port = (exposed.get(env_guest) or state.get("api_port") or 0) if has_env else 0
    info = RuntimeInfo(
        host=state.get("host") or "127.0.0.1",
        api_port=int(api_port),
        vnc_port=state.get("vnc_port"),
        qmp_port=state.get("qmp_port"),
        name=name,
        environment=environment or state.get("os_type"),
        guest_server_port=env_guest if has_env else None,
        exposed_ports={g: h for g, h in exposed.items() if g != env_guest or not has_env} or None,
        native=handle,
        env_token=token,
    )
    return info


class NativeRuntime(Runtime):
    """A local runtime backed by the SDK's ``Sandboxes`` (provider = local).

    Subclasses choose the backend with :meth:`_image_ref` (``container:<ref>``,
    ``vm:<ref>``, ``disk:<path>`` or ``lume:<ref>``).
    """

    #: runtime_type label kept in Python-facing listings.
    runtime_type = "local"
    #: The SDK provider (``ProviderKind`` member name) and ``on`` word.
    provider_kind = "LOCAL"
    on: Optional[str] = None
    #: How long the first interface call waits for cua-spacesd after boot.
    env_ready_timeout = 120.0

    def __init__(
        self,
        *,
        ephemeral: bool = True,
        cpus: Optional[int] = None,
        memory_mb: Optional[int] = None,
        ready_timeout: float = 900.0,
        server_port: Optional[int] = None,
        environment: Optional[dict[str, str]] = None,
    ) -> None:
        self.ephemeral = ephemeral
        self.cpus = cpus
        self.memory_mb = memory_mb
        self.ready_timeout = ready_timeout
        self.server_port = server_port
        self.environment = dict(environment or {})

    # ── subclass hooks ───────────────────────────────────────────────────

    async def _image_ref(self, image: Image, name: str, **opts: Any) -> str:
        raise NotImplementedError

    def _os(self, image: Image) -> str:
        return image.os_type or "linux"

    #: Whether this runtime mints the spacesd token itself (containers).
    #: Otherwise the SDK mints one for Linux VMs (cloud-init, the Fleet
    #: /run/cua/env-token contract) and macOS VMs (the Lume setup share) and
    #: keeps it in the sandbox record, which ``_info`` reads back.
    delivers_guest_env = False

    def _env_token(self) -> Optional[str]:
        """The spacesd token for a new sandbox.

        ``CUA_SANDBOX_ENV_TOKEN`` (a token baked into the image as
        /etc/cua/env-token) wins; otherwise a fresh per-sandbox token when the
        backend delivers it to the guest (libs/images ensure-env-token.sh reads
        CUA_ENV_TOKEN), else none.
        """
        configured = os.environ.get("CUA_SANDBOX_ENV_TOKEN")
        if configured:
            return configured
        return secrets.token_urlsafe(24) if self.delivers_guest_env else None

    # ── Runtime API ──────────────────────────────────────────────────────

    async def start(self, image: Image, name: str, **opts: Any) -> RuntimeInfo:
        ephemeral = opts.pop("ephemeral", True)
        # The portable sandbox options (Sandbox.create(command=, env=,
        # services=, wait_for=)).
        command = opts.pop("command", None)
        extra_env = dict(opts.pop("env", None) or {})
        services = dict(opts.pop("services", None) or {})
        wait_for = list(opts.pop("wait_for", None) or [])
        from cua_sandbox.containers import sidecars_of

        sidecars = sidecars_of(opts.pop("sidecars", None))
        # What kind and which engine (Sandbox.create(kind=, runtime=)); the
        # SDK checks them against the image prefix this adapter picks.
        kind = opts.pop("kind", None)
        legacy_engine = opts.pop("container_runtime", None)
        engine = opts.pop("runtime", None) or legacy_engine
        # "none" cuts guest egress (QEMU); the backend refuses it where it
        # cannot (containers, Lume). Unset: outbound network.
        network = opts.pop("network", None)
        n = native()
        ref = await self._image_ref(image, name, **opts)
        # Layers on a container image: a local build before boot (any
        # image). On a VM: applied after boot, which needs cua-spacesd.
        build = None
        if image._layers or image._files or image._env:
            from cua_sandbox.containers import buildable

            if ref.startswith(("container:", "docker:")) and buildable(image):
                from cua_sandbox.containers import image_build

                build = image_build(image, where="local")
            else:
                # A VM, or layers a container build cannot run (app_install,
                # brew_install, ...): after boot, through cua-spacesd.
                _require_spacesd_for_layers(ref)
        # 3211 is published (and `env` reported) only when the image carries
        # cua-spacesd: the SDK decides from the image, so `env` is implicit.
        ports = sorted({SPACESD_PORT, *image._ports, *services.values()})
        probes = [p.native() for p in wait_for]
        if self.server_port is not None:
            ports = sorted({*ports, self.server_port})
            probes.append(n.ReadinessProbe(port=self.server_port))
            services["server"] = self.server_port
        token = self._env_token()
        options = n.SandboxCreateOptions(
            on=self.on or "local",
            image=ref,
            name=name,
            token=token,
            os=self._os(image),
            cpus=opts.get("cpu_count") or self.cpus,
            memory_mb=opts.get("memory_mb") or self.memory_mb,
            ports=ports,
            services=services,
            wait_for=probes,
            ready_timeout_ms=millis(self.ready_timeout),
            env={
                **self.environment,
                **dict(image._env),
                **extra_env,
                **({"CUA_ENV_TOKEN": token} if token else {}),
            },
            command=list(command) if command else None,
            sidecars=[c.native() for c in sidecars],
            registry_secret=image._secret.native() if image._secret is not None else None,
            kind=kind,
            runtime=engine,
            network=network,
            build=build,
        )
        logger.info("Starting %s sandbox %r from %s", type(self).__name__, name, ref)
        try:
            handle = await local_runtime().sandboxes().create(options)
        except (n.CuaError.InvalidPlacement, n.CuaError.InvalidArgument) as error:
            from cua_sandbox._placement import translate

            raise translate(error) from None
        if token:
            _remember_token(name, token)
        info = self._info(name, handle, image)
        try:
            if build is None:
                await self._apply_layers(info, image)
        except BaseException:
            await self._delete_quietly(name)
            raise
        if ephemeral:
            logger.debug("sandbox %r is ephemeral; it is deleted on destroy()", name)
        return info

    def _info(self, name: str, handle: Any, image: Optional[Image]) -> RuntimeInfo:
        from cua_sandbox import sandbox_state

        state = sandbox_state.load(name) or {}
        if not state:
            record = handle.info()
            state = {
                "host": "127.0.0.1",
                "services": dict(record.services),
                "os_type": image.os_type if image else None,
            }
        info = runtime_info_from_state(
            name, state, handle=handle, environment=image.os_type if image else None
        )
        info.env_ready_timeout = self.env_ready_timeout
        return info

    async def _apply_layers(self, info: RuntimeInfo, image: Image) -> None:
        if not (image._layers or image._files or image._env):
            return
        from cua_sandbox.builder.build import _apply_env
        from cua_sandbox.builder.executor import LayerExecutor

        executor = await LayerExecutor.for_sandbox(
            info.native,
            os_type=image.os_type,
            ready_timeout=self.env_ready_timeout,
        )
        await _apply_env(executor, image)
        for src, dst in image._files:
            await executor.execute_layers([{"type": "copy", "src": src, "dst": dst}])
        if image._layers:
            await executor.execute_layers(list(image._layers))

    async def _handle(self, name: str) -> Any:
        return await local_runtime().sandboxes().connect(name)

    async def _delete_quietly(self, name: str) -> None:
        try:
            await local_runtime().sandboxes().delete(name)
        except Exception as error:  # noqa: BLE001 - cleanup after a failed start
            logger.warning("Failed to remove sandbox %r after a failed start: %s", name, error)

    async def stop(self, name: str) -> None:
        if self.ephemeral:
            await self.delete(name)
        else:
            await self.suspend(name)

    async def delete(self, name: str) -> None:
        await local_runtime().sandboxes().delete(name)

    async def suspend(self, name: str) -> None:
        await (await self._handle(name)).suspend()

    async def resume(self, image: Optional[Image], name: str, **opts: Any) -> RuntimeInfo:
        handle = await self._handle(name)
        await handle.resume()
        return self._info(name, await self._handle(name), image)

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        # Daemon-agnostic: the SDK returned the instance running (and passed
        # any declared server_port probe) before start() returned.
        handle = getattr(info, "native", None)
        if handle is not None and self.server_port is not None:
            n = native()
            await handle.wait_ready([n.ReadinessProbe(port=self.server_port)], millis(timeout))
        return True

    async def list(self) -> list[dict]:
        rows = []
        for record in await local_runtime().sandboxes().list("local"):
            if record.runtime_type not in NATIVE_RUNTIME_TYPES:
                continue
            rows.append(
                {
                    "name": record.name,
                    "status": _status_word(record.status),
                    "runtime_type": record.runtime_type,
                    "image": record.image,
                }
            )
        return rows


def _remember_token(name: str, token: str) -> None:
    """Keep the spacesd token with the sandbox record (owner-only file).

    The SDK holds tokens in memory only, so a later ``Sandbox.connect(name,
    local=True)`` from another process reads it from here.
    """
    from cua_sandbox import sandbox_state

    if sandbox_state.load(name) is None:
        return
    sandbox_state.update(name, env_token=token)
    try:
        os.chmod(sandbox_state._state_path(name), 0o600)
    except OSError:
        pass
