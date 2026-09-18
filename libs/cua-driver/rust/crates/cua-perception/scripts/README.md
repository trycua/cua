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
