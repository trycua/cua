"""Credential-free checks for the first Cloud Fleet tutorial and Python example."""

import asyncio
import hashlib
import importlib.metadata
import inspect
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock


DOCS = Path(__file__).resolve().parents[2]
PAGE = DOCS / "content/docs/tutorials/your-first-cloud-fleet.mdx"
RECOVERY = DOCS / "content/docs/how-to-guides/sandbox/recover-first-cloud-fleet.mdx"
PYTHON_SOURCE = DOCS / "public/scripts/first-cloud-fleet/first_cloud_fleet.py"
TYPESCRIPT_SOURCE = DOCS / "public/scripts/first-cloud-fleet/first-cloud-fleet.ts.txt"
POOL_NAME = "first-fleet-0123456789abcdef"
IMAGE_REF = "registry.example/cua-image@sha256:" + "1" * 64


class PythonExampleTests(unittest.TestCase):
    def setUp(self):
        self.files = {}
        self.file_api = SimpleNamespace(
            write_text=AsyncMock(side_effect=self.write_text),
            read_text=AsyncMock(side_effect=self.read_text),
        )
        self.shell = SimpleNamespace(run=AsyncMock(side_effect=self.run_command))
        self.sandbox = SimpleNamespace(files=self.file_api, shell=self.shell)
        self.claim = AsyncMock()
        self.claim.__aenter__.return_value = self.sandbox
        self.claim.__aexit__.return_value = False
        self.pool = SimpleNamespace(
            claim=Mock(return_value=self.claim),
            delete=AsyncMock(),
        )
        self.pool_api = SimpleNamespace(apply=AsyncMock(return_value=self.pool))
        fake_sdk = ModuleType("cua_sandbox")
        fake_sdk.Image = SimpleNamespace(from_registry=Mock(side_effect=lambda value: value))
        fake_sdk.Pool = self.pool_api
        self.addCleanup(sys.modules.pop, "cua_sandbox", None)
        sys.modules["cua_sandbox"] = fake_sdk
        self.namespace = {"__name__": "tutorial_test"}
        source = PYTHON_SOURCE.read_text()
        exec(compile(source, str(PYTHON_SOURCE), "exec"), self.namespace)

    async def write_text(self, path, content):
        self.files[path] = content

    async def read_text(self, path):
        return self.files[path]

    async def run_command(self, command):
        source_path = self.namespace["SOURCE_PATH"]
        result_path = self.namespace["RESULT_PATH"]
        self.assertIn("sha256sum", command)
        self.files[result_path] = hashlib.sha256(self.files[source_path].encode()).hexdigest() + "\n"
        return SimpleNamespace(success=True, stderr="")

    def test_success_uses_server_files_and_shell_then_cleans_up(self):
        asyncio.run(self.namespace["run_tutorial"](IMAGE_REF, POOL_NAME))

        apply = self.pool_api.apply.await_args
        self.assertEqual(apply.args, (IMAGE_REF,))
        self.assertEqual(apply.kwargs["name"], POOL_NAME)
        self.assertEqual(apply.kwargs["replicas"], 1)
        self.assertEqual(apply.kwargs["services"], {"server": 8000})
        self.pool.claim.assert_called_once_with(
            name="first-task", service="server", time_to_start=900
        )
        self.file_api.write_text.assert_awaited_once()
        self.shell.run.assert_awaited_once()
        self.file_api.read_text.assert_awaited_once()
        self.claim.__aexit__.assert_awaited_once()
        self.pool.delete.assert_awaited_once()

    def test_verification_failure_still_releases_claim_and_deletes_pool(self):
        self.file_api.read_text.side_effect = None
        self.file_api.read_text.return_value = "wrong-digest\n"
        with self.assertRaisesRegex(RuntimeError, "Verification failed"):
            asyncio.run(self.namespace["run_tutorial"](IMAGE_REF, POOL_NAME))
        self.claim.__aexit__.assert_awaited_once()
        self.pool.delete.assert_awaited_once()

    def test_claim_failure_still_deletes_pool(self):
        self.claim.__aenter__.side_effect = TimeoutError("synthetic readiness timeout")
        with self.assertRaisesRegex(TimeoutError, "readiness timeout"):
            asyncio.run(self.namespace["run_tutorial"](IMAGE_REF, POOL_NAME))
        self.pool.delete.assert_awaited_once()


class DocumentationContractTests(unittest.TestCase):
    def test_python_example_matches_the_published_sdk(self):
        from cua_sandbox import Image, Pool

        self.assertEqual(importlib.metadata.version("cua-sandbox"), "0.7.0")
        image = Image.from_registry(IMAGE_REF)
        inspect.signature(Pool.apply).bind(
            image,
            name=POOL_NAME,
            replicas=1,
            cpu=4,
            memory_mb=4096,
            services={"server": 8000},
        )

    def test_tutorial_has_persistent_language_tabs_and_both_examples(self):
        page = PAGE.read_text()
        self.assertGreaterEqual(
            page.count('<Tabs groupId="language" persist items={[\'Python\', \'TypeScript\']}>'),
            2,
        )
        self.assertIn("first_cloud_fleet.py", page)
        self.assertIn("first-cloud-fleet.ts", page)
        self.assertIn("cua-sandbox==0.7.0", page)
        self.assertIn("@trycua/fleet@0.1.2", page)

    def test_tutorial_requires_server_and_independent_verification(self):
        page = PAGE.read_text()
        python = PYTHON_SOURCE.read_text()
        typescript = TYPESCRIPT_SOURCE.read_text()
        self.assertIn('services={"server": 8000}', python)
        self.assertIn("services: [{ name: 'server', targetPort: 8000 }]", typescript)
        self.assertIn("hashlib.sha256", python)
        self.assertIn("createHash('sha256')", typescript)
        self.assertIn("does not rely on the guest shell to verify its own result", page)

    def test_tutorial_has_no_driver_prerequisite_or_unsupported_image_claim(self):
        page = PAGE.read_text()
        self.assertIn("does not require Cua Driver", page)
        self.assertNotIn("Omarchy", page)
        self.assertNotIn("Driver-compatible", page)
        self.assertNotIn("supported image", page.lower())

    def test_cleanup_is_honest_and_recovery_is_narrow(self):
        page = PAGE.read_text()
        recovery = RECOVERY.read_text()
        self.assertIn("can incur usage charges until it is", page)
        self.assertIn("accepted cleanup request can precede final removal", page)
        self.assertIn("/how-to-guides/sandbox/recover-first-cloud-fleet", page)
        self.assertIn("Do not rerun the", recovery)
        self.assertIn("exact `first-fleet-...` name", recovery)
        self.assertIn("account inventory until the exact pool name is absent", recovery)


if __name__ == "__main__":
    unittest.main()
