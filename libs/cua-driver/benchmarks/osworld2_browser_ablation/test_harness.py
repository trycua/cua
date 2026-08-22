from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import harness


class ModePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.native = {
            "screenshot": "png",
            "screenshot_metadata": {"width": 100},
            "accessibility": ["button"],
            "secret": "do-not-leak",
        }
        self.browser = {
            "outline": "heading",
            "refs": [{"ref": "p1:1"}],
            "snapshot": {"format": "semantic_v2"},
            "target_id": "hidden",
        }

    def test_screenshot_only_redacts_ax_and_cdp(self) -> None:
        projected = harness.project_observation(
            "screenshot_only", native=self.native, browser=self.browser
        )
        self.assertEqual(projected["native_screenshot"], "png")
        self.assertNotIn("native_accessibility", projected)
        self.assertNotIn("browser_outline", projected)
        self.assertNotIn("secret", projected)

    def test_cdp_only_redacts_native_state(self) -> None:
        projected = harness.project_observation(
            "cdp_only", native=self.native, browser=self.browser
        )
        self.assertEqual(projected["browser_outline"], "heading")
        self.assertNotIn("native_screenshot", projected)
        self.assertNotIn("native_accessibility", projected)
        self.assertNotIn("target_id", projected)

    def test_combined_contains_all_intended_modalities(self) -> None:
        projected = harness.project_observation(
            "combined", native=self.native, browser=self.browser
        )
        self.assertEqual(projected["native_screenshot"], "png")
        self.assertEqual(projected["native_accessibility"], ["button"])
        self.assertEqual(projected["browser_outline"], "heading")

    def test_tools_are_mode_scoped(self) -> None:
        self.assertNotIn("browser_click", harness.allowed_tools("screenshot_ax"))
        self.assertNotIn("click", harness.allowed_tools("cdp_only"))
        self.assertIn("browser_click", harness.allowed_tools("combined"))
        self.assertIn("set_value", harness.allowed_tools("combined"))


class CredentialTests(unittest.TestCase):
    def test_missing_credentials_are_reported_without_values(self) -> None:
        status = harness.credential_status({})
        self.assertIn("HF_TOKEN", status["missing"])
        self.assertFalse(status["ready_for_model_run"])

    def test_openai_credentials_can_be_complete(self) -> None:
        status = harness.credential_status(
            {
                "HF_TOKEN": "hf-secret",
                "CUA_BENCH_MODEL_PROVIDER": "openai",
                "CUA_BENCH_MODEL": "model-name",
                "OPENAI_API_KEY": "model-secret",
                "CUA_CLIENT_ID": "client",
                "CUA_CLIENT_SECRET": "fleet-secret",
            },
            local_config={"container_disk_image": "registry/image@sha256:digest"},
        )
        self.assertEqual(status["missing"], [])
        serialized = json.dumps(status)
        self.assertNotIn("hf-secret", serialized)
        self.assertNotIn("model-secret", serialized)
        self.assertNotIn("fleet-secret", serialized)

    def test_litellm_can_use_an_aws_secret_without_exposing_its_name(self) -> None:
        status = harness.credential_status(
            {"HF_TOKEN": "hf-secret"},
            local_config={
                "container_disk_image": "registry/image@sha256:digest",
                "fleet_secret_name": "fleet/credentials",
                "model_provider": "litellm",
                "model": "pinned-model-route",
                "model_base_url": "https://gateway.example/v1",
                "model_api_key_secret_name": "gateway/consumer-key",
                "model_route_verified": True,
            },
        )
        self.assertEqual(status["missing"], [])
        self.assertTrue(status["model"]["configured"])
        self.assertEqual(status["model"]["credential_source"], "aws_secret")
        serialized = json.dumps(status)
        self.assertNotIn("hf-secret", serialized)
        self.assertNotIn("gateway/consumer-key", serialized)


class BindingTests(unittest.TestCase):
    def test_requires_exactly_one_proven_active_tab(self) -> None:
        selected = harness.select_active_tab(
            [{"tab_id": "a", "active": False}, {"tab_id": "b", "active": True}]
        )
        self.assertEqual(selected["tab_id"], "b")
        with self.assertRaises(harness.HarnessError):
            harness.select_active_tab([{"tab_id": "a", "active": None}])
        with self.assertRaises(harness.HarnessError):
            harness.select_active_tab(
                [{"tab_id": "a", "active": True}, {"tab_id": "b", "active": True}]
            )


class MatrixTests(unittest.TestCase):
    def test_manifest_pins_the_driver_release_archive(self) -> None:
        manifest = harness.read_json_object(harness.DEFAULT_MANIFEST)
        driver = manifest["cua_driver"]
        self.assertEqual(driver["release"], "cua-driver-rs-v0.12.6")
        self.assertEqual(len(driver["linux_x86_64_archive_sha256"]), 64)
        self.assertEqual(
            driver["python_sdk_linux_x86_64_archive"],
            "cua-driver-rs-0.12.6-linux-x86_64.tar.gz",
        )
        self.assertEqual(
            len(driver["python_sdk_linux_x86_64_archive_sha256"]),
            64,
        )
        self.assertEqual(
            set(driver["python_sdk_source_sha256"]),
            {"__init__.py", "_native.py", "_native_contract.py", "wrapper.py"},
        )
        self.assertTrue(
            all(
                len(digest) == 64
                for digest in driver["python_sdk_source_sha256"].values()
            )
        )

    def test_default_pilot_matrix_has_240_episodes(self) -> None:
        manifest = harness.read_json_object(harness.DEFAULT_MANIFEST)
        tasks = [f"task_{index:03d}" for index in range(1, 21)]
        matrix = harness.build_matrix(
            manifest,
            tasks,
            model_provider="provider",
            model="model",
        )
        self.assertEqual(len(matrix), 240)
        self.assertEqual(len({row["episode_id"] for row in matrix}), 240)

    def test_duplicate_tasks_refuse(self) -> None:
        manifest = harness.read_json_object(harness.DEFAULT_MANIFEST)
        with self.assertRaises(harness.HarnessError):
            harness.build_matrix(
                manifest,
                ["task_001", "task_001"],
                model_provider="provider",
                model="model",
            )

    def test_release_manifest_validation(self) -> None:
        manifest = harness.read_json_object(harness.DEFAULT_MANIFEST)
        official = {
            "release": manifest["benchmark_release"],
            "osworld_code": {"tag": manifest["osworld_code"]["tag"]},
            "tasks": {
                "repository": manifest["tasks"]["repository"],
                "tag": manifest["tasks"]["revision"],
            },
            "assets": {
                "repository": manifest["assets"]["repository"],
                "tag": manifest["assets"]["revision"],
            },
            "task_hash_manifest": {
                "repository": manifest["tasks"]["repository"],
                "tag": manifest["tasks"]["revision"],
                "path": manifest["task_hash_manifest"]["path"],
                "sha256": f"sha256:{manifest['task_hash_manifest']['sha256']}",
                "task_count": manifest["tasks"]["expected_count"],
            },
            "provider_images": {
                "docker": {
                    "ubuntu": {
                        "artifact_path": manifest["image"]["archive"],
                        "artifact_size": manifest["image"]["archive_size"],
                        "artifact_sha256": f"sha256:{manifest['image']['archive_sha256']}",
                    }
                }
            },
        }
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory)
            release_dir = checkout / "benchmark_releases"
            release_dir.mkdir()
            path = release_dir / f"{manifest['benchmark_release']}.json"
            path.write_text(json.dumps(official), encoding="utf-8")
            result = harness.validate_release_manifest(manifest, checkout)
        self.assertEqual(result["task_count"], 108)


if __name__ == "__main__":
    unittest.main()
