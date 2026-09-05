import unittest
import xml.etree.ElementTree as ET

from realapp_proof import rect_position, validate_plan


class ProofOracles(unittest.TestCase):
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
