"""Sidecar containers, private-registry credentials and cloud image builds.

The same options mean the same thing locally and in the cloud:

* ``Sandbox.create(image, sidecars=[Container("redis:7-alpine", ports=[6379], name="db")])``
  runs extra containers next to the sandbox, addressed by name on every
  runtime: the sandbox reaches a sidecar at its name (``db:6379``), a sidecar
  reaches the sandbox at ``main``, and ``services={"db": 6379}`` names the
  port for use from outside. Local containers share the sandbox's network
  namespace (``localhost`` works too) and run on runc: pass ``runtime="runc"``
  (where gVisor would run, the SDK refuses the group, since separate gVisor
  containers cannot share a network namespace). Cloud gVisor sandboxes run
  them in the same pod; cloud VM (KubeVirt) sandboxes run them in a companion
  pod named in the guest's ``/etc/hosts``. Local VM sandboxes refuse sidecars.
  With sidecars the service names ``main``, ``sidecars`` and ``sc`` are
  reserved.
* ``Image.from_registry(ref, secret=RegistrySecret(...))`` pulls a private
  image: locally with the credentials, in the cloud through a registry pull
  secret the SDK writes for the sandbox. Credentials are never logged or
  saved in ``~/.cua``.
* Image layers (``pip_install``, ``run``, ``copy``, ``env``, ...) with
  ``local=False`` build remotely on the image as base, cached by content.

Everything here converts to the native ``cua`` SDK types; the Rust core does
the work.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Iterable, Mapping, Optional, Sequence, Union

if TYPE_CHECKING:  # pragma: no cover
    from cua_sandbox.image import Image


@dataclass(frozen=True)
class Container:
    """A sidecar container, reachable from the sandbox at its ``name``.

    ``name`` (a DNS label, not ``main``) defaults from the image (``redis``
    for ``redis:7-alpine``); the sidecar reaches the sandbox at ``main``.
    ``command`` replaces the image's entrypoint. ``env`` holds plain values,
    not secrets.
    """

    image: str
    command: Optional[Sequence[str]] = None
    env: Optional[Mapping[str, str]] = None
    ports: Optional[Sequence[int]] = None
    name: Optional[str] = None

    def __post_init__(self) -> None:
        if not isinstance(self.image, str) or not self.image.strip():
            raise ValueError("Container needs an image reference")
        for port in self.ports or ():
            if isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
                raise ValueError(f"Container port must be 1-65535, not {port!r}")

    def native(self) -> Any:
        from cua_sandbox._sdk import native

        return native().Container(
            image=self.image,
            command=list(self.command) if self.command else None,
            env=dict(self.env or {}),
            ports=[int(p) for p in (self.ports or ())],
            name=self.name,
        )


SidecarLike = Union[Container, str]


def sidecars_of(sidecars: Optional[Iterable[SidecarLike]]) -> list[Container]:
    """Normalize ``sidecars=``: a :class:`Container` or a bare image string."""
    out: list[Container] = []
    for s in sidecars or ():
        if isinstance(s, Container):
            out.append(s)
        elif isinstance(s, str):
            out.append(Container(s))
        else:
            raise TypeError(f"sidecars take Container(...) or an image string, not {s!r}")
    return out


class RegistrySecret:
    """Credentials for a private registry image.

    * ``RegistrySecret(username, password)``: explicit values (a token works
      as the password).
    * ``RegistrySecret.from_env()``: read ``CUA_REGISTRY_USERNAME`` /
      ``CUA_REGISTRY_PASSWORD`` (or the variables you name) at create time.
    * ``RegistrySecret.aws_ecr(region=None)``: a private Amazon ECR image,
      with a login token from the AWS CLI.

    ``registry`` scopes them to one host; by default they apply to the
    image's registry. The password never appears in ``repr`` or logs.
    """

    __slots__ = ("_kind", "_a", "_b", "registry")

    def __init__(self, username: str, password: str, *, registry: Optional[str] = None) -> None:
        if not username or not password:
            raise ValueError("RegistrySecret needs a username and a password")
        self._kind = "basic"
        self._a = username
        self._b = password
        self.registry = registry

    @classmethod
    def from_env(
        cls,
        username_var: str = "CUA_REGISTRY_USERNAME",
        password_var: str = "CUA_REGISTRY_PASSWORD",
        *,
        registry: Optional[str] = None,
    ) -> "RegistrySecret":
        secret = cls.__new__(cls)
        secret._kind = "env"
        secret._a = username_var
        secret._b = password_var
        secret.registry = registry
        return secret

    @classmethod
    def aws_ecr(cls, region: Optional[str] = None) -> "RegistrySecret":
        secret = cls.__new__(cls)
        secret._kind = "aws_ecr"
        secret._a = region
        secret._b = None
        secret.registry = None
        return secret

    @property
    def username(self) -> Optional[str]:
        return self._a if self._kind == "basic" else None

    def native(self) -> Any:
        from cua_sandbox._sdk import native

        n = native().RegistrySecret
        if self._kind == "basic":
            return n.BASIC(username=self._a, password=self._b, registry=self.registry)
        if self._kind == "env":
            return n.FROM_ENV(username_var=self._a, password_var=self._b, registry=self.registry)
        return n.AWS_ECR(region=self._a)

    def __repr__(self) -> str:
        if self._kind == "basic":
            return f"RegistrySecret(username={self._a!r}, password='<redacted>')"
        if self._kind == "env":
            return f"RegistrySecret.from_env({self._a!r}, {self._b!r})"
        return f"RegistrySecret.aws_ecr(region={self._a!r})"

    def __eq__(self, other: object) -> bool:
        return isinstance(other, RegistrySecret) and (
            self._kind,
            self._a,
            self._b,
            self.registry,
        ) == (other._kind, other._a, other._b, other.registry)

    def __hash__(self) -> int:
        return hash((self._kind, self._a, self.registry))


#: Layer types a cloud build (a container image on a registry base) runs.
_CLOUD_LAYERS = {"apt_install", "pip_install", "uv_install", "run"}


def has_build(image: "Image") -> bool:
    """Whether *image* carries layers, files or environment to build."""
    return bool(image._layers or image._files or image._env)


def buildable(image: "Image") -> bool:
    """Whether every layer of *image* can run in a container image build."""
    return all(layer.get("type") in _CLOUD_LAYERS for layer in image._layers)


def image_build(image: "Image", *, where: str = "cloud") -> Any:
    """The native ``ImageBuild`` for *image*'s layers (``None`` without any).

    Builds make a container image on the registry base, in the cloud (a
    remote build) or on this machine (``where="local"``: the container
    engine), cached by the same content hash. VM-only and other-OS layers
    (``app_install``, ``brew_install``, ``winget_install``, ...) are refused
    with :class:`~cua_sandbox.Unsupported`.
    """
    if not has_build(image):
        return None
    from cua_sandbox._sdk import Unsupported, native

    n = native()
    layers = []
    for layer in image._layers:
        kind = layer.get("type")
        if kind not in _CLOUD_LAYERS:
            place = "in the cloud" if where == "cloud" else "into a container image"
            raise Unsupported(
                f"{kind} layers do not build {place} (a build makes a Linux container "
                "image); use pip_install, uv_install, apt_install, run, copy or env"
            )
        if kind == "apt_install":
            layers.append(n.ImageLayer.APT_INSTALL(packages=list(layer["packages"])))
        elif kind == "pip_install":
            layers.append(n.ImageLayer.PIP_INSTALL(packages=list(layer["packages"])))
        elif kind == "uv_install":
            layers.append(n.ImageLayer.UV_INSTALL(packages=list(layer["packages"])))
        else:
            layers.append(n.ImageLayer.RUN(command=str(layer["command"])))
    return n.ImageBuild(
        layers=layers,
        env=dict(image._env),
        ports=[int(p) for p in image._ports],
        files=[n.BuildFile(source=str(src), destination=str(dst)) for src, dst in image._files],
        timeout_ms=None,
    )


__all__ = ["Container", "RegistrySecret", "image_build", "has_build", "sidecars_of"]
