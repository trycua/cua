import copy
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch

import image_build


class JsonDocument:
    def __init__(self, document):
        self.document = document

    @classmethod
    def from_json(cls, value):
        return cls(json.loads(value))

    def to_json(self):
        return json.dumps(self.document)


def manifest():
    return {
        "apiVersion": "images.cua.ai/v1alpha1",
        "kind": "Image",
        "metadata": {"namespace": "workers", "name": "example.v1"},
        "spec": {"recipe": {"osType": "linux", "distro": "ubuntu", "version": "24.04", "kind": "vm", "layers": [{"type": "run", "command": "true"}]}},
    }


def observation(phase="Ready", observed_generation=1):
    document = manifest()
    document["metadata"].update(uid="image-uid", generation=1)
    document["status"] = {
        "phase": phase,
        "observedGeneration": observed_generation,
        "conditions": [{"type": "Ready", "status": "True", "observedGeneration": observed_generation}],
        "artifacts": {
            "oci": {"reference": "registry.example/images@sha256:" + "a" * 64, "digest": "sha256:" + "a" * 64},
            "volumeSnapshot": {"namespace": "workers", "name": "snapshot"},
        },
    }
    return document


class ImageBuildExampleTest(unittest.IsolatedAsyncioTestCase):
    def test_access_token_uses_native_transport_without_oauth_configuration(self):
        constructors = SimpleNamespace(connect_with_access_token_and_native_http_client=Mock(return_value="token-client"), connect_with_native_http_client=Mock())
        sdk = SimpleNamespace(CyclopsClient=constructors, CyclopsConfiguration=Mock(), CyclopsCredentials=Mock(), CyclopsTokenProviderConfiguration=lambda **values: values)
        with patch.dict("os.environ", {"CUA_BASE_URL": "https://fleet.example", "CUA_ACCESS_TOKEN": "test-token"}, clear=True), patch.dict(sys.modules, {"fleet_sdk": sdk}):
            self.assertEqual(image_build.connect_client(), "token-client")
        self.assertEqual(constructors.connect_with_access_token_and_native_http_client.call_args.args[1], "test-token")
        constructors.connect_with_native_http_client.assert_not_called()

    def test_client_credentials_use_native_transport(self):
        constructors = SimpleNamespace(connect_with_access_token_and_native_http_client=Mock(), connect_with_native_http_client=Mock(return_value="oauth-client"))
        sdk = SimpleNamespace(CyclopsClient=constructors, CyclopsConfiguration=lambda **values: values, CyclopsCredentials=Mock(return_value="credentials"), CyclopsTokenProviderConfiguration=Mock())
        environment = {"CUA_BASE_URL": "https://fleet.example", "CUA_TOKEN_URL": "https://identity.example/token", "CUA_CLIENT_ID": "client", "CUA_CLIENT_SECRET": "test-secret"}
        with patch.dict("os.environ", environment, clear=True), patch.dict(sys.modules, {"fleet_sdk": sdk}):
            self.assertEqual(image_build.connect_client(), "oauth-client")
        constructors.connect_with_access_token_and_native_http_client.assert_not_called()
        sdk.CyclopsCredentials.assert_called_once_with("client", "test-secret")

    async def test_upload_composes_source_then_creates_without_mutating_input(self):
        original = manifest()
        client = SimpleNamespace(
            upload_image_file=AsyncMock(return_value=SimpleNamespace(reference="uploads/tenant-x/hash", digest="sha256:" + "a" * 64, size_bytes=3)),
            create_image=AsyncMock(return_value=JsonDocument(observation("Pending"))),
            get_image=AsyncMock(),
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.txt"
            path.write_bytes(b"abc")
            with patch.dict(sys.modules, {"fleet_sdk": SimpleNamespace(PreservedJson=JsonDocument)}):
                result = await image_build.run_image_build(client, original, [(path, "/opt/input.txt")], wait_seconds=0)
        client.upload_image_file.assert_awaited_once_with("workers", "input.txt", b"abc")
        created = client.create_image.await_args.args[1].document
        self.assertEqual(created["spec"]["recipe"]["files"][0]["source"]["sizeBytes"], 3)
        self.assertEqual(created["spec"]["recipe"]["files"][0]["destination"], "/opt/input.txt")
        self.assertNotIn("files", original["spec"]["recipe"])
        self.assertEqual(result["metadata"]["uid"], "image-uid")
        client.get_image.assert_not_awaited()

    async def test_upload_failure_never_creates_image(self):
        client = SimpleNamespace(upload_image_file=AsyncMock(side_effect=RuntimeError("upload failure")), create_image=AsyncMock())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.txt"
            path.write_bytes(b"abc")
            with patch.dict(sys.modules, {"fleet_sdk": SimpleNamespace(PreservedJson=JsonDocument)}):
                with self.assertRaises(RuntimeError):
                    await image_build.run_image_build(client, manifest(), [(path, "/opt/input.txt")], wait_seconds=0)
        client.create_image.assert_not_awaited()

    async def test_poll_ignores_stale_ready_generation(self):
        client = SimpleNamespace(get_image=AsyncMock(side_effect=[JsonDocument(observation(observed_generation=0)), JsonDocument(observation())]))
        result = await image_build.wait_for_image(client, observation("Pending"), 1, poll_seconds=0.001)
        self.assertEqual(result["status"]["observedGeneration"], 1)
        self.assertEqual(client.get_image.await_count, 2)

    async def test_poll_fails_on_replacement_or_update(self):
        for field, value in [("uid", "replacement"), ("generation", 2)]:
            with self.subTest(field=field):
                changed = observation()
                changed["metadata"][field] = value
                client = SimpleNamespace(get_image=AsyncMock(return_value=JsonDocument(changed)))
                with self.assertRaisesRegex(image_build.BuildError, "changed"):
                    await image_build.wait_for_image(client, observation("Pending"), 1)

    async def test_build_disabled_does_not_wait_or_delete(self):
        disabled = observation("Pending")
        disabled["status"]["conditions"] = [{"type": "Ready", "status": "False", "reason": "BuildDisabled", "observedGeneration": 1}]
        client = SimpleNamespace(get_image=AsyncMock(return_value=JsonDocument(disabled)))
        with self.assertRaisesRegex(image_build.BuildError, "disabled"):
            await image_build.wait_for_image(client, observation("Pending"), 1)
        client.get_image.assert_awaited_once()

    async def test_timeout_retains_image(self):
        client = SimpleNamespace(get_image=AsyncMock(return_value=JsonDocument(observation("Building", 0))))
        with self.assertRaisesRegex(image_build.BuildError, "retained"):
            await image_build.wait_for_image(client, observation("Pending"), 0.01, poll_seconds=0.001)

    async def test_ready_requires_both_same_namespace_artifacts(self):
        for changed in [None, {"namespace": "other", "name": "snapshot"}]:
            document = observation()
            document["status"]["artifacts"]["volumeSnapshot"] = changed
            client = SimpleNamespace(get_image=AsyncMock(return_value=JsonDocument(document)))
            with self.assertRaisesRegex(image_build.BuildError, "artifacts"):
                await image_build.wait_for_image(client, observation("Pending"), 1)

    async def test_invalid_manifest_or_destination_does_not_upload(self):
        client = SimpleNamespace(upload_image_file=AsyncMock(), create_image=AsyncMock())
        invalid = copy.deepcopy(manifest())
        invalid["metadata"]["namespace"] = "../other"
        with self.assertRaises(image_build.BuildError):
            await image_build.run_image_build(client, invalid, [], wait_seconds=0)
        with self.assertRaises(image_build.BuildError):
            await image_build.run_image_build(client, manifest(), [(Path("missing"), "../outside")], wait_seconds=0)
        client.upload_image_file.assert_not_awaited()
        client.create_image.assert_not_awaited()


if __name__ == "__main__":
    unittest.main()
