"""Run the tutorial snippets with released SDK imports and mocked Fleet calls.

Install cua-sandbox==0.4.3 (requires cua-fleet==0.1.14), then run this file.
No credentials, cloud provisioning, or local desktop actions are used.
"""

import asyncio
import importlib.metadata
import inspect
import json
import os
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch

from cua_sandbox import Pool
from fleet_sdk import SdkError


PAGE = Path(__file__).resolve().parents[2] / "content/docs/tutorials/your-first-cloud-fleet.mdx"
SOURCE = re.search(r'```python title="first_cloud_fleet.py"\n(.*?)\n```', PAGE.read_text(), re.S)[1]
CREATED_AT = "2026-01-01T00:00:00Z"
POOL_NAME = "first-fleet-" + "1" * 32


def missing():
    return SdkError.Status(operation="get pool", status=404, body="not found")


class TutorialTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.namespace = {"__name__": "tutorial_test"}
        exec(compile(SOURCE, str(PAGE), "exec"), self.namespace)
        self.namespace["RECORD"] = self.root / "cloud-fleet-run.json"
        self.namespace["uuid4"] = lambda: SimpleNamespace(hex="1" * 32)
        self.namespace["Path"] = lambda name: self.root / name
        self.sandbox = SimpleNamespace(
            name="tutorial-guest",
            shell=SimpleNamespace(
                run=AsyncMock(
                    return_value=SimpleNamespace(
                        success=True,
                        stdout="Linux tutorial-guest\n",
                        stderr="",
                    )
                )
            ),
            screenshot=AsyncMock(return_value=b"synthetic screenshot"),
        )
        self.claim = AsyncMock()
        self.claim.__aenter__.return_value = self.sandbox
        self.claim.__aexit__.return_value = False
        self.pool = SimpleNamespace(
            name=POOL_NAME,
            resource=SimpleNamespace(metadata=SimpleNamespace(creation_timestamp=CREATED_AT)),
            claim=Mock(return_value=self.claim),
            delete=AsyncMock(),
        )
        self.api = SimpleNamespace(
            get=AsyncMock(side_effect=missing()),
            apply=AsyncMock(return_value=self.pool),
        )
        self.namespace["Pool"] = self.api
        self.make_client = self.namespace["fleet_client"]
        self.reservation = SimpleNamespace(name=POOL_NAME, created_at=CREATED_AT)
        self.client = SimpleNamespace(
            create_namespace=AsyncMock(return_value=self.reservation),
            list_namespaces=AsyncMock(side_effect=[[self.reservation], []]),
            delete_namespace=AsyncMock(),
        )
        self.namespace["fleet_client"] = lambda: self.client

    def record(self, created_at=CREATED_AT):
        self.namespace["RECORD"].write_text(
            json.dumps(
                {
                    "name": POOL_NAME,
                    "created_at": created_at,
                }
            )
        )

    def test_released_versions_and_public_signatures(self):
        self.assertEqual(importlib.metadata.version("cua-sandbox"), "0.4.3")
        self.assertEqual(importlib.metadata.version("cua-fleet"), "0.1.14")
        self.assertIn('"cua-sandbox==0.4.3"', SOURCE)
        self.assertIn('"cua-fleet==0.1.14"', SOURCE)
        inspect.signature(Pool.apply).bind(
            self.namespace["Image"].from_registry(self.namespace["IMAGE"]),
            name=POOL_NAME,
            replicas=1,
            cpu=4,
            memory_mb=4096,
            services={"server": 8000},
            ttl_seconds_after_created=3600,
        )
        inspect.signature(Pool.get).bind(POOL_NAME)
        inspect.signature(Pool.claim).bind(
            self.pool,
            name="first-claim",
            service="server",
            time_to_start=900,
        )

    def test_success_saves_result_releases_claim_and_deletes(self):
        asyncio.run(self.namespace["run"]())
        self.assertEqual((self.root / "cloud-fleet.png").read_bytes(), b"synthetic screenshot")
        self.sandbox.shell.run.assert_awaited_once_with("uname -a")
        self.claim.__aexit__.assert_awaited_once()
        self.client.create_namespace.assert_awaited_once_with(POOL_NAME)
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)
        self.api.get.assert_not_awaited()
        record = json.loads(self.namespace["RECORD"].read_text())
        self.assertEqual(record, {"name": POOL_NAME, "created_at": CREATED_AT})
        self.assertEqual(self.api.apply.call_args.kwargs["ttl_seconds_after_created"], 3600)

    def test_existing_record_cannot_be_overwritten(self):
        self.record()
        before = self.namespace["RECORD"].read_bytes()
        with self.assertRaises(FileExistsError):
            asyncio.run(self.namespace["run"]())
        self.assertEqual(self.namespace["RECORD"].read_bytes(), before)
        self.api.get.assert_not_awaited()
        self.api.apply.assert_not_awaited()
        self.client.create_namespace.assert_not_awaited()

    def test_name_collision_does_not_apply_claim_or_delete(self):
        self.client.create_namespace.side_effect = SdkError.Status(
            operation="create namespace", status=409, body="synthetic collision"
        )
        with self.assertRaises(SdkError.Status):
            asyncio.run(self.namespace["run"]())
        self.api.apply.assert_not_awaited()
        self.pool.claim.assert_not_called()
        self.client.delete_namespace.assert_not_awaited()

    def test_reservation_access_denial_is_not_absence(self):
        self.client.create_namespace.side_effect = SdkError.Status(
            operation="create namespace",
            status=403,
            body="synthetic denial",
        )
        with self.assertRaises(SdkError.Status):
            asyncio.run(self.namespace["run"]())
        self.api.apply.assert_not_awaited()
        self.client.delete_namespace.assert_not_awaited()

    def test_admission_failure_deletes_only_confirmed_reservation(self):
        self.api.apply.side_effect = RuntimeError("synthetic admission denial")
        with self.assertRaisesRegex(RuntimeError, "admission denial"):
            asyncio.run(self.namespace["run"]())
        self.pool.claim.assert_not_called()
        self.pool.delete.assert_not_awaited()
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)

    def test_unconfirmed_reservation_refuses_cleanup(self):
        self.record(created_at=None)
        with self.assertRaisesRegex(RuntimeError, "not confirmed"):
            asyncio.run(self.namespace["cleanup"]())
        self.client.list_namespaces.assert_not_awaited()
        self.client.delete_namespace.assert_not_awaited()

    def test_claim_failure_deletes_created_pool(self):
        self.claim.__aenter__.side_effect = TimeoutError("synthetic readiness timeout")
        with self.assertRaises(TimeoutError):
            asyncio.run(self.namespace["run"]())
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)

    def test_workload_failure_releases_claim_and_deletes(self):
        self.sandbox.shell.run.return_value = SimpleNamespace(
            success=False,
            stdout="",
            stderr="synthetic command failure",
        )
        with self.assertRaisesRegex(RuntimeError, "command failure"):
            asyncio.run(self.namespace["run"]())
        self.claim.__aexit__.assert_awaited_once()
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)
        self.sandbox.screenshot.assert_not_awaited()

    def test_screenshot_failure_still_deletes(self):
        self.sandbox.screenshot.side_effect = RuntimeError("synthetic screenshot failure")
        with self.assertRaisesRegex(RuntimeError, "screenshot failure"):
            asyncio.run(self.namespace["run"]())
        self.claim.__aexit__.assert_awaited_once()
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)

    def test_cleanup_deletes_only_recorded_namespace_without_running_workload(self):
        self.record()
        asyncio.run(self.namespace["cleanup"]())
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)
        self.api.apply.assert_not_awaited()
        self.pool.claim.assert_not_called()
        self.api.get.assert_not_awaited()

    def test_repeated_cleanup_of_absent_namespace_is_safe(self):
        self.record()
        self.client.list_namespaces.side_effect = None
        self.client.list_namespaces.return_value = []
        asyncio.run(self.namespace["cleanup"]())
        asyncio.run(self.namespace["cleanup"]())
        self.client.delete_namespace.assert_not_awaited()
        self.api.apply.assert_not_awaited()

    def test_cleanup_refuses_a_replacement_namespace(self):
        self.record(created_at="2025-01-01T00:00:00Z")
        with self.assertRaisesRegex(RuntimeError, "identity changed"):
            asyncio.run(self.namespace["cleanup"]())
        self.client.delete_namespace.assert_not_awaited()

    def test_cleanup_error_retains_record_for_retry(self):
        self.record()
        self.client.delete_namespace.side_effect = RuntimeError("synthetic connection loss")
        with self.assertRaisesRegex(RuntimeError, "connection loss"):
            asyncio.run(self.namespace["cleanup"]())
        self.assertTrue(self.namespace["RECORD"].exists())

    def test_deletion_poll_is_bounded_and_reports_timeout(self):
        self.record()
        self.client.list_namespaces.side_effect = None
        self.client.list_namespaces.return_value = [self.reservation]
        with patch.object(asyncio, "sleep", new_callable=AsyncMock) as sleep:
            with self.assertRaises(TimeoutError):
                asyncio.run(self.namespace["cleanup"]())
        self.assertEqual(sleep.await_count, 60)
        self.assertTrue(self.namespace["RECORD"].exists())

    def test_inventory_denial_is_not_successful_cleanup(self):
        self.record()
        self.client.list_namespaces.side_effect = SdkError.Status(
            operation="list namespaces", status=403, body="synthetic denial"
        )
        with self.assertRaises(SdkError.Status):
            asyncio.run(self.namespace["cleanup"]())
        self.client.delete_namespace.assert_not_awaited()

    def test_inventory_failure_after_delete_is_not_absence(self):
        self.record()
        self.client.list_namespaces.side_effect = [
            [self.reservation],
            SdkError.Status(operation="list namespaces", status=403, body="synthetic denial"),
        ]
        with self.assertRaises(SdkError.Status):
            asyncio.run(self.namespace["cleanup"]())
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)

    def test_unrelated_namespaces_are_not_deleted(self):
        self.record()
        other = SimpleNamespace(name="another-project", created_at=CREATED_AT)
        self.client.list_namespaces.side_effect = [[other, self.reservation], [other]]
        asyncio.run(self.namespace["cleanup"]())
        self.client.delete_namespace.assert_awaited_once_with(POOL_NAME)

    def test_client_credentials_use_the_documented_endpoint(self):
        sdk = self.namespace["CyclopsClient"]
        credentials = Mock(wraps=self.namespace["CyclopsCredentials"])
        with (
            patch.dict(
                os.environ,
                {
                    "CUA_CLIENT_ID": "synthetic-client",
                    "CUA_CLIENT_SECRET": "synthetic-secret",
                    "CUA_TOKEN_URL": "https://auth.example/token",
                },
                clear=True,
            ),
            patch.object(sdk, "connect_with_native_http_client") as connect,
            patch.dict(self.namespace, {"CyclopsCredentials": credentials}),
        ):
            self.assertIs(self.make_client(), connect.return_value)
            configuration = connect.call_args.args[0]
            self.assertEqual(configuration.base_url, "https://run.cua.ai")
            self.assertEqual(configuration.token_url, "https://auth.example/token")
            credentials.assert_called_once_with("synthetic-client", "synthetic-secret")

    def test_static_token_takes_precedence_for_namespace_operations(self):
        sdk = self.namespace["CyclopsClient"]
        with (
            patch.dict(
                os.environ,
                {
                    "FLEETS_TOKEN": "synthetic-token",
                    "CUA_FLEET_BASE_URL": "https://fleet.example",
                },
                clear=True,
            ),
            patch.object(sdk, "connect_with_access_token_and_native_http_client") as connect,
        ):
            self.assertIs(self.make_client(), connect.return_value)
            configuration, token = connect.call_args.args
            self.assertEqual(configuration.base_url, "https://fleet.example")
            self.assertEqual(token, "synthetic-token")


if __name__ == "__main__":
    unittest.main()
