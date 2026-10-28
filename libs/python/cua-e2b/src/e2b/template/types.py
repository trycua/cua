"""Types of the template builder (same names, fields and values as e2b's).

The e2b SDK is MIT-licensed; this module matches its public API.
"""

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import List, Literal, Optional, TypedDict, Union

from e2b.template.logger import LogEntry


class TemplateBuildStatus(str, Enum):
    """Status of a template build."""

    BUILDING = "building"
    WAITING = "waiting"
    READY = "ready"
    ERROR = "error"


@dataclass
class BuildStatusReason:
    """Why a build has its status (typically an error)."""

    message: str
    step: Optional[str] = None
    log_entries: List[LogEntry] = field(default_factory=list)


@dataclass
class TemplateBuildStatusResponse:
    """The state of a build."""

    build_id: str
    template_id: str
    status: TemplateBuildStatus
    log_entries: List[LogEntry]
    logs: List[str]
    reason: Optional[BuildStatusReason] = None


@dataclass
class TemplateTagInfo:
    """The tags assigned by `assign_tags` and the build they point at."""

    build_id: str
    tags: List[str]


@dataclass
class TemplateTag:
    """One tag of a template."""

    tag: str
    build_id: str
    created_at: datetime


class InstructionType(str, Enum):
    """The kinds of build instruction."""

    COPY = "COPY"
    ENV = "ENV"
    RUN = "RUN"
    WORKDIR = "WORKDIR"
    USER = "USER"


class _CopyItemRequired(TypedDict):
    src: Union[Union[str, Path], List[Union[str, Path]]]
    dest: Union[str, Path]


class CopyItem(_CopyItemRequired, total=False):
    """One copy operation: `src` and `dest`, with optional settings."""

    forceUpload: Optional[Literal[True]]
    user: Optional[str]
    mode: Optional[int]
    resolveSymlinks: Optional[bool]
    gzip: Optional[bool]


class _InstructionRequired(TypedDict):
    type: InstructionType
    args: List[str]
    force: bool


class Instruction(_InstructionRequired, total=False):
    """One step of a build."""

    forceUpload: Optional[Literal[True]]
    filesHash: Optional[str]
    resolveSymlinks: Optional[bool]
    gzip: Optional[bool]


class _GenericDockerRegistryRequired(TypedDict):
    type: Literal["registry"]
    username: str
    password: str


class GenericDockerRegistry(_GenericDockerRegistryRequired):
    """A registry with user name and password."""


class AWSRegistry(TypedDict):
    """AWS Elastic Container Registry credentials."""

    type: Literal["aws"]
    awsAccessKeyId: str
    awsSecretAccessKey: str
    awsRegion: str


class GCPRegistry(TypedDict):
    """Google Container Registry / Artifact Registry credentials."""

    type: Literal["gcp"]
    serviceAccountJson: str


RegistryConfig = Union[GenericDockerRegistry, AWSRegistry, GCPRegistry]


class _TemplateTypeRequired(TypedDict):
    steps: List[Instruction]
    force: bool


class TemplateType(_TemplateTypeRequired, total=False):
    """A template as the build receives it."""

    fromImage: str
    fromTemplate: str
    fromImageRegistry: RegistryConfig
    startCmd: str
    readyCmd: str


@dataclass
class BuildInfo:
    """The result of a build."""

    template_id: str
    build_id: str
    name: str
    # Deprecated: use name instead
    alias: str
    tags: List[str] = field(default_factory=list)
