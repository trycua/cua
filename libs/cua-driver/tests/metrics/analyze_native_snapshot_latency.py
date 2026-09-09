import argparse
import json
import math
import random
import statistics
from pathlib import Path


def analyze(receipt):
    assert receipt['schema'] == 'cua-driver/native-snapshot-latency@v1'
    plan = receipt['plan']
    assert plan['pairs'] == 16
    assert plan['samples_per_workload_per_block'] == 20
    assert plan['warmups_per_workload_per_block'] == 5
    assert plan['upper_ratio_gate'] == 1.05
    assert plan['order'] == 'AB for even pairs, BA for odd pairs'
    assert plan['workloads'] == ['snapshot', 'semantic', 'pixel_background', 'text']
    assert len(receipt['blocks']) == 128
    groups = {}
    for row in receipt['blocks']:
        key = (row['label'], row['block'], row['workload'])
        assert key not in groups
        assert row['label'] in ('baseline', 'candidate')
        assert type(row['block']) is int and 0 <= row['block'] < 16
        assert row['workload'] in plan['workloads']
        assert len(row['ns']) == 25
        assert all(type(ns) is int and ns > 0 for ns in row['ns'])
        groups[key] = statistics.median(row['ns'][5:])
    report = {}
    for workload in plan['workloads']:
        medians = {
            label: [groups[label, block, workload] for block in range(16)]
            for label in ('baseline', 'candidate')
        }
        ratios = [math.log(c / b) for b, c in zip(medians['baseline'], medians['candidate'])]
        rng = random.Random(3473)
        boot = sorted(
            math.exp(statistics.mean(rng.choices(ratios, k=16)))
            for _ in range(20000)
        )
        ci = [boot[499], boot[19499]]
        report[workload] = {
            'baseline_block_median_ms': statistics.median(medians['baseline']) / 1e6,
            'candidate_block_median_ms': statistics.median(medians['candidate']) / 1e6,
            'paired_geometric_ratio': math.exp(statistics.mean(ratios)),
            'ratio_ci95': ci,
            'no_slowdown_over_5pct': ci[1] < 1.05,
        }
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('receipt', type=Path)
    args = parser.parse_args()
    print(json.dumps(analyze(json.loads(args.receipt.read_text())), indent=2))
