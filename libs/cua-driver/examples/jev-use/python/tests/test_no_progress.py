from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from no_progress import NoProgressGuard, observed_progress_score


class NoProgressGuardTest(unittest.TestCase):
    def test_repeated_reobserve_stops_at_the_bounded_limit(self) -> None:
        guard = NoProgressGuard("appkit-counter")
        self.assertIsNone(guard.before_step({"counter": 0}))
        for _ in range(2):
            guard.note("reobserve", "reobserve")
            self.assertIsNone(guard.before_step({"counter": 0}))
        guard.note("reobserve", "reobserve")
        self.assertEqual(
            guard.before_step({"counter": 0}).__dict__,
            {"pattern": "reobserve", "streak": 3},
        )

    def test_proven_app_progress_resets_the_streak(self) -> None:
        guard = NoProgressGuard("appkit-counter")
        guard.before_step({"counter": 0})
        guard.note("reobserve", "reobserve")
        guard.before_step({"counter": 0})
        guard.note("performed", "ax:button:increment")
        self.assertIsNone(guard.before_step({"counter": 1}))
        guard.note("reobserve", "reobserve")
        self.assertIsNone(guard.before_step({"counter": 1}))

    def test_same_delivered_candidate_without_app_progress_stops(self) -> None:
        guard = NoProgressGuard("appkit-counter")
        guard.before_step({"counter": 1})
        for _ in range(2):
            guard.note("performed", "ax:button:increment")
            self.assertIsNone(guard.before_step({"counter": 1}))
        guard.note("performed", "ax:button:increment:foreground")
        stop = guard.before_step({"counter": 1})
        self.assertEqual((stop.pattern, stop.streak), ("same_candidate", 3))

    def test_recovery_patterns_stop_without_replay(self) -> None:
        guard = NoProgressGuard("appkit-counter")
        guard.before_step({"counter": 0})
        for kind in ("stale", "refused"):
            guard.note(kind, "ax:button:increment")
            self.assertIsNone(guard.before_step({"counter": 0}))
        guard.note("reobserve", "reobserve")
        stop = guard.before_step({"counter": 0})
        self.assertEqual((stop.pattern, stop.streak), ("recovery", 3))

    def test_repeated_unobservable_save_note_action_stops(self) -> None:
        guard = NoProgressGuard("appkit-save-note")
        guard.before_step({"note_saved": None})
        for _ in range(2):
            guard.note("performed", "ax:text_input:note:set:note")
            self.assertIsNone(guard.before_step({"note_saved": None}))
        guard.note("performed", "ax:text_input:note:set:note")
        stop = guard.before_step({"note_saved": None})
        self.assertEqual((stop.pattern, stop.streak), ("same_candidate", 3))

    def test_unobservable_candidate_change_starts_fresh_window(self) -> None:
        guard = NoProgressGuard("appkit-save-note")
        guard.before_step({"note_saved": None})
        for _ in range(2):
            guard.note("performed", "ax:text_input:note:set:note")
            self.assertIsNone(guard.before_step({"note_saved": None}))
        guard.note("performed", "ax:button:save-note")
        self.assertIsNone(guard.before_step({"note_saved": None}))
        guard.note("performed", "ax:button:save-note")
        self.assertIsNone(guard.before_step({"note_saved": None}))

    def test_unobservable_recovery_then_dispatch_remains_bounded(self) -> None:
        # A successful dispatch without app-owned progress is not evidence
        # that a stale/refused recovery cycle made progress.
        for recovery in ("stale", "refused"):
            with self.subTest(recovery=recovery):
                guard = NoProgressGuard("appkit-save-note")
                guard.before_step({"note_saved": None})
                for kind in (recovery, "performed"):
                    guard.note(kind, "ax:button:save-note")
                    self.assertIsNone(guard.before_step({"note_saved": None}))
                guard.note(recovery, "ax:button:save-note")
                stop = guard.before_step({"note_saved": None})
                self.assertIsNotNone(stop)
                self.assertEqual((stop.pattern, stop.streak), ("recovery", 3))

    def test_unobservable_new_candidate_after_recovery_resets(self) -> None:
        guard = NoProgressGuard("appkit-save-note")
        guard.before_step({"note_saved": None})
        guard.note("performed", "ax:text_input:note:set:note")
        guard.before_step({"note_saved": None})
        guard.note("refused", "ax:button:save-note")
        guard.before_step({"note_saved": None})
        guard.note("performed", "ax:button:save-note:foreground")
        self.assertIsNone(guard.before_step({"note_saved": None}))
        self.assertEqual(guard.streak, 1)

    def test_observed_scores_are_value_free_and_task_local(self) -> None:
        self.assertEqual(observed_progress_score("appkit-counter", {"counter": 2}), 2)
        self.assertEqual(
            observed_progress_score("gtk3-choose-size", {"size": "large", "agreed": False}),
            1,
        )
        self.assertIsNone(observed_progress_score("wpf-save-note", {"note_saved": None}))


if __name__ == "__main__":
    unittest.main()
