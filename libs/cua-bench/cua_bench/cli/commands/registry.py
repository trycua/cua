"""Dataset registry helpers for the CLI (see :mod:`cua_bench.registry`).

``name`` or ``name@version`` resolves through the versioned registry: the
pinned commit is fetched once (shallow, sparse) into ``~/.cua/cbregistry``.
"""

from pathlib import Path
from typing import Optional

RED = "\033[91m"
GREY = "\033[90m"
RESET = "\033[0m"


def resolve_dataset(dataset_name: str) -> tuple[Optional[Path], Optional[str]]:
    """``name[@version]`` to (local dataset directory, default image).

    The default image is the registry entry's desktop image for tasks that
    name none (None when the entry has none). ``(None, None)`` if unknown.
    """
    from cua_bench.registry import RegistryError, resolve_entry

    try:
        path, entry = resolve_entry(dataset_name)
    except RegistryError as error:
        print(f"{RED}Error: {error}{RESET}")
        return None, None
    return path, (entry.default_image if entry else None)


def resolve_dataset_path(dataset_name: str, update_registry: bool = True) -> Optional[Path]:
    """Resolve ``name[@version]`` to a local dataset directory (None if unknown).

    ``update_registry`` is kept for 0.2.x callers: pinned versions never change,
    so there is nothing to update.
    """
    return resolve_dataset(dataset_name)[0]


def resolve_task_path(
    dataset_name: str, task_name: str, update_registry: bool = True
) -> Optional[Path]:
    """Resolve a task within a registry dataset to its local path."""
    dataset_path = resolve_dataset_path(dataset_name, update_registry=update_registry)
    if dataset_path is None:
        return None
    task_path = dataset_path / task_name
    return task_path if task_path.exists() else None
