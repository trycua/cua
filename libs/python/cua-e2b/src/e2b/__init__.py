"""An e2b-compatible `e2b` module whose sandboxes are Cua Spaces.

Install `cua-e2b` in place of `e2b` and existing code keeps working::

    from e2b import AsyncSandbox, CommandExitException

    sandbox = await AsyncSandbox.create(template="my-world")  # a Cua checkpoint or image
    result = await sandbox.commands.run("uname -a", user="root")
    await sandbox.files.write("/tmp/x", b"hi")
    url = f"http://{sandbox.get_host(8080)}/"
    await sandbox.kill()

Each sandbox is a Space of kind `vm` (a Linux VM) placed with `on="auto"`
(``CUA_E2B_ON``) on your machines. See the README for configuration and the
calls that are not covered.
"""

from e2b.api import ApiClient, client
from e2b.client import E2B, E2BClientParams
from e2b.connection_config import (
    ApiParams,
    ConnectionConfig,
    HttpVersion,
    ProxyTypes,
    Username,
)
from e2b.exceptions import (
    AuthenticationException,
    BuildException,
    FileNotFoundException,
    FileUploadException,
    GitAuthException,
    GitUpstreamException,
    InvalidArgumentException,
    NotEnoughSpaceException,
    NotFoundException,
    RateLimitException,
    SandboxException,
    SandboxNotFoundException,
    SandboxNotRunningException,
    SandboxUnreachableException,
    SecretException,
    SecretNotFoundException,
    ServiceBusyException,
    TemplateException,
    TimeoutException,
    VolumeException,
    VolumeNotFoundException,
    VolumePathNotFoundException,
)
from e2b.sandbox.commands.command_handle import (
    CommandExitException,
    CommandResult,
    PtyOutput,
    PtySize,
    Stderr,
    Stdout,
)
from e2b.sandbox.commands.main import ProcessInfo
from e2b.sandbox.filesystem.filesystem import (
    AsyncFileStreamReader,
    EntryInfo,
    FileType,
    WriteEntry,
    WriteInfo,
)
from e2b.sandbox.filesystem.watch_handle import FilesystemEvent, FilesystemEventType
from e2b.sandbox.network import ALL_TRAFFIC
from e2b.sandbox.sandbox_api import (
    GitHubMcpServer,
    GitHubMcpServerConfig,
    McpServer,
    SandboxEgressProxyInfo,
    SandboxEgressProxyOpts,
    SandboxEgressProxyTLSInfo,
    SandboxEgressProxyTLSOpts,
    SandboxIamOpts,
    SandboxIamToken,
    SandboxIamTokenType,
    SandboxInfo,
    SandboxInfoLifecycle,
    SandboxLifecycle,
    SandboxListOrder,
    SandboxMetrics,
    SandboxNetworkInfo,
    SandboxNetworkOpts,
    SandboxNetworkRule,
    SandboxNetworkRuleInfo,
    SandboxNetworkRules,
    SandboxNetworkSelector,
    SandboxNetworkSelectorContext,
    SandboxNetworkTransform,
    SandboxNetworkTransformContext,
    SandboxNetworkTransformResolver,
    SandboxNetworkUpdate,
    SandboxOnResume,
    SandboxOnTimeout,
    SandboxQuery,
    SandboxState,
    SnapshotInfo,
    SnapshotMode,
)
from e2b.sandbox.signature import get_signature
from e2b.sandbox_async.commands.command_handle import AsyncCommandHandle
from e2b.sandbox_async.commands.command_input_handle import AsyncCommandInputHandle
from e2b.sandbox_async.filesystem.watch_handle import AsyncWatchHandle
from e2b.sandbox_async.git import (
    Git,
    GitBranches,
    GitFileStatus,
    GitResetMode,
    GitStatus,
)
from e2b.sandbox_async.main import AsyncSandbox
from e2b.sandbox_async.paginator import AsyncSandboxPaginator, AsyncSnapshotPaginator
from e2b.sandbox_async.utils import OutputHandler
from e2b.sandbox_sync.main import (
    CommandHandle,
    Sandbox,
    SandboxPaginator,
    SnapshotPaginator,
    WatchHandle,
)
from e2b.secret import (
    AsyncSecret,
    AsyncSecretPaginator,
    Secret,
    SecretInfo,
    SecretPaginator,
)
from e2b.template.dockerfile_parser import DockerfileSyntaxError
from e2b.template.logger import (
    LogEntry,
    LogEntryEnd,
    LogEntryLevel,
    LogEntryStart,
    default_build_logger,
)
from e2b.template.main import TemplateBase, TemplateClass
from e2b.template.readycmd import (
    ReadyCmd,
    wait_for_file,
    wait_for_port,
    wait_for_process,
    wait_for_timeout,
    wait_for_url,
)
from e2b.template.types import (
    BuildInfo,
    BuildStatusReason,
    CopyItem,
    TemplateBuildStatus,
    TemplateBuildStatusResponse,
    TemplateTag,
    TemplateTagInfo,
)
from e2b.template_async.main import AsyncTemplate
from e2b.template_sync.main import Template
from e2b.volume.connection_config import VolumeApiParams, VolumeConnectionConfig
from e2b.volume.types import (
    VolumeAndToken,
    VolumeEntryStat,
    VolumeEntryStatType,
    VolumeFileType,
    VolumeInfo,
)
from e2b.volume.volume_async import AsyncVolume
from e2b.volume.volume_sync import Volume

__version__ = "0.1.0"

__all__ = [
    "E2B",
    "E2BClientParams",
    "ApiClient",
    "ApiParams",
    "ConnectionConfig",
    "HttpVersion",
    "ProxyTypes",
    "Username",
    "client",
    "get_signature",
    "AsyncSecret",
    "AsyncSecretPaginator",
    "Secret",
    "SecretException",
    "SecretInfo",
    "SecretNotFoundException",
    "SecretPaginator",
    "GitHubMcpServer",
    "GitHubMcpServerConfig",
    "McpServer",
    "SandboxEgressProxyInfo",
    "SandboxEgressProxyOpts",
    "SandboxEgressProxyTLSInfo",
    "SandboxEgressProxyTLSOpts",
    "SandboxIamOpts",
    "SandboxIamToken",
    "SandboxIamTokenType",
    "SandboxInfoLifecycle",
    "SandboxLifecycle",
    "SandboxListOrder",
    "SandboxNetworkInfo",
    "SandboxNetworkOpts",
    "SandboxNetworkRule",
    "SandboxNetworkRuleInfo",
    "SandboxNetworkRules",
    "SandboxNetworkSelector",
    "SandboxNetworkSelectorContext",
    "SandboxNetworkTransform",
    "SandboxNetworkTransformContext",
    "SandboxNetworkTransformResolver",
    "SandboxNetworkUpdate",
    "SandboxOnResume",
    "SandboxOnTimeout",
    "SandboxState",
    "SnapshotMode",
    "BuildStatusReason",
    "CopyItem",
    "DockerfileSyntaxError",
    "LogEntryLevel",
    "TemplateClass",
    "TemplateTag",
    "TemplateTagInfo",
    "VolumeApiParams",
    "VolumeConnectionConfig",
    "AsyncTemplate",
    "AsyncVolume",
    "BuildInfo",
    "LogEntry",
    "LogEntryEnd",
    "LogEntryStart",
    "ReadyCmd",
    "Template",
    "TemplateBase",
    "TemplateBuildStatus",
    "TemplateBuildStatusResponse",
    "Volume",
    "VolumeAndToken",
    "VolumeEntryStat",
    "VolumeEntryStatType",
    "VolumeFileType",
    "VolumeInfo",
    "VolumeNotFoundException",
    "VolumePathNotFoundException",
    "default_build_logger",
    "wait_for_file",
    "wait_for_port",
    "wait_for_process",
    "wait_for_timeout",
    "wait_for_url",
    "Git",
    "GitBranches",
    "GitFileStatus",
    "GitResetMode",
    "GitStatus",
    "ALL_TRAFFIC",
    "AsyncSandboxPaginator",
    "AsyncSnapshotPaginator",
    "AsyncWatchHandle",
    "FilesystemEvent",
    "FilesystemEventType",
    "SandboxInfo",
    "SandboxMetrics",
    "SandboxPaginator",
    "SnapshotPaginator",
    "SandboxQuery",
    "WatchHandle",
    "AsyncCommandHandle",
    "AsyncCommandInputHandle",
    "AsyncFileStreamReader",
    "AsyncSandbox",
    "AuthenticationException",
    "BuildException",
    "CommandExitException",
    "CommandHandle",
    "CommandResult",
    "EntryInfo",
    "FileNotFoundException",
    "FileType",
    "FileUploadException",
    "GitAuthException",
    "GitUpstreamException",
    "InvalidArgumentException",
    "NotEnoughSpaceException",
    "NotFoundException",
    "OutputHandler",
    "ProcessInfo",
    "PtyOutput",
    "PtySize",
    "RateLimitException",
    "Sandbox",
    "SandboxException",
    "SandboxNotFoundException",
    "SandboxNotRunningException",
    "SandboxUnreachableException",
    "ServiceBusyException",
    "SnapshotInfo",
    "Stderr",
    "Stdout",
    "TemplateException",
    "TimeoutException",
    "VolumeException",
    "WriteEntry",
    "WriteInfo",
]
