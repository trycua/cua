# Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.
from __future__ import annotations

from typing import Any, Literal, NotRequired, TypedDict

class ClaimSpec(TypedDict):
    bindDeadline: NotRequired[int]
    lifecycle: NotRequired[ClaimSpecLifecycle]
    sandboxTemplateRef: SandboxTemplateRef
    warmpool: NotRequired[str]

class ClaimSpecLifecycle(TypedDict):
    autoRenew: NotRequired[bool]
    shutdownPolicy: NotRequired[str]
    shutdownTime: NotRequired[str]

class PoolSpec(TypedDict):
    autoscaling: NotRequired[PoolSpecAutoscaling]
    replicas: int
    services: NotRequired[list[PoolSpecService]]
    template: PoolTemplate

class PoolSpecAutoscaling(TypedDict):
    initialPoolSize: NotRequired[int]
    maxPoolSize: NotRequired[int]
    minPoolSize: NotRequired[int]

class PoolSpecService(TypedDict):
    name: str
    protocol: NotRequired[PoolSpecServiceProtocol]
    targetPort: int

PoolSpecServiceProtocol = Literal["TCP", "UDP"]

class PoolTemplate(TypedDict):
    command: NotRequired[list[str]]
    containerDiskImage: str
    cpuCores: NotRequired[int]
    firmware: NotRequired[PoolTemplateFirmware]
    imagePullSecret: NotRequired[str]
    memory: NotRequired[str]
    nodeSelector: NotRequired[dict[str, str]]
    oidc: NotRequired[PoolTemplateOidc]
    probes: NotRequired[Any]
    runtime: NotRequired[PoolTemplateRuntime]
    runtimeClassName: NotRequired[str]
    tolerations: NotRequired[list[Any]]

PoolTemplateFirmware = Literal["bios", "efi"]

class PoolTemplateOidc(TypedDict):
    awsRegion: NotRequired[str]
    awsRoleArn: NotRequired[str]
    credentialsSecret: str
    refreshIntervalSeconds: NotRequired[int]
    tokenUrl: str

PoolTemplateRuntime = Literal["kubevirt", "macos", "gvisor"]

class SandboxTemplateRef(TypedDict):
    name: str

