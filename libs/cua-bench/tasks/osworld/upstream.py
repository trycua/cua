"""The pinned upstream OSWorld tree (xlang-ai/OSWorld, Apache-2.0).

Tasks, ``SetupController``, the getters and the metrics are used as
upstream ships them: the tree at :data:`OSWORLD_COMMIT` is downloaded once
into ``~/.cache/cua-bench/osworld/<commit>`` (sha256-verified), put on
``sys.path`` and imported. Nothing is copied into cua-bench.

``desktop_env`` imports many heavy packages at module level (easyocr and
torch, librosa, OpenCV ...). :func:`import_desktop_env` imports it with
shims for any of them that is missing, so tasks whose evaluator needs none
of them run with ``pip install "cua-bench[osworld]"``; a task whose metric
does need one fails at evaluate with :class:`OSWorldDependencyError` naming
the package. ``pyautogui`` is always shimmed: on the host it would drive
this machine's real mouse, and inside the guest it is the OSWorld server's
business.

``CUA_BENCH_OSWORLD_ROOT`` points at an existing checkout instead (offline,
tests).
"""

from __future__ import annotations

import contextlib
import hashlib
import importlib
import importlib.abc
import importlib.machinery
import json
import os
import shutil
import sys
import tarfile
import tempfile
import types
import urllib.request
from pathlib import Path
from typing import Any, Iterator, Optional

OSWORLD_REPO = "https://github.com/xlang-ai/OSWorld"
OSWORLD_COMMIT = "b138d348256078fa634fc3b73567a7337c793e6b"
TARBALL_URL = f"https://codeload.github.com/xlang-ai/OSWorld/tar.gz/{OSWORLD_COMMIT}"
TARBALL_SHA256 = "fc430ab878d4a3c1b1ad8f614456dcb771b6e3aa508ef5f0878cee585ffb5b5b"
ROOT_ENV = "CUA_BENCH_OSWORLD_ROOT"

#: Always shimmed, installed or not (host safety, see the module docstring).
ALWAYS_SHIM = frozenset({"pyautogui", "pyscreeze", "pymsgbox", "pytweening", "mouseinfo"})


class OSWorldDependencyError(ImportError):
    """An OSWorld getter or metric needs a package that is not installed."""


def cache_root() -> Path:
    base = os.environ.get("CUA_BENCH_CACHE") or os.path.join(
        os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "cua-bench"
    )
    return Path(base) / "osworld"


def ensure_tree(root: Optional[str] = None) -> Path:
    """The OSWorld tree: ``root``/``CUA_BENCH_OSWORLD_ROOT``, else the pinned download."""
    root = root or os.environ.get(ROOT_ENV)
    if root:
        path = Path(root).expanduser()
        if not (path / "desktop_env").is_dir():
            raise FileNotFoundError(f"{path}: not an OSWorld checkout (no desktop_env/)")
        return path
    dest = cache_root() / OSWORLD_COMMIT
    marker = dest / ".cua-bench-verified"
    if marker.is_file() and marker.read_text().strip() == TARBALL_SHA256:
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=dest.parent) as tmp:
        tarball = Path(tmp) / "osworld.tar.gz"
        req = urllib.request.Request(TARBALL_URL, headers={"User-Agent": "cua-bench"})
        with urllib.request.urlopen(req, timeout=300) as r, open(tarball, "wb") as f:
            shutil.copyfileobj(r, f, 1 << 20)
        got = hashlib.sha256(tarball.read_bytes()).hexdigest()
        if got != TARBALL_SHA256:
            raise RuntimeError(f"{TARBALL_URL}: sha256 {got} != pinned {TARBALL_SHA256}")
        out = Path(tmp) / "x"
        with tarfile.open(tarball) as t:
            t.extractall(out, filter="data")
        (top,) = list(out.iterdir())
        if dest.exists():
            shutil.rmtree(dest)
        top.rename(dest)
    marker.write_text(TARBALL_SHA256 + "\n")
    return dest


# ── tasks ─────────────────────────────────────────────────────────────────

SPLITS = ("test_all", "test_small", "test_nogdrive", "test_infeasible", "parity")


def task_index(root: Path, split: str, parity: list[str]) -> list[tuple[str, str]]:
    """``(domain, task_id)`` pairs of a split (``parity``: the listed ids)."""
    ex = root / "evaluation_examples"
    if split == "parity":
        full = json.loads((ex / "test_all.json").read_text())
        where = {tid: dom for dom, ids in full.items() for tid in ids}
        missing = [t for t in parity if t not in where]
        if missing:
            raise KeyError(f"parity tasks not in test_all.json: {missing}")
        return [(where[t], t) for t in parity]
    if split not in SPLITS:
        raise ValueError(f"unknown OSWorld split {split!r} (use one of {', '.join(SPLITS)})")
    data = json.loads((ex / f"{split}.json").read_text())
    return [(dom, tid) for dom, ids in data.items() for tid in ids]


def load_task(root: Path, domain: str, task_id: str) -> dict:
    return json.loads((root / "evaluation_examples" / "examples" / domain / f"{task_id}.json").read_text())


# ── importing desktop_env with shims ──────────────────────────────────────


class _MissingMeta(type):
    def __getattr__(cls, name: str) -> Any:
        if name.startswith("__"):
            raise AttributeError(name)
        return _missing(f"{cls._cb_name}.{name}", cls._cb_module)

    def __call__(cls, *a: Any, **k: Any) -> Any:
        raise OSWorldDependencyError(
            f"{cls._cb_name} is unavailable: the OSWorld evaluator needs the "
            f"{cls._cb_module!r} package (pip install \"cua-bench[osworld]\" or {cls._cb_module})"
        )


def _missing(name: str, module: str) -> type:
    return _MissingMeta(name.rsplit(".", 1)[-1], (), {"_cb_name": name, "_cb_module": module})


class _ShimModule(types.ModuleType):
    def __init__(self, name: str) -> None:
        super().__init__(name)
        self.__path__ = []  # a package, so submodule imports reach the finder
        self.__cua_bench_shim__ = True

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__"):
            raise AttributeError(name)
        return _missing(f"{self.__name__}.{name}", self.__name__.split(".")[0])


class _ShimLoader(importlib.abc.Loader):
    def create_module(self, spec):  # noqa: D401
        return _ShimModule(spec.name)

    def exec_module(self, module) -> None:
        pass


class _ShimFinder(importlib.abc.MetaPathFinder):
    """Shims ``always`` names first, and any other missing module when last."""

    def __init__(self, always: frozenset, missing_ok: bool) -> None:
        self.always = always
        self.missing_ok = missing_ok
        self.made: set[str] = set()

    def find_spec(self, fullname, path=None, target=None):
        top = fullname.split(".")[0]
        if top == "desktop_env":
            return None
        if top in self.always or (self.missing_ok and _imported_by_desktop_env()):
            self.made.add(fullname)
            return importlib.machinery.ModuleSpec(fullname, _ShimLoader())
        return None


def _imported_by_desktop_env() -> bool:
    """Whether the code importing right now is desktop_env's (not a library
    probing an optional dependency, which must keep getting ImportError)."""
    frame = sys._getframe(2)
    for _ in range(64):
        if frame is None:
            return False
        name = frame.f_code.co_filename
        if "importlib" in name or name.startswith("<frozen") or name == __file__:
            frame = frame.f_back
            continue
        return str(frame.f_globals.get("__name__", "")).startswith("desktop_env")
    return False


@contextlib.contextmanager
def _shims() -> Iterator[_ShimFinder]:
    front = _ShimFinder(ALWAYS_SHIM, missing_ok=False)
    back = _ShimFinder(frozenset(), missing_ok=True)
    saved = {n: sys.modules.pop(n) for n in list(sys.modules) if n.split(".")[0] in ALWAYS_SHIM}
    sys.meta_path.insert(0, front)
    sys.meta_path.append(back)
    try:
        yield back
    finally:
        sys.meta_path.remove(front)
        sys.meta_path.remove(back)
        # desktop_env keeps its references; nothing else sees the shims.
        for n in list(sys.modules):
            if getattr(sys.modules[n], "__cua_bench_shim__", False):
                del sys.modules[n]
        sys.modules.update(saved)


_LOADED: dict[str, Any] = {}


def import_desktop_env(root: Path) -> types.SimpleNamespace:
    """``desktop_env`` modules: ``setup``, ``python``, ``getters``, ``metrics``,
    ``DesktopEnv``, plus ``shimmed`` (the missing packages)."""
    key = str(root)
    if key in _LOADED:
        return _LOADED[key]
    for n in [n for n in sys.modules if n == "desktop_env" or n.startswith("desktop_env.")]:
        del sys.modules[n]
    if str(root) not in sys.path:
        sys.path.insert(0, str(root))
    with _shims() as back:
        setup = importlib.import_module("desktop_env.controllers.setup")
        python = importlib.import_module("desktop_env.controllers.python")
        getters = importlib.import_module("desktop_env.evaluators.getters")
        metrics = importlib.import_module("desktop_env.evaluators.metrics")
        de = importlib.import_module("desktop_env.desktop_env")
        shimmed = sorted({m.split(".")[0] for m in back.made})
    ns = types.SimpleNamespace(
        setup=setup, python=python, getters=getters, metrics=metrics,
        DesktopEnv=de.DesktopEnv, shimmed=shimmed,
    )
    _LOADED[key] = ns
    return ns
