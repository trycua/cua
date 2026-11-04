"""
Checkpoint utilities for the Tinker RL training loop.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from tinker import TrainingClient


# ---------------------------------------------------------------------------
# Data types
# ---------------------------------------------------------------------------


@dataclass
class CheckpointInfo:
    """Metadata returned by ``get_last_checkpoint``."""

    state_path: str
    epoch: int


# ---------------------------------------------------------------------------
# Save
# ---------------------------------------------------------------------------


def save_checkpoint(
    training_client: "TrainingClient",
    name: str,
    log_path: str,
    *,
    kind: str = "state",
    loop_state: Optional[dict] = None,
    ttl_seconds: Optional[int] = None,
) -> str:
    """Save training state with loop metadata.

    Parameters
    ----------
    training_client:
        Active Tinker ``TrainingClient``.
    name:
        Human-readable label (e.g. ``"epoch-5"`` or ``"final"``).
    log_path:
        Local directory where a ``checkpoints.json`` manifest is maintained.
    kind:
        ``"state"`` (weights + optimiser) or ``"weights"`` (weights only).
        Currently only ``"state"`` is implemented.
    loop_state:
        Arbitrary dict persisted alongside the checkpoint (e.g.
        ``{"epoch": 5}``).  Used by ``get_last_checkpoint`` for resume.
    ttl_seconds:
        Optional TTL forwarded to Tinker's ``save_state``.

    Returns
    -------
    A ``tinker://`` path string identifying the checkpoint.
    """
    save_kwargs: dict = {"name": name}
    if ttl_seconds is not None:
        save_kwargs["ttl_seconds"] = ttl_seconds

    state_path: str = training_client.save_state(**save_kwargs).result()
    print(f"[checkpoint] State saved: {state_path}")

    # Persist a local manifest so we can resume without querying Tinker.
    manifest_path = Path(log_path) / "checkpoints.json"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)

    manifest: list[dict] = []
    if manifest_path.exists():
        try:
            manifest = json.loads(manifest_path.read_text())
        except Exception:
            manifest = []

    entry = {
        "name": name,
        "state_path": state_path,
        "kind": kind,
        **(loop_state or {}),
    }
    manifest.append(entry)
    manifest_path.write_text(json.dumps(manifest, indent=2))

    return state_path


# ---------------------------------------------------------------------------
# Resume
# ---------------------------------------------------------------------------


def get_last_checkpoint(log_path: str) -> Optional[CheckpointInfo]:
    """Return the most recent checkpoint from the local manifest.

    Returns ``None`` if no checkpoints have been saved yet.
    """
    manifest_path = Path(log_path) / "checkpoints.json"
    if not manifest_path.exists():
        return None

    try:
        manifest = json.loads(manifest_path.read_text())
    except Exception:
        return None

    if not manifest:
        return None

    last = manifest[-1]
    return CheckpointInfo(
        state_path=last["state_path"],
        epoch=last.get("epoch", 0),
    )
