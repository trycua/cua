"""The metadata record of a secret."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from typing import Dict


@dataclass
class SecretInfo:
    """Metadata of a secret. Secret values are write-only and never returned."""

    secret_id: str
    """Secret ID."""
    name: str
    """Secret name, unique within the project."""
    version: int
    """Version served to readers that do not name one."""
    metadata: Dict[str, str]
    """Customer metadata of the secret."""
    created_at: datetime
    """Time when the secret was created."""
    updated_at: datetime
    """Time when the secret was last updated."""
