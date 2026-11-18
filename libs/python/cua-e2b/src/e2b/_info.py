"""A `SandboxInfo` from what the backend lists about a sandbox; shared by
`AsyncSandbox.get_info(id)` and the listing paginator so both report a
sandbox the same way."""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Optional

from e2b._backend import MachineInfo
from e2b.sandbox.sandbox_api import SandboxInfo, SandboxState

ENVD_VERSION = "cua-spacesd"
DEFAULT_CPUS = 2  # a Space's default size when nothing was recorded
DEFAULT_MEMORY_MB = 4096


def parse_ts(v) -> Optional[datetime]:
    try:
        return datetime.fromisoformat(v) if v else None
    except ValueError:
        return None


def network_info(egress):
    """The `network` of a SandboxInfo: only the lists that are set, and
    `allow_public_traffic` (on unless the sandbox was created with it off)."""
    if not egress:
        return None
    out = {}
    if egress.get("allow"):
        out["allow_out"] = list(egress["allow"])
    if egress.get("deny"):
        out["deny_out"] = list(egress["deny"])
    out["allow_public_traffic"] = True
    return out


def sandbox_info(m: MachineInfo) -> SandboxInfo:
    rec = m.extra or {}
    started = m.started_at or datetime.now(timezone.utc)
    lc = rec.get("lifecycle") or {}
    return SandboxInfo(
        sandbox_id=m.sandbox_id,
        sandbox_domain=None,
        template_id=m.template or "",
        name=m.template or "base",
        metadata=dict(rec.get("metadata") or {}),
        started_at=started,
        end_at=parse_ts(rec.get("end_at")) or started,
        state=SandboxState(m.state),
        cpu_count=int(rec.get("cpu_count") or m.cpus or DEFAULT_CPUS),
        memory_mb=int(rec.get("memory_mb") or m.memory_mb or DEFAULT_MEMORY_MB),
        envd_version=ENVD_VERSION,
        allow_internet_access=rec.get("allow_internet_access"),
        network=network_info(rec.get("egress")),
        lifecycle={
            "on_timeout": lc.get("on_timeout", "kill"),
            "auto_resume": bool(lc.get("auto_resume") or False),
        },
    )
