import copy
import hashlib
import json
import unittest
from pathlib import Path

from analyze_native_snapshot_latency import analyze


class NativeLatencyTests(unittest.TestCase):
    def setUp(self):
        self.receipt = json.loads(
            Path(__file__).with_name('rfc-3473-native-latency.json').read_text()
        )

    def test_receipt_preserves_complete_raw_measurements_and_client(self):
        receipt = self.receipt
        plan = receipt['plan']
        groups = {(r['label'], r['block'], r['workload']): r['ns'] for r in receipt['blocks']}
        digest = hashlib.sha256()
        for block in range(16):
            labels = ('baseline', 'candidate') if block % 2 == 0 else ('candidate', 'baseline')
            for label in labels:
                for iteration in range(25):
                    for workload in plan['workloads']:
                        row = dict(
                            label=label, sha=plan[label + '_sha'], block=block,
                            iteration=iteration, warmup=iteration < 5, workload=workload,
                            ns=groups[label, block, workload][iteration],
                            screenshot_width=receipt['screenshot_dimensions'][0],
                            screenshot_height=receipt['screenshot_dimensions'][1],
                        )
                        digest.update((json.dumps(row) + '\n').encode())
        self.assertEqual(digest.hexdigest(), receipt['raw_jsonl_sha256'])
        client = Path(__file__).with_name('native_snapshot_latency.py')
        self.assertEqual(hashlib.sha256(client.read_bytes()).hexdigest(), plan['script_sha256']['measure.py'])

    def test_known_slowdown_fails_without_counting_warmups(self):
        receipt = copy.deepcopy(self.receipt)
        for row in receipt['blocks']:
            ns = 100 if row['label'] == 'baseline' else 200
            row['ns'] = [1000000] * 5 + [ns] * 20
        for result in analyze(receipt).values():
            self.assertEqual(result['paired_geometric_ratio'], 2)
            self.assertEqual(result['ratio_ci95'], [2, 2])
            self.assertFalse(result['no_slowdown_over_5pct'])

    def test_incomplete_duplicate_and_invalid_samples_are_rejected(self):
        incomplete = copy.deepcopy(self.receipt)
        incomplete['blocks'].pop()
        duplicate = copy.deepcopy(self.receipt)
        duplicate['blocks'][1] = duplicate['blocks'][0]
        invalid = copy.deepcopy(self.receipt)
        invalid['blocks'][0]['ns'][7] = 0
        short = copy.deepcopy(self.receipt)
        short['blocks'][0]['ns'].pop()
        for receipt in (incomplete, duplicate, invalid, short):
            with self.subTest(), self.assertRaises(AssertionError):
                analyze(receipt)


if __name__ == '__main__':
    unittest.main()
