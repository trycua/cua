from __future__ import annotations

import base64
import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import fleet_pilot


class StageLocalFileTests(unittest.TestCase):
    def test_stages_in_bounded_chunks_and_attests_payload(self) -> None:
        payload = b"a" * (fleet_pilot.LOCAL_ASSET_CHUNK_BYTES + 17)
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "asset.bin"
            source.write_bytes(payload)
            calls: list[tuple[list[str], int]] = []

            def record(command: list[str], *, timeout: int = 120, **_: object):
                calls.append((command, timeout))
                if command[0] == "sha256sum":
                    return {
                        "returncode": 0,
                        "output": f"{hashlib.sha256(payload).hexdigest()}  staged\n",
                    }
                if command[0] == "stat":
                    return {"returncode": 0, "output": f"{len(payload)}\n"}
                return {"returncode": 0, "output": ""}

            with patch.object(fleet_pilot, "guest_exec", side_effect=record):
                evidence = fleet_pilot.stage_local_file_to_guest(
                    source,
                    "/home/user/Downloads/asset.bin",
                )

        chunk_calls = [
            command for command, _ in calls if len(command) > 4 and "base64.b64decode" in command[2]
        ]
        self.assertEqual(len(chunk_calls), 2)
        self.assertEqual(
            b"".join(base64.b64decode(command[3]) for command in chunk_calls),
            payload,
        )
        self.assertEqual(evidence["bytes"], len(payload))
        self.assertEqual(evidence["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertEqual(evidence["destination"], "/home/user/Downloads/asset.bin")
        self.assertLessEqual(
            max(len(command[3]) for command in chunk_calls),
            (fleet_pilot.LOCAL_ASSET_CHUNK_BYTES * 4 // 3) + 4,
        )

    def test_rejects_non_absolute_guest_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "asset.bin"
            source.write_bytes(b"x")
            with self.assertRaisesRegex(
                fleet_pilot.PilotError,
                "must be absolute",
            ):
                fleet_pilot.stage_local_file_to_guest(source, "relative.bin")


class PoolShapeTests(unittest.TestCase):
    def test_create_pool_uses_explicit_vm_shape(self) -> None:
        http = Mock()
        http.post.return_value.status_code = 201

        fleet_pilot.create_pool(
            http,
            namespace="shape-test",
            image="registry/image@sha256:digest",
            services=(fleet_pilot.Service("control", 5000, 5000),),
            cpu_cores=2,
            memory="8Gi",
        )

        body = http.post.call_args.kwargs["json"]
        self.assertEqual(body["spec"]["template"]["cpuCores"], 2)
        self.assertEqual(body["spec"]["template"]["memory"], "8Gi")


if __name__ == "__main__":
    unittest.main()
