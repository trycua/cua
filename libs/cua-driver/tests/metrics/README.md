# Snapshot slice measurements

These tools measure production Rust source and internal cache operations. They do not replace native desktop regression or end-to-end latency certification.

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
git archive ed289df50257bd6a65f9ee7964bb842777a1a10a libs/cua-driver | tar -x -C "$EVIDENCE/baseline"
cp libs/cua-driver/tests/metrics/snapshot_latency_baseline.rs "$EVIDENCE/baseline/libs/cua-driver/rust/crates/cua-driver-core/tests/snapshot_latency.rs"
CARGO_TARGET_DIR="$EVIDENCE/baseline-target" cargo +1.97.1 test --release --locked --manifest-path "$EVIDENCE/baseline/libs/cua-driver/rust/Cargo.toml" -p cua-driver-core --test snapshot_latency --no-run --message-format=json > "$EVIDENCE/baseline-build.jsonl"
CARGO_TARGET_DIR="$EVIDENCE/candidate-target" cargo +1.97.1 test --release --locked --manifest-path libs/cua-driver/rust/Cargo.toml -p cua-driver-core --test snapshot_latency --no-run --message-format=json > "$EVIDENCE/candidate-build.jsonl"
python libs/cua-driver/tests/metrics/run_snapshot_latency.py "$EVIDENCE"
```

Keep baseline and candidate Cargo targets separate to avoid mixing cached crate APIs between source trees. Both binaries use the same toolchain, release profile, workloads and sampling loop. The baseline adapter performs the old identity validation followed by native-cache lookup; the candidate acquires from the unified cache. Cleanup occurs outside timing.

The runner alternates execution order across three repetitions. Each workload runs 45 samples of one million iterations, discarding the first five. Output includes raw samples, per-run and pooled medians, and binary SHA-256 hashes.

Payloads contain 64 integer members, not AX/COM objects. The rotating workload uses 16 windows: the baseline's separate native map can retain all 16 while the corrected cache enforces the eight-entry cap. No screenshot, GUI action, IPC, native retain/release or end-to-end latency is measured. Archive source hashes and machine details alongside results; do not treat a dirty-tree benchmark as an exact-commit desktop certificate.
