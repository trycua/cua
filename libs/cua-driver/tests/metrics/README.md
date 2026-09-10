# Snapshot measurement tools

These tools measure production source, cache operations, and bounded native tasks.
They supplement, but do not replace, the [desktop harnesses](../../docs/test-harnesses-guide.md).

## Source metrics

Use an isolated Python 3.12 environment with `tree-sitter==0.25.2`,
`tree-sitter-rust==0.24.2`, and rustfmt 1.97.1 available:

```bash
python libs/cua-driver/tests/metrics/test_snapshot_complexity.py
python libs/cua-driver/tests/metrics/snapshot_complexity.py "$BASELINE_ROOT" > before.json
python libs/cua-driver/tests/metrics/snapshot_complexity.py . > after.json
```

Compare identical file scopes and tool versions. The analyzer excludes tests and
comments, normalizes formatting, and does not expand macros. Reject results with
any `parse_error`; the complexity measure is source-level, not runtime cost.

## Cache microbenchmark

Build `cua-driver-core --test snapshot_latency` in release mode for both sources,
using separate Cargo targets. For the historical baseline, copy
`snapshot_latency_baseline.rs` into its core integration-test directory as
`snapshot_latency.rs`. Save Cargo's `--message-format=json` output as
`baseline-build.jsonl` and `candidate-build.jsonl` in a fresh evidence directory,
then run:

```bash
python3 libs/cua-driver/tests/metrics/run_snapshot_latency.py "$EVIDENCE"
```

The runner alternates versions and retains raw samples and binary hashes. Integer
payloads do not measure native retention, screenshots, IPC, or task latency.
Do not present cache speedups as end-to-end speedups.

## Native latency receipt

Reconstruct and verify the checked-in receipt without operating a desktop:

```bash
python3 -m unittest discover -s libs/cua-driver/tests/metrics -p test_native_snapshot_latency.py
python3 libs/cua-driver/tests/metrics/analyze_native_snapshot_latency.py libs/cua-driver/tests/metrics/rfc-3473-native-latency.json
```

The receipt includes source identities, raw-duration reconstruction, environment,
client and binary hashes, and the fixed paired bootstrap protocol. Displayed
medians are not the estimator: comparisons use geometric means of paired
block-median ratios.

For new measurements, prepare an owned disposable, authorized macOS desktop and
exact signed daemon using the maintainer workflow. The client does not install,
sign, authorize, or swap the daemon. One planned block is invoked as follows:

```bash
python3 libs/cua-driver/tests/metrics/native_snapshot_latency.py \
  --binary "$SIGNED_LOCAL_CLI" --fixture "$APPKIT_EXECUTABLE" \
  --sha "$BASELINE_SHA" --label baseline --block 0 --samples 20 \
  --output "$FRESH_EVIDENCE/samples.jsonl"
```

The client uses the configured local-driver socket and persistent MCP. Tasks
include screenshots and counter/text verification; background pixel addressing
uses AX hit-testing, not raw physical delivery. Freeze the paired plan before
execution, preserve failed attempts, verify both source identities, and restore
the standard daemon and stop the owned worker afterward. Do not change permissions
or substitute workloads to obtain a passing result. Browser, other-platform,
memory high-water, and native-read-count certification are separate concerns.
