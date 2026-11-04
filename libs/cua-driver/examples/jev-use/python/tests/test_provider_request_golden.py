from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from jev_adapter import choose_for_task  # noqa: E402
from provider_request_golden import (  # noqa: E402
    GOLDEN,
    encode,
    legacy_build,
    legacy_choose,
    payloads,
)
from tasks import FixtureFormTask, fixture_sources  # noqa: E402


def task_build(snapshot, token, visual, capture_bound_click, visual_delivery):
    return FixtureFormTask(token).candidates(
        fixture_sources(
            snapshot,
            visual,
            capture_bound_click=capture_bound_click,
            visual_delivery=visual_delivery,
        )
    )


def task_choose(client, candidates, snapshot, visual, history, token, visual_path):
    return choose_for_task(
        client,
        FixtureFormTask(token),
        fixture_sources(snapshot, visual, visual_path=visual_path),
        candidates,
        history,
    )


class ProviderRequestGoldenTest(unittest.TestCase):
    """Provider requests and candidate tables match the capture from main.

    The golden was recorded before the candidate-source and task-spec refactor
    (RFC #4268 Phase 0), so equality proves the refactor sends byte-identical
    model input and offers identical executable candidates.
    """

    maxDiff = None

    def assert_golden(self, build, choose) -> None:
        self.assertEqual(encode(payloads(build, choose)), GOLDEN.read_text(encoding="utf-8"))

    def test_core_entry_points_send_byte_identical_requests(self) -> None:
        self.assert_golden(legacy_build, legacy_choose)

    def test_task_spec_and_sources_send_byte_identical_requests(self) -> None:
        self.assert_golden(task_build, task_choose)


if __name__ == "__main__":
    unittest.main()
