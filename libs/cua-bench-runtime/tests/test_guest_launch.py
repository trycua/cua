from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.guest_launch import load_guest_launch


class GuestLaunchTests(unittest.TestCase):
    def contract(self, root: Path, **changes: object) -> Path:
        build = root / "harness.bin"
        build.write_bytes(b"frozen harness")
        document = {
            "schema_version": 1,
            "id": "apparatus.synthetic-task-v1",
            "argv": ["/usr/bin/python3", "${agent}", "--workspace", "${workspace}"],
            "cwd": "${home}/work",
            "env": {"PATH": "/usr/bin:/bin", "CDB_ARTIFACTS": "${artifacts}"},
            "build_artifacts": [
                {
                    "path": "harness.bin",
                    "sha256": hashlib.sha256(build.read_bytes()).hexdigest(),
                }
            ],
            "telemetry": [{"path": "events.jsonl", "trust": "harness_reported"}],
            "collection": {
                "max_files": 20,
                "max_total_bytes": 2048,
                "max_file_bytes": 1024,
                "max_depth": 4,
            },
        }
        document.update(changes)
        path = root / "launch.json"
        path.write_text(json.dumps(document), encoding="utf-8")
        return path

    def values(self) -> dict[str, str]:
        return {
            "agent": "/Users/Shared/attempt/harness/agent.py",
            "workspace": "/Users/Shared/attempt/workspace",
            "artifacts": "/Users/Shared/attempt/artifacts",
            "home": "/Users/Shared/attempt/home",
            "brief": "/Users/Shared/attempt/task/brief.md",
            "driver_socket": "/private/var/run/cdb-mediator/attempt/driver.sock",
        }

    def test_loads_and_renders_closed_contract(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spec = load_guest_launch(self.contract(Path(directory)))
            rendered = spec.render(self.values())
            self.assertEqual(rendered.argv[1], self.values()["agent"])
            self.assertEqual(rendered.cwd, "/Users/Shared/attempt/home/work")
            self.assertEqual(
                rendered.environment,
                {"CDB_ARTIFACTS": self.values()["artifacts"], "PATH": "/usr/bin:/bin"},
            )
            self.assertEqual(spec.collection.max_files, 20)

    def test_renders_protected_driver_socket_without_changing_other_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spec = load_guest_launch(
                self.contract(
                    Path(directory),
                    argv=["/usr/bin/python3", "${agent}", "--socket", "${driver_socket}"],
                )
            )
            rendered = spec.render(self.values())
            self.assertEqual(rendered.argv[-1], self.values()["driver_socket"])

    def test_rejects_unknown_placeholder_and_dangerous_environment(self) -> None:
        cases = (
            {"argv": ["${unknown}"]},
            {"env": {"DYLD_INSERT_LIBRARIES": "/tmp/x"}},
            {"env": {"lowercase": "x"}},
        )
        for index, changes in enumerate(cases):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(ValidationFailure):
                    load_guest_launch(self.contract(Path(directory), **changes))

    def test_rejects_build_digest_mismatch_and_escaping_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = self.contract(root)
            document = json.loads(path.read_text(encoding="utf-8"))
            document["build_artifacts"][0]["sha256"] = "0" * 64
            path.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "digest mismatch"):
                load_guest_launch(path)

        with tempfile.TemporaryDirectory() as directory:
            spec = load_guest_launch(self.contract(Path(directory), cwd="/Users/Shared/other"))
            with self.assertRaisesRegex(ValidationFailure, "escapes"):
                spec.render(self.values())


if __name__ == "__main__":
    unittest.main()
