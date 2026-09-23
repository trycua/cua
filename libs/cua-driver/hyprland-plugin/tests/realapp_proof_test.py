import unittest
import xml.etree.ElementTree as ET

from realapp_proof import cleanup_all, rect_position, released_synthetic_input, validate_plan
from primary_trace_test import START, STOP, trace


class ProofOracles(unittest.TestCase):
    def test_cleanup_failure_does_not_skip_primary_release(self):
        completed = []
        def broken():
            raise OSError('closed test transport')
        errors = cleanup_all([('video', broken), ('primary', lambda: completed.append(True))])
        self.assertEqual(completed, [True])
        self.assertEqual(errors[0]['operation'], 'video')

    def test_synthetic_release_must_balance_within_each_lane(self):
        press = ('pointer_button', 100, 200, 1, 1)
        release = ('pointer_button', 100, 200, 1, 0)
        self.assertTrue(released_synthetic_input(trace(START, press, release, STOP)))
        for events in ((press,), (release,), (press, ('pointer_button', 100, 200, 2, 0))):
            with self.assertRaises(AssertionError):
                released_synthetic_input(trace(START, *events, STOP))

    def test_control_requires_two_distinct_agents_and_known_command(self):
        plan = {'agents': [{}, {}], 'phases': [{'parallel': [{'agent': 0}, {'agent': 1}],
                'control': {'command': 'CANCEL', 'agent': 0}}]}
        validate_plan(plan)
        plan['phases'][0]['control']['command'] = 'APPROVE'
        with self.assertRaises(AssertionError):
            validate_plan(plan)

    def test_saved_rectangle_position_is_independent_of_ui(self):
        self.assertEqual(rect_position(ET.fromstring('<rect x="120" y="170"/>')), [120, 170])
        self.assertEqual(rect_position(ET.fromstring('<rect x="120" y="170" transform="translate(100,50)"/>')), [220, 220])
        for transform in ('rotate(90)', 'matrix(1,0,0,1,0,0)', 'translate(nan,0)'):
            with self.assertRaises(AssertionError):
                rect_position(ET.Element('rect', {'transform': transform}))

    def test_parallel_steps_cannot_share_a_transport(self):
        plan = {'agents': [{}, {}], 'phases': [{'parallel': [{'agent': 0}, {'agent': 1}]}]}
        validate_plan(plan)
        plan['phases'][0]['parallel'][1]['agent'] = 0
        with self.assertRaises(AssertionError):
            validate_plan(plan)

    def test_missing_delta_or_out_of_range_agent_cannot_pass(self):
        with self.assertRaises(AssertionError):
            validate_plan({'agents': [{}], 'phases': [{'agent': 1}]})
        with self.assertRaises(AssertionError):
            validate_plan({'agents': [], 'phases': [], 'outputs': [{'rect_translation': []}]})


if __name__ == '__main__':
    unittest.main()
