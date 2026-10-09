# Dataset Manifest v1

A new **read-only dataset integrity feature** for CUA Bench. It inventories each task's source and static assets and detects drift before a benchmark is compared with previous runs.

```sh
cd libs/cua-bench
python3 scripts/dataset_manifest.py datasets/cua-bench-basic --output /tmp/cua-basic-manifest.json
python3 scripts/dataset_manifest.py datasets/cua-bench-basic --verify /tmp/cua-basic-manifest.json
```

The manifest format is `cua-dataset-manifest/v1`: deterministic task IDs (directory names), sorted relative file paths, raw-byte SHA-256 and sizes. Verification fails if task files are added, removed or edited, or if task directories are added or removed. Hidden and generated directories are excluded. Symlinked task files are rejected. The scanner never imports `main.py` or runs task code.

**Security limits:** SHA-256 validates file integrity against the provided manifest; the manifest itself must be authenticated by a trusted commit, source repository or signed release. This is not an Oracle execution receipt and cannot prove that a GUI action was performed. Pin the task code, data assets and environment separately for comparable results.

**Acceptance:** negative tests cover drift, new tasks, symlinks and non-execution. CI executes a manifest round-trip against `cua-bench-basic`. Do not mark the feature verified until the relevant CI run passes.
