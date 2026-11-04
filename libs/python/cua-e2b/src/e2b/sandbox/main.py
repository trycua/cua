"""`SandboxBase`: what `Sandbox` and `AsyncSandbox` share, with e2b's attributes.

It holds the identity of a sandbox (id, domain, connection config, tokens) and the
pure URL builders on top of it: `get_host`, `download_url`, `upload_url`,
`get_mcp_url`, with signed file URLs as e2b builds them (see
`e2b.sandbox.signature`). The envd version is compared as dotted numbers; a value
that is not one (cua-e2b reports `"cua-spacesd"`) counts as newer than every
release, so the modern behaviour (default user chosen by the sandbox) applies.
"""

from __future__ import annotations

import urllib.parse
from typing import Any, Optional, Tuple, TypedDict

from e2b.connection_config import (
    ClientFactory,
    ConnectionConfig,
    default_username,
)
from e2b.exceptions import InvalidArgumentException
from e2b.sandbox.signature import get_signature

ENVD_API_FILES_ROUTE = "/files"
ENVD_DEFAULT_USER = (0, 4, 0)
"""First envd version that picks the default user itself."""


def _version_tuple(version: Any) -> Tuple[int, ...]:
    """A dotted version as a tuple of ints; anything else is newer than any
    release."""
    try:
        return tuple(int(p) for p in str(version).split("+")[0].split("."))
    except ValueError:
        return (99, 99, 99)


class SandboxOpts(TypedDict):
    sandbox_id: str
    sandbox_domain: Optional[str]
    envd_version: Any
    envd_access_token: Optional[str]
    traffic_access_token: Optional[str]
    connection_config: ConnectionConfig


class SandboxBase(ClientFactory):
    mcp_port = 50005

    default_sandbox_timeout = 300

    default_template = "base"
    default_mcp_template = "mcp-gateway"

    def __init__(
        self,
        sandbox_id: str,
        envd_version: Any,
        envd_access_token: Optional[str],
        sandbox_domain: Optional[str],
        connection_config: ConnectionConfig,
        traffic_access_token: Optional[str] = None,
    ):
        self.__connection_config = connection_config
        self.__sandbox_id = sandbox_id
        self.__sandbox_domain = sandbox_domain or self.connection_config.domain
        self.__envd_version = envd_version
        self.__envd_access_token = envd_access_token
        self.__traffic_access_token = traffic_access_token
        self.__envd_api_url = self.connection_config.get_sandbox_url(
            self.sandbox_id, self.sandbox_domain
        )
        self.__envd_direct_url = self.connection_config.get_sandbox_direct_url(
            self.sandbox_id, self.sandbox_domain
        )
        self.__mcp_token: Optional[str] = None

    @property
    def _envd_access_token(self) -> Optional[str]:
        """The envd token (private)."""
        return self.__envd_access_token

    @property
    def _mcp_token(self) -> Optional[str]:
        return self.__mcp_token

    @_mcp_token.setter
    def _mcp_token(self, token: str) -> None:
        self.__mcp_token = token

    @property
    def connection_config(self) -> ConnectionConfig:
        return self.__connection_config

    @property
    def _envd_version(self) -> Any:
        return self.__envd_version

    @property
    def traffic_access_token(self) -> Optional[str]:
        return self.__traffic_access_token

    @property
    def sandbox_domain(self) -> str:
        return self.__sandbox_domain

    @property
    def envd_api_url(self) -> str:
        return self.__envd_api_url

    @property
    def envd_direct_url(self) -> str:
        return self.__envd_direct_url

    @property
    def sandbox_id(self) -> str:
        """Unique identifier of the sandbox."""
        return self.__sandbox_id

    def _file_url(
        self,
        path: str,
        user: Optional[str] = None,
        signature: Optional[str] = None,
        signature_expiration: Optional[int] = None,
    ) -> str:
        url = urllib.parse.urljoin(self.envd_direct_url, ENVD_API_FILES_ROUTE)
        query = {"path": path} if path else {}

        if user:
            query["username"] = user

        if signature:
            query["signature"] = signature

        if signature_expiration:
            if signature is None:
                raise ValueError("signature_expiration requires signature to be set")
            query["signature_expiration"] = str(signature_expiration)

        params = urllib.parse.urlencode(
            query,
            quote_via=urllib.parse.quote,
        )
        url = urllib.parse.urljoin(url, f"?{params}")

        return url

    def _signed_file_url(
        self,
        operation: str,
        path: str,
        user: Optional[str],
        use_signature_expiration: Optional[int],
    ) -> str:
        use_signature = self._envd_access_token is not None
        if not use_signature and use_signature_expiration is not None:
            raise InvalidArgumentException(
                "Signature expiration can be used only when sandbox is created as secured."
            )

        username = user
        if username is None and _version_tuple(self._envd_version) < ENVD_DEFAULT_USER:
            username = default_username

        if use_signature:
            signature = get_signature(
                path,
                operation,  # type: ignore[arg-type]
                username,
                self._envd_access_token,
                use_signature_expiration,
            )
            return self._file_url(path, username, signature["signature"], signature["expiration"])
        return self._file_url(path, username)

    def download_url(
        self,
        path: str,
        user: Optional[str] = None,
        use_signature_expiration: Optional[int] = None,
    ) -> str:
        """The URL to download a file from the sandbox.

        :param path: Path to the file to download
        :param user: User to download the file as
        :param use_signature_expiration: Expiration time of the signed URL in seconds
        """
        return self._signed_file_url("read", path, user, use_signature_expiration)

    def upload_url(
        self,
        path: str,
        user: Optional[str] = None,
        use_signature_expiration: Optional[int] = None,
    ) -> str:
        """The URL to upload a file to the sandbox: send a POST with the file as
        multipart/form-data.

        :param path: Path to the file to upload
        :param user: User to upload the file as
        :param use_signature_expiration: Expiration time of the signed URL in seconds
        """
        return self._signed_file_url("write", path, user, use_signature_expiration)

    def get_host(self, port: int) -> str:
        """The host address to reach `port` of the sandbox from outside it.

        :param port: Port to connect to
        """
        return self.connection_config.get_host(self.sandbox_id, self.sandbox_domain, port)

    def get_mcp_url(self) -> str:
        """The MCP URL of the sandbox."""
        return f"https://{self.get_host(self.mcp_port)}/mcp"
