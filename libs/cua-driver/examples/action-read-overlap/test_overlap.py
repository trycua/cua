import asyncio
import unittest

from overlap import action_with_read_warming


class ReadWarmingTest(unittest.IsolatedAsyncioTestCase):
    async def test_early_match_is_discarded_after_deferred_rejection(self):
        value = "original"
        done = asyncio.Event()
        dispatched = 0
        early_matches = []
        final_reads = []

        async def dispatch():
            nonlocal value, dispatched
            dispatched += 1
            value = "requested"
            await asyncio.sleep(0.02)
            value = "rejected"
            await asyncio.sleep(0.02)
            done.set()
            return {"effect": "unverifiable", "late_window_report": "preserved"}

        async def read():
            await asyncio.sleep(0.001)
            final_reads.append(done.is_set())
            return value

        def matches(observation):
            early_matches.append(observation == "requested")
            return observation == "requested"

        result, final = await action_with_read_warming(dispatch, read, matches)
        self.assertEqual(dispatched, 1)
        self.assertEqual(early_matches, [True])
        self.assertEqual(final_reads, [False, True])
        self.assertEqual(final, "rejected")
        self.assertEqual(result["late_window_report"], "preserved")

    async def test_read_failure_joins_action_without_retry_or_cancellation(self):
        done = asyncio.Event()
        dispatched = 0

        async def dispatch():
            nonlocal dispatched
            dispatched += 1
            await asyncio.sleep(0.02)
            done.set()
            return {"effect": "unverifiable"}

        async def read():
            raise RuntimeError("observation unavailable")

        with self.assertRaisesRegex(RuntimeError, "^observation unavailable$"):
            await action_with_read_warming(dispatch, read, lambda _: False)
        self.assertTrue(done.is_set())
        self.assertEqual(dispatched, 1)

    async def test_cancellation_after_early_match_joins_action_before_returning(self):
        advisory_done = asyncio.Event()
        release_action = asyncio.Event()
        action_done = asyncio.Event()
        dispatched = 0

        async def dispatch():
            nonlocal dispatched
            dispatched += 1
            await release_action.wait()
            action_done.set()
            return "action result"

        async def read():
            advisory_done.set()
            return "early match"

        task = asyncio.create_task(action_with_read_warming(dispatch, read, lambda _: True))
        await advisory_done.wait()
        await asyncio.sleep(0)
        task.cancel()
        asyncio.get_running_loop().call_later(0.02, release_action.set)
        with self.assertRaises(asyncio.CancelledError):
            await task
        self.assertTrue(action_done.is_set())
        self.assertEqual(dispatched, 1)

    async def test_unsatisfied_advisory_reads_are_bounded_and_final_read_remains(self):
        reads = 0
        done = asyncio.Event()

        async def dispatch():
            await asyncio.sleep(0.03)
            done.set()
            return "action result"

        async def read():
            nonlocal reads
            reads += 1
            return done.is_set()

        result, final = await action_with_read_warming(dispatch, read, lambda _: False)
        self.assertEqual(reads, 3)
        self.assertEqual((result, final), ("action result", True))


if __name__ == "__main__":
    unittest.main()
