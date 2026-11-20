"""e2b's template builder: `Template().from_...().run_cmd(...)...` and
`Template.build(template, name)`.

The classes live in the modules e2b has them in (`e2b.template.main`,
`.types`, `.logger`, `.readycmd`, `.utils`, `.dockerfile_parser`,
`e2b.template_async.main`, `e2b.template_sync.main`); this package re-exports
them so `from e2b.template import ...` works too. What a build does on Cua is
in `e2b.template._engine`.

The e2b SDK is MIT-licensed; this package matches its public API.
"""

from e2b.template.dockerfile_parser import DockerfileSyntaxError, parse_dockerfile
from e2b.template.logger import (
    DefaultBuildLogger,
    LogEntry,
    LogEntryEnd,
    LogEntryLevel,
    LogEntryStart,
    default_build_logger,
)
from e2b.template.main import (
    TemplateBase,
    TemplateBuilder,
    TemplateClass,
    TemplateFinal,
)
from e2b.template.readycmd import (
    ReadyCmd,
    wait_for_file,
    wait_for_port,
    wait_for_process,
    wait_for_timeout,
    wait_for_url,
)
from e2b.template.types import (
    AWSRegistry,
    BuildInfo,
    BuildStatusReason,
    CopyItem,
    GCPRegistry,
    GenericDockerRegistry,
    Instruction,
    InstructionType,
    RegistryConfig,
    TemplateBuildStatus,
    TemplateBuildStatusResponse,
    TemplateTag,
    TemplateTagInfo,
    TemplateType,
)

__all__ = [
    "AWSRegistry",
    "BuildInfo",
    "BuildStatusReason",
    "CopyItem",
    "DefaultBuildLogger",
    "DockerfileSyntaxError",
    "GCPRegistry",
    "GenericDockerRegistry",
    "Instruction",
    "InstructionType",
    "LogEntry",
    "LogEntryEnd",
    "LogEntryLevel",
    "LogEntryStart",
    "ReadyCmd",
    "RegistryConfig",
    "TemplateBase",
    "TemplateBuildStatus",
    "TemplateBuildStatusResponse",
    "TemplateBuilder",
    "TemplateClass",
    "TemplateFinal",
    "TemplateTag",
    "TemplateTagInfo",
    "TemplateType",
    "default_build_logger",
    "parse_dockerfile",
    "wait_for_file",
    "wait_for_port",
    "wait_for_process",
    "wait_for_timeout",
    "wait_for_url",
]
