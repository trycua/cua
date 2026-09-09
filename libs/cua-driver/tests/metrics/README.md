# Snapshot slice measurements

These tools measure production Rust source, internal cache operations, and a bounded macOS native task workload. They do not replace canonical desktop regression testing or comprehensive cross-platform latency certification.

## Production code

Use Python 3.12 with `tree-sitter==0.25.2` and `tree-sitter-rust==0.24.2` in an isolated environment, with rustfmt 1.97.1 available. The macOS system Python 3.9 cannot install these pinned parser versions; do not substitute an older grammar. Run:

```bash
python libs/cua-driver/tests/metrics/test_snapshot_complexity.py
python libs/cua-driver/tests/metrics/snapshot_complexity.py "$BASELINE_ROOT" > before.json
python libs/cua-driver/tests/metrics/snapshot_complexity.py . > after.json
```

The script normalizes formatting, excludes test-only items/modules and comments, and reports nonblank production lines and a disclosed source-level complexity metric. Each function contributes one plus conditionals (including let-else), loops, try expressions, short-circuit operators, match alternatives and guards. Closure decisions belong to the enclosing function; nested functions are separate. Macros are not expanded. Reject results with any `parse_error` before comparing them. Use identical file scope and analyzer versions on both sources.

Lizard's Rust parser missed substantial function bodies in this slice and is not used for acceptance evidence.

## Cache latency

From the repository root, use a fresh evidence directory:

```bash
EVIDENCE="$PWD/artifacts/cua-driver/rfc-3473/cache-comparison"
mkdir -p "$EVIDENCE/baseline"
git archive 84340731fc38d6571881db27e3cf9f884d8f0e88 libs/cua-driver | tar -x -C "$EVIDENCE/baseline"
cp libs/cua-driver/tests/metrics/snapshot_latency_baseline.rs "$EVIDENCE/baseline/libs/cua-driver/rust/crates/cua-driver-core/tests/snapshot_latency.rs"
CARGO_TARGET_DIR="$EVIDENCE/baseline-target" cargo +1.97.1 test --release --locked --manifest-path "$EVIDENCE/baseline/libs/cua-driver/rust/Cargo.toml" -p cua-driver-core --test snapshot_latency --no-run --message-format=json > "$EVIDENCE/baseline-build.jsonl"
CARGO_TARGET_DIR="$EVIDENCE/candidate-target" cargo +1.97.1 test --release --locked --manifest-path libs/cua-driver/rust/Cargo.toml -p cua-driver-core --test snapshot_latency --no-run --message-format=json > "$EVIDENCE/candidate-build.jsonl"
python libs/cua-driver/tests/metrics/run_snapshot_latency.py "$EVIDENCE"
```

Keep baseline and candidate Cargo targets separate to avoid mixing cached crate APIs between source trees. Both binaries use the same toolchain, release profile, workloads and sampling loop. The baseline adapter performs the old identity validation followed by native-cache lookup; the candidate acquires from the unified cache. Cleanup occurs outside timing.

The runner alternates execution order across three repetitions. Each workload runs 45 samples of one million iterations, discarding the first five. Output includes raw samples, per-run and pooled medians, and binary SHA-256 hashes.

Payloads contain 64 integer members, not AX/COM objects. The rotating workload uses 16 windows: the baseline's separate native map can retain all 16 while the corrected cache enforces the eight-entry cap. No screenshot, GUI action, IPC, native retain/release or end-to-end latency is measured. Archive source hashes and machine details alongside results; do not treat a dirty-tree benchmark as an exact-commit desktop certificate.

## Bounded native task latency

`rfc-3473-native-latency.json` preserves all 3,200 sample durations, including 640 warmups, from the frozen comparison of main `6c0348b059595e63d1df96e6df2047ca7dbbbf1c` and product candidate `2a8b170609abc0cd9eb5bda0feae5f95bf0bb75c`. Its compact blocks reconstruct the original JSONL byte-for-byte; the unit test checks that digest and the measured client's SHA-256. Source hashes, environment, signed-binary hashes, protocol, and cleanup receipt are included.

Reproduce the statistics without opening a GUI:

```bash
python3 -m unittest discover -s libs/cua-driver/tests/metrics -p test_native_snapshot_latency.py
python3 libs/cua-driver/tests/metrics/analyze_native_snapshot_latency.py libs/cua-driver/tests/metrics/rfc-3473-native-latency.json
```

The estimator is the geometric mean of 16 paired block-median ratios. The unchanged percentile bootstrap uses 20,000 paired resamples, seed 3473, and order statistics 500 and 19,500 for the 95% interval. Each version/workload/block has 20 measured samples after five excluded warmups. Execution order alternates AB/BA. No failing block is dropped or sampled again for acceptance.

| Workload | Baseline block median ms | Candidate block median ms | Paired ratio | 95% ratio interval |
| --- | ---: | ---: | ---: | --- |
| Snapshot + screenshot | 93.917 | 94.562 | 1.00393 | 0.97532–1.02746 |
| Semantic press + observed counter | 1726.432 | 1720.744 | 0.99555 | 0.98906–1.00183 |
| Background pixel address + observed counter | 326.230 | 325.680 | 1.00230 | 0.98663–1.01686 |
| Set value + observed text | 2535.573 | 2531.302 | 0.99957 | 0.99818–1.00082 |

The displayed milliseconds are medians across block medians; the paired estimator is not their quotient. All four upper bounds are below the predeclared 1.05 gate. This establishes essentially flat latency within the measured scope, not a meaningful native speedup.

The client is `native_snapshot_latency.py`, byte-identical to the measured client. It uses persistent daemon-backed MCP, a fresh AppKit fixture per version/block, screenshots, and native state confirmation. The target stays visible; video recording is off. The pixel workload addresses coordinates through the AppKit background AX hit-test bridge, **not raw physical delivery**. Browser tasks, other platforms, native-read counters, and native memory high-water measurements are not covered.

For a new experiment, first use the trusted maintainer workflow to prepare an owned disposable macOS desktop and the exact signed source-identified daemon. Do not use a personal desktop, alter permissions, or silently change the measured sources/settings. The client expects the configured `~/Library/Caches/cua-driver-local/cua-driver-local.sock`; it does not install, sign, authorize, or swap the daemon. Example for one planned block:

```bash
python3 libs/cua-driver/tests/metrics/native_snapshot_latency.py \
  --binary "$SIGNED_LOCAL_CLI" --fixture "$APPKIT_EXECUTABLE" \
  --sha "$BASELINE_SHA" --label baseline --block 0 --samples 20 \
  --output "$FRESH_EVIDENCE/samples.jsonl"
```

Collect all planned pairs with the same fixture and settings, verify each daemon source, retain failures, and restore the original daemon and stop the owned worker afterward. The recorded experiment used the already-authorized local bundle identity and unrestricted measurement daemon, then restored standard mode. Its worker-derived baseline is not a certified immutable private seed. Earlier setup pilots and the failed raw foreground-click pilot are excluded and retained in the workstream evidence, not interpreted as passing performance samples.
