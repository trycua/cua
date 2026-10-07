"""Credential-free checks for the first Cloud Fleet tutorial and its Python example."""

import asyncio
import hashlib
import importlib.metadata
import inspect
import os
from pathlib import Path
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import AsyncMock, MagicMock, Mock


DOCS = Path(__file__).resolve().parents[2]
PAGE = DOCS / "content/docs/fleets/quickstart.mdx"
RECOVERY = DOCS / "content/docs/fleets/guides/troubleshoot.mdx"
PYTHON_SOURCE = DOCS / "public/scripts/first-cloud-fleet/first_cloud_fleet.py"
TYPESCRIPT_SOURCE = DOCS / "public/scripts/first-cloud-fleet/first-cloud-fleet.ts.txt"
IMAGE_REF = "registry.example/cua-image@sha256:" + "1" * 64
SANDBOX_VERSION = "0.9.0"


class PythonExampleTests(unittest.TestCase):
    def setUp(self):
        self.files = {}
        self.digest_override = None
        self.sandbox = SimpleNamespace(
            claim_name="cua-auto-test-claim",
            pool_name="cua-auto-test",
            files=SimpleNamespace(write_text=AsyncMock(side_effect=self.write_text)),
            shell=SimpleNamespace(run=AsyncMock(side_effect=self.run_command)),
            screenshot=AsyncMock(return_value=b"\x89PNG"),
        )
        self.context = MagicMock()
        self.context.__aenter__ = AsyncMock(return_value=self.sandbox)
        self.context.__aexit__ = AsyncMock(return_value=False)
        fake_sdk = ModuleType("cua_sandbox")
        fake_sdk.Image = SimpleNamespace(
            from_registry=Mock(side_effect=lambda value: value),
            linux=Mock(return_value="canonical-linux"),
        )
        fake_sdk.Sandbox = SimpleNamespace(ephemeral=Mock(return_value=self.context))
        self.fake_sdk = fake_sdk
        self.addCleanup(sys.modules.pop, "cua_sandbox", None)
        sys.modules["cua_sandbox"] = fake_sdk
        self.namespace = {"__name__": "tutorial_test"}
        exec(compile(PYTHON_SOURCE.read_text(), str(PYTHON_SOURCE), "exec"), self.namespace)
        cwd = os.getcwd()
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.addCleanup(os.chdir, cwd)
        os.chdir(tmp.name)

    async def write_text(self, path, content):
        self.files[path] = content

    async def run_command(self, command):
        self.assertIn("sha256sum", command)
        source = self.files[self.namespace["SOURCE_PATH"]]
        digest = self.digest_override or hashlib.sha256(source.encode()).hexdigest()
        return SimpleNamespace(success=True, stdout=f"{digest}  /tmp/x\n", stderr="")

    def test_success_uses_a_managed_ephemeral_sandbox(self):
        asyncio.run(self.namespace["run_tutorial"](IMAGE_REF))
        call = self.fake_sdk.Sandbox.ephemeral.call_args
        self.assertEqual(call.args, (IMAGE_REF,))
        self.assertIs(call.kwargs["local"], False)
        self.assertNotIn("pool", call.kwargs)
        self.sandbox.files.write_text.assert_awaited_once()
        self.sandbox.shell.run.assert_awaited_once()
        self.sandbox.screenshot.assert_awaited_once()
        self.context.__aexit__.assert_awaited_once()

    def test_default_image_is_the_canonical_linux_image(self):
        asyncio.run(self.namespace["run_tutorial"]())
        call = self.fake_sdk.Sandbox.ephemeral.call_args
        self.assertEqual(call.args, ("canonical-linux",))
        self.assertIs(call.kwargs["local"], False)

    def test_verification_failure_still_releases_the_claim(self):
        self.digest_override = "wrong-digest"
        with self.assertRaisesRegex(RuntimeError, "Verification failed"):
            asyncio.run(self.namespace["run_tutorial"](IMAGE_REF))
        self.context.__aexit__.assert_awaited_once()


class DocumentationContractTests(unittest.TestCase):
    def test_python_example_matches_the_published_sdk(self):
        try:
            from cua_sandbox import Image, Sandbox
        except ImportError:
            self.skipTest("cua-sandbox is not installed")
        self.assertEqual(importlib.metadata.version("cua-sandbox"), SANDBOX_VERSION)
        inspect.signature(Sandbox.ephemeral).bind(
            Image.from_registry(IMAGE_REF), local=False, cpu=4, memory_mb=4096
        )

    def test_page_versions_match_the_scripts(self):
        page = PAGE.read_text()
        self.assertIn(f"cua-sandbox=={SANDBOX_VERSION}", page)
        self.assertIn(f'"cua-sandbox=={SANDBOX_VERSION}"', PYTHON_SOURCE.read_text())
        self.assertIn("from '@trycua/cua'", TYPESCRIPT_SOURCE.read_text())
        self.assertIn("@trycua/cua@", page)
        self.assertGreaterEqual(
            page.count("<Tabs groupId=\"language\" persist items={['CLI', 'Python', 'TypeScript']}>"), 1
        )

    def test_verification_is_independent_of_the_guest(self):
        self.assertIn("hashlib.sha256", PYTHON_SOURCE.read_text())
        self.assertIn("createHash('sha256')", TYPESCRIPT_SOURCE.read_text())
        self.assertIn("does not rely on the guest", PAGE.read_text())

    def test_cleanup_and_recovery_are_documented(self):
        page = PAGE.read_text()
        recovery = RECOVERY.read_text()
        self.assertIn("cua sb rm", page)
        self.assertIn("/fleets/guides/troubleshoot#clean-up-leftovers", page)
        self.assertIn("cua fleet pools gc", recovery)
        self.assertNotIn("—", page + recovery)


if __name__ == "__main__":
    unittest.main()
