# Cua Perception artifact tooling

`assemble_bundle.py` runs on the target platform and creates a sealed bundle
plus a deterministic `.tar.gz`. It accepts a target worker, an exact source
commit, a real-parse PNG, and a directory containing reviewed inputs. The
reviewed OmniParser ONNX conversion must be supplied locally; the two OCR
models, OCR dictionary, and ONNX Runtime archive may be obtained from the
immutable URLs in `artifacts.lock.json`. Every downloaded and supplied byte is
checked before use.

Example on macOS arm64:

```sh
python3 scripts/assemble_bundle.py \
  --target aarch64-apple-darwin \
  --worker target/aarch64-apple-darwin/release/cua-perception \
  --inputs /reviewed/cua-perception-inputs \
  --cache /reviewed/cua-perception-cache \
  --real-parse-fixture /reviewed/known-answer.png \
  --source-sha "$(git rev-parse HEAD)" \
  --version 0.1.0 \
  --output /staging/cua-perception-0.1.0-aarch64-apple-darwin
```

Assembly must run on the target host. The worker and extracted runtime are
checked for the requested executable format before model sessions are created.
The assembler invokes `health`, `self_test`, and a real parse, proves that a
tampered runtime and another platform's binary header are rejected, writes the
release verification reports, then seals every file in `SHA256SUMS`.

After installation, run the same checks without modifying the bundle:

```sh
python3 scripts/verify_bundle.py /installed/cua-perception-0.1.0-aarch64-apple-darwin
```

For review of a foreign-platform bundle, `--static-only` verifies hashes,
model/runtime binding, and executable headers without launching the worker.

## Quality runner

`quality_runner.py` produces the normalized, artifact-bound engine result files
consumed by `measure_quality.py`. It runs the exact closed corpus, performs a
cold startup plus first parse and a warm parse in one process for every image,
and records measured latency plus `sampled_process_peak_rss_bytes`. RSS is
polled every 5 ms for the single worker process, excludes descendant processes,
and can miss a peak between samples. The runner never downloads models or
substitutes fixture detections in its production path.

```sh
python3 scripts/quality_runner.py \
  --engine rust \
  --config /staging/rust-quality-config.json \
  --manifest tests/quality-corpus/manifest.json \
  --output /staging/rust-results.json

python3 scripts/quality_runner.py \
  --engine python \
  --config /staging/python-quality-config.json \
  --manifest tests/quality-corpus/manifest.json \
  --output /staging/python-results.json
```

Both configs contain closed `identity`, `bindings`, and `execution` objects.
Every binding path is relative to the config, confined without symlink
traversal, and paired with its expected SHA-256. `package_artifact_id` and
`installed_artifact_ids` select the bound files summed as
`bound_installed_artifact_bytes`. This is the byte total of those selected
artifacts, not a complete installed footprint. `artifact_size_bytes` is the
size of the selected package archive and is not described as installed size.

Rust execution requires `worker_artifact_id`, `manifest_artifact_id`,
`onnx_runtime_artifact_id`, `source_model_artifact_id`,
`detector_artifact_id`, `ocr_detector_artifact_id`,
`ocr_recognizer_artifact_id`, `ocr_dictionary_artifact_id`, `extension_id`,
`extension_version`, and `timeout_seconds`. The model manifest must be a bound
`model_manifest`; its source revision must match `identity.model.revision`, and
its detector, OCR, dictionary, and runtime paths and hashes must match the
named bound artifacts. A bound `conversion_metadata` artifact requires
`conversion_metadata_artifact_id`; the runner then verifies that its input and
expected-output hashes bind the source `.pt` and converted detector. Without
that record, the source model remains declared provenance and the report does
not claim that it proves derivation of the converted detector.

Python execution requires `python_interpreter_artifact_id`, `python_home`,
`site_packages`, `model_artifact_id`, `ocr_model_artifact_ids`, `cache_dir`,
`force_device`, `box_threshold`, `iou_threshold`, `use_ocr`, and
`timeout_seconds`. The bound Python interpreter has role `worker`; the detector
has role `source_model`; and each bound `ocr_model` file must be directly under
`cache_dir/model` so EasyOCR uses the measured offline cache. For every image,
the runner copies the hash-verified wheel, detector, OCR models, and
manifest-declared corpus image into a fresh runner-owned temporary directory.
The child verifies the staged image hash before inference and uses those same
bytes for both parses. It safely extracts the staged wheel and imports `som`
only from that extraction while retaining the configured offline
`site_packages` dependencies. The child starts with inherited `PYTHON*`
settings removed and Python site initialization disabled, so `sitecustomize`,
`usercustomize`, and `.pth` startup hooks cannot run before the bound wheel is
selected. Arbitrary `source_root` configuration is rejected.

Shortly before each engine process starts, the runner resolves and hashes the
bound executable, manifest, runtime, package, and model files again. Python
execution additionally revalidates `python_home`, `site_packages`, and
`cache_dir` immediately before launch, and the child repeats the directory
checks it can perform after startup. These checks fail closed for changes they
observe; path revalidation alone is not a defense against a same-user process
that races a remaining path-based Rust or Python dependency after its final
check. The staged Python files and corpus input avoid that pathname race for
the bytes copied into the runner-owned directory.
The Python `dependency_lock` binds the declared dependency resolution, but the
current result format has no installed-file manifest for `python_home` or
`site_packages`; the runner therefore does not claim cryptographic proof of
every installed dependency byte. Those directories must be prepared from the
bound lock in the controlled quality environment.

After generating both files, build the canonical comparison report with:

```sh
python3 scripts/measure_quality.py \
  --manifest tests/quality-corpus/manifest.json \
  --rust-results /staging/rust-results.json \
  --python-results /staging/python-results.json \
  --output /staging/quality-report.json
```

Control attribute metrics keep the total geometrically matched control count in
`matched`, report supplied-field coverage separately, and use `evaluated` as
the accuracy denominator. If no matched prediction supplies an attribute, its
accuracy has `status: "unavailable"` and `ratio: null`; omission is not counted
as a wrong classification. Label coverage is limited to matched controls whose
expected annotation has a label.
