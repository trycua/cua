from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from run import jev_config_from_args, parse_args


class RunnerConfigTest(unittest.TestCase):
    def resolve(self, arguments: list[str], environment: dict[str, str]):
        # Exercise the actual CLI parser and runner merge, not a fabricated config.
        with patch.dict(os.environ, environment, clear=True):
            before = dict(os.environ)
            with patch.object(sys, "argv", ["run.py", *arguments]):
                parsed = parse_args()
            try:
                return jev_config_from_args(parsed)
            finally:
                self.assertEqual(dict(os.environ), before)

    def test_omitted_provider_preserves_environment_backend(self) -> None:
        config = self.resolve([], {"JEV_BACKEND": "local"})
        self.assertEqual(config.backend, "local")
        self.assertEqual(config.base_url, "http://127.0.0.1:8787")

    def test_no_selection_defaults_to_mock(self) -> None:
        self.assertEqual(self.resolve([], {}).backend, "mock")

    def test_explicit_mock_overrides_environment_backend(self) -> None:
        config = self.resolve(["--provider", "mock"], {"JEV_BACKEND": "local"})
        self.assertEqual(config.backend, "mock")

    def test_live_alias_overrides_environment_backend(self) -> None:
        config = self.resolve(["--provider", "live"], {"JEV_BACKEND": "local"})
        self.assertEqual(config.backend, "typesafe")

    def test_cli_fields_override_only_their_environment_values(self) -> None:
        config = self.resolve(
            [
                "--jev-base-url", "http://127.0.0.1:9999",
                "--jev-model", "cli-model",
                "--jev-timeout-ms", "750",
            ],
            {
                "JEV_BACKEND": "local",
                "JEV_BASE_URL": "http://127.0.0.1:8787",
                "JEV_MODEL": "environment-model",
                "JEV_TIMEOUT_MS": "1200",
                "JEV_API_KEY": "fixture-only-not-a-credential",
            },
        )
        self.assertEqual(config.backend, "local")
        self.assertEqual(config.base_url, "http://127.0.0.1:9999")
        self.assertEqual(config.model, "cli-model")
        self.assertEqual(config.timeout_ms, 750)
        self.assertEqual(config.api_key, "fixture-only-not-a-credential")

    def test_unknown_environment_backend_is_not_silent_mock(self) -> None:
        with self.assertRaisesRegex(ValueError, "JEV_BACKEND"):
            self.resolve([], {"JEV_BACKEND": "typo"})

    def test_explicit_provider_overrides_invalid_environment_backend(self) -> None:
        config = self.resolve(["--provider", "mock"], {"JEV_BACKEND": "typo"})
        self.assertEqual(config.backend, "mock")


if __name__ == "__main__":
    unittest.main()
