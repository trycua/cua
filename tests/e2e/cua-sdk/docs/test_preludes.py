"""The docs cleanup preludes delete only what the block created (stdlib only).

    python3 -m unittest discover -s tests/e2e/cua-sdk/docs -p 'test_*.py'

The Python `local-cleanup` prelude runs against a stand-in `cua_sandbox`
that already holds a reader's sandbox; the block creates one of its own, and
cleanup must delete exactly that one. Every prelude that deletes anything is
also checked to never list sandboxes (list-then-delete is the bug class).
"""

from __future__ import annotations

import asyncio
import json
import re
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path

PRELUDES = Path(__file__).resolve().parent / "preludes"


def _stub_cua_sandbox(existing: list[str]) -> tuple[types.ModuleType, list[str]]:
    deleted: list[str] = []
    live = list(existing)

    class _Sb:
        def __init__(self, name: str) -> None:
            self.name = name

    class Sandbox:
        @classmethod
        async def create(cls, image=None, *, name=None, **_):
            return await cls._create(image=image, name=name or "generated-name")

        @classmethod
        async def _create(cls, *, image=None, name=None, **_):
            live.append(name)
            return _Sb(name)

        @classmethod
        async def list(cls, **_):
            return [_Sb(n) for n in live]

        @classmethod
        async def delete(cls, name, **_):
            if name not in live:
                raise RuntimeError(f"not found: {name}")
            live.remove(name)
            deleted.append(name)

    mod = types.ModuleType("cua_sandbox")
    mod.Sandbox = Sandbox
    return mod, deleted


class LocalCleanupPython(unittest.TestCase):
    def setUp(self) -> None:
        self._saved = sys.modules.get("cua_sandbox")

    def tearDown(self) -> None:
        if self._saved is None:
            sys.modules.pop("cua_sandbox", None)
        else:
            sys.modules["cua_sandbox"] = self._saved

    def _prelude(self, existing: list[str]):
        mod, deleted = _stub_cua_sandbox(existing)
        sys.modules["cua_sandbox"] = mod
        ns: dict = {"__name__": "docs_block"}
        registered = []
        import atexit

        real = atexit.register
        atexit.register = lambda f, *a, **k: registered.append(f) or f  # type: ignore[assignment]
        try:
            exec(
                compile((PRELUDES / "local-cleanup.py").read_text(), "local-cleanup.py", "exec"), ns
            )
        finally:
            atexit.register = real  # type: ignore[assignment]
        self.assertEqual(len(registered), 1)
        return mod, deleted, registered[0]

    def test_deletes_only_what_the_block_created(self) -> None:
        mod, deleted, cleanup = self._prelude(["readers-desktop", "another-runs-box"])

        async def block() -> None:
            await mod.Sandbox.create("img", name="made-by-block")
            await mod.Sandbox.create("img")  # a generated name

        asyncio.run(block())
        cleanup()
        self.assertEqual(sorted(deleted), ["generated-name", "made-by-block"])

    def test_nothing_created_nothing_deleted(self) -> None:
        _, deleted, cleanup = self._prelude(["readers-desktop"])
        cleanup()
        self.assertEqual(deleted, [])

    def test_a_block_that_deleted_its_own_is_fine(self) -> None:
        mod, deleted, cleanup = self._prelude(["readers-desktop"])

        async def block() -> None:
            await mod.Sandbox.create("img", name="gone")
            await mod.Sandbox.delete("gone")

        asyncio.run(block())
        deleted.clear()
        cleanup()  # "not found" is not an error, and nothing else is touched
        self.assertEqual(deleted, [])


STUB_CUA = """
export const live = ['readers-desktop', 'another-runs-box'];
export const deleted = [];
export class Sandboxes {
  async create(options) {
    live.push(options.name);
    return { name: () => options.name };
  }
  async list() { return live.map((name) => ({ name })); }
  async delete_(name) {
    const i = live.indexOf(name);
    if (i < 0) throw new Error(`not found: ${name}`);
    live.splice(i, 1);
    deleted.push(name);
  }
}
export function embedded() { return { sandboxes: () => new Sandboxes() }; }
"""

BLOCK_TS = """
import { deleted, embedded } from '@trycua/cua';
await embedded().sandboxes().create({ name: 'made-by-block' });
process.on('exit', () => console.log(JSON.stringify(deleted)));
"""


@unittest.skipUnless(shutil.which("node"), "node not installed")
class LocalCleanupTypeScript(unittest.TestCase):
    def test_deletes_only_what_the_block_created(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            pkg = root / "node_modules" / "@trycua" / "cua"
            pkg.mkdir(parents=True)
            (pkg / "package.json").write_text(
                json.dumps({"name": "@trycua/cua", "type": "module", "main": "index.js"})
            )
            (pkg / "index.js").write_text(STUB_CUA)
            (root / "package.json").write_text('{"type":"module"}')
            prelude = (PRELUDES / "local-cleanup.ts").read_text()
            (root / "block.ts").write_text(prelude + "\n" + BLOCK_TS)
            r = subprocess.run(
                ["node", "--experimental-strip-types", "--no-warnings", "block.ts"],
                cwd=root,
                capture_output=True,
                text=True,
                timeout=60,
            )
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(json.loads(r.stdout.strip().splitlines()[-1]), ["made-by-block"])


class NoListThenDelete(unittest.TestCase):
    DELETE = re.compile(r"\.delete_?\(|\.destroy\(|docker['\"]?,?\s*\[?['\"]?rm|\brm -f\b")
    LIST = re.compile(r"\.list\(|list_all|listAll|\bsb ls\b|docker ps")

    def test_no_prelude_lists_and_deletes(self) -> None:
        offenders = []
        for path in sorted(PRELUDES.glob("*.*")):
            if path.suffix not in (".py", ".ts"):
                continue
            code = "\n".join(
                line
                for line in path.read_text().splitlines()
                if not line.lstrip().startswith(("#", "//"))
            )
            if self.DELETE.search(code) and self.LIST.search(code):
                offenders.append(path.name)
        self.assertEqual(offenders, [], "preludes must delete only what they created")

    def test_the_ts_prelude_tracks_creates(self) -> None:
        code = (PRELUDES / "local-cleanup.ts").read_text()
        self.assertIn("prototype.create", code)
        self.assertIn("__cuaDocsCreated", code)


if __name__ == "__main__":
    unittest.main()
