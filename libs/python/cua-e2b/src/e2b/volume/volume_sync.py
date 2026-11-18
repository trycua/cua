"""e2b volumes on Cua Volume (blocking): `Volume` is `AsyncVolume` with
blocking calls on the cua-e2b event loop; see `e2b.volume.volume_async` for
how a volume is kept.

The e2b SDK is MIT-licensed; this module matches its public API.
"""

from __future__ import annotations

from typing import IO, Iterator, List, Literal, Optional, Union, overload

from e2b.connection_config import ClientFactory, HttpVersion, ProxyTypes
from e2b.exceptions import NotFoundException
from e2b.sandbox_sync.main import _Loop
from e2b.volume.connection_config import VolumeConnectionConfig
from e2b.volume.types import VolumeAndToken, VolumeEntryStat, VolumeInfo
from e2b.volume.utils import DualMethod
from e2b.volume.volume_async import AsyncVolume


class Volume(ClientFactory):
    """A named volume: `Volume.create("data")`."""

    def __init__(
        self,
        volume_id: str,
        name: str,
        token: Optional[str] = None,
        domain: Optional[str] = None,
        debug: Optional[bool] = None,
        proxy: Optional[ProxyTypes] = None,
        *,
        http_version: Optional[HttpVersion] = None,
    ):
        self._volume_id = volume_id
        self._name = name
        self._token = token
        self._domain = domain
        self._debug = debug
        self._proxy = proxy
        self._http_version = http_version
        self._inner = AsyncVolume(
            volume_id, name, token, domain, debug, proxy, http_version=http_version
        )

    @property
    def volume_id(self) -> str:
        return self._volume_id

    @property
    def name(self) -> str:
        return self._name

    @property
    def token(self) -> Optional[str]:
        return self._token

    def _get_volume_config(self, **opts) -> VolumeConnectionConfig:
        return self._inner._get_volume_config(**opts)

    @classmethod
    def _wrap(cls, inner: AsyncVolume) -> "Volume":
        return cls(
            volume_id=inner.volume_id,
            name=inner.name,
            token=inner.token,
            domain=inner._domain,
            debug=inner._debug,
            proxy=inner._proxy,
            http_version=inner._http_version,
        )

    @classmethod
    def create(cls, name: str, **opts) -> "Volume":
        """Creates a volume (a folder of Cua Volume); an existing one of that
        name is returned as it is."""
        params = cls._resolve_api_params(**opts)
        return cls._wrap(_Loop.run(AsyncVolume.create(name, **params)))

    @classmethod
    def connect(cls, volume_id: str, **opts) -> "Volume":
        """Connects to an existing volume by its id.

        :raises VolumeNotFoundException: when there is none
        """
        params = cls._resolve_api_params(**opts)
        return cls._wrap(_Loop.run(AsyncVolume.connect(volume_id, **params)))

    @classmethod
    def _class_get_info(cls, volume_id: str, **opts) -> VolumeAndToken:
        """Information about a volume.

        :raises VolumeNotFoundException: when there is none
        """
        params = cls._resolve_api_params(**opts)
        return _Loop.run(AsyncVolume._class_get_info(volume_id, **params))

    @classmethod
    def _class_list(cls, **opts) -> List[VolumeInfo]:
        """All volumes."""
        params = cls._resolve_api_params(**opts)
        return _Loop.run(AsyncVolume._class_list(**params))

    @classmethod
    def destroy(cls, volume_id: str, **opts) -> bool:
        """Deletes a volume with all its files. `False`: there was none."""
        params = cls._resolve_api_params(**opts)
        return _Loop.run(AsyncVolume.destroy(volume_id, **params))

    def _instance_list(
        self, path: str, depth: Optional[int] = None, **opts
    ) -> List[VolumeEntryStat]:
        """Lists a directory (`depth` layers deep, default 1)."""
        return _Loop.run(self._inner._instance_list(path, depth, **opts))

    def make_dir(
        self,
        path: str,
        uid: Optional[int] = None,
        gid: Optional[int] = None,
        mode: Optional[int] = None,
        force: Optional[bool] = None,
        **opts,
    ) -> VolumeEntryStat:
        """Creates a directory; `force` creates missing parent directories."""
        return _Loop.run(self._inner.make_dir(path, uid, gid, mode, force, **opts))

    def exists(self, path: str, **opts) -> bool:
        """Whether a file or directory exists at `path`."""
        try:
            self.get_info(path, **opts)
            return True
        except NotFoundException:
            return False

    def _instance_get_info(self, path: str, **opts) -> VolumeEntryStat:
        """The stat of a file or directory."""
        return _Loop.run(self._inner._instance_get_info(path, **opts))

    get_info = DualMethod(_class_get_info, _instance_get_info)
    list = DualMethod(_class_list, _instance_list)

    def update_metadata(
        self,
        path: str,
        uid: Optional[int] = None,
        gid: Optional[int] = None,
        mode: Optional[int] = None,
        **opts,
    ) -> VolumeEntryStat:
        """Changes the uid, gid or mode kept for a file or directory."""
        return _Loop.run(self._inner.update_metadata(path, uid, gid, mode, **opts))

    @overload
    def read_file(self, path: str, format: Literal["text"] = "text", **opts) -> str: ...

    @overload
    def read_file(self, path: str, format: Literal["bytes"], **opts) -> bytes: ...

    @overload
    def read_file(
        self,
        path: str,
        format: Literal["stream"],
        stream_idle_timeout: Optional[float] = None,
        **opts,
    ) -> Iterator[bytes]: ...

    def read_file(
        self,
        path: str,
        format: Literal["text", "bytes", "stream"] = "text",
        stream_idle_timeout: Optional[float] = None,
        **opts,
    ) -> Union[str, bytes, Iterator[bytes]]:
        """Reads a file as `"text"`, `"bytes"` or a `"stream"` (an iterator of
        chunks; errors surface when it is first iterated)."""
        out = _Loop.run(self._inner.read_file(path, format, stream_idle_timeout, **opts))
        if format != "stream":
            return out

        def chunks() -> Iterator[bytes]:
            while True:
                try:
                    yield _Loop.run(out.__anext__())  # type: ignore[union-attr]
                except StopAsyncIteration:
                    return

        return chunks()

    def write_file(
        self,
        path: str,
        data: Union[str, bytes, IO],
        uid: Optional[int] = None,
        gid: Optional[int] = None,
        mode: Optional[int] = None,
        force: Optional[bool] = None,
        **opts,
    ) -> VolumeEntryStat:
        """Writes a file; `force` creates missing parent directories."""
        return _Loop.run(self._inner.write_file(path, data, uid, gid, mode, force, **opts))

    def remove(self, path: str, **opts) -> None:
        """Removes a file, or a directory with everything in it."""
        return _Loop.run(self._inner.remove(path, **opts))


__all__ = ["Volume"]
