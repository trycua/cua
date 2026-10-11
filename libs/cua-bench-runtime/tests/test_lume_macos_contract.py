from __future__ import annotations

import unittest

from cua_bench_runtime.adapters.lume_macos import _desktop_fixture
from cua_bench_runtime.errors import HarnessFailure


class LumeMacosContractTests(unittest.TestCase):
    def test_protected_desktop_contract_is_task_owned(self) -> None:
        task = {
            "variants": [
                {
                    "parameters": {
                        "protected_desktop": {
                            "app_id": "example-desk",
                            "store_relative": "task-store",
                            "launch_arguments": ["--record=ITEM-1042"],
                        }
                    }
                }
            ]
        }
        self.assertEqual(
            _desktop_fixture(task),
            ("task-store", "example-desk", ("--record=ITEM-1042",)),
        )

    def test_protected_desktop_contract_rejects_implicit_or_unsafe_values(self) -> None:
        self.assertIsNone(_desktop_fixture({"variants": [{"parameters": {}}]}))
        for fixture in (
            {"app_id": "example-desk", "store_relative": "task-store"},
            {
                "app_id": "example-desk",
                "store_relative": "../store",
                "launch_arguments": [],
            },
            {
                "app_id": "example-desk",
                "store_relative": "task-store",
                "launch_arguments": ["--store=/tmp/override"],
            },
        ):
            with (
                self.subTest(fixture=fixture),
                self.assertRaisesRegex(HarnessFailure, "protected desktop contract is invalid"),
            ):
                _desktop_fixture({"variants": [{"parameters": {"protected_desktop": fixture}}]})


if __name__ == "__main__":
    unittest.main()
