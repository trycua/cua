"""Typed launch specifications for supported production agent harnesses.

These specifications are deliberately pure: rendering a launch does not read the
host environment or write configuration.  The environment adapter is responsible
for materializing the returned files inside the disposable harness home.
"""

from .production import (
    HarnessKind,
    HarnessRenderContext,
    LaunchContract,
    ModelRoute,
    NativeMcpDriver,
    NormalizedTelemetry,
    ProductionHarnessSpec,
    RenderedConfig,
    TelemetryTrust,
    production_harness,
)

__all__ = [
    "HarnessKind",
    "HarnessRenderContext",
    "LaunchContract",
    "ModelRoute",
    "NativeMcpDriver",
    "NormalizedTelemetry",
    "ProductionHarnessSpec",
    "RenderedConfig",
    "TelemetryTrust",
    "production_harness",
]
