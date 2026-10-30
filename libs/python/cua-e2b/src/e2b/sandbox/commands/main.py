"""ProcessInfo, as in e2b.sandbox.commands.main."""

from dataclasses import dataclass
from typing import Dict, List, Optional


@dataclass
class ProcessInfo:
    """A command running in the sandbox."""

    pid: int
    tag: Optional[str]
    cmd: str
    args: List[str]
    envs: Dict[str, str]
    cwd: Optional[str]
