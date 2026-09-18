# Cua perception Rust inference feasibility spike

Status: bounded evidence for RFC #3931, captured 2026-09-17 from commit
`aca1985ffc4061d580218f5f850ac57132d90e36` on Apple M1 Ultra / macOS 26.6.1.
This work adds no Driver implementation or public protocol.

Primary sources inspected:

- local `libs/python/som` implementation, metadata, tests, and license;
- OmniParser v2 Hugging Face revision
  `6600256cb0f1b07651e3bc86166196307bad7e2d`, including
  `icon_detect/LICENSE`;
- OmniParser source revision `354021201345a96178360b28733573e27269f2de`;
- EasyOCR source revision `363afb184047ce452e436f4224f3098422df872e`,
  including its download configuration and Apache-2.0 license;
- `ocrs` revision `e5f1c5205326804637368422fe420aa1b9769676`,
  `ocrs-models` revision `068934f1725959b734edef025b13c46ddf784326`,
  and Hugging Face model revision
  `df0edd170279ab971b53e094c627255a87e1a503`; and
- crates.io metadata/source for `ort` 2.0.0-rc.13, `tract-onnx` 0.23.7,
  `ocrs` 0.13.1, and RTen 0.26.0.

## Recommendation

**Go** on a second, artifact-selection prototype only after choosing
redistribution-cleared detector and OCR weights. That prototype should compare
pinned ONNX Runtime CPU and tract CPU with the same detector ONNX graph, plus a
locally provisioned OCR candidate. ONNX Runtime and `ocrs`/RTen are candidates
based on documented format, provider, platform, and API capabilities plus the
license inventory below. This spike did not compile or benchmark ONNX Runtime,
run OCR, export or run YOLO, or execute any real perception model.

**No-go** on packaging the current `cua-som` implementation or its current
OmniParser icon checkpoint as the production extension until licensing is
reviewed and a pinned ONNX export passes numerical and quality parity tests on
all supported targets. The current CC-BY-SA-4.0 `ocrs` model family is also a
no-go pending explicit approval or replacement with cleared OCR weights.

The accompanying `tract-onnx` experiment demonstrates only the Python-free
mechanics of decoding a PNG and executing a 169-byte synthetic identity graph.
It does not establish tract suitability for YOLO or OCR. ONNX Runtime is worth
evaluating next because its documentation describes CPU, CoreML, CUDA,
TensorRT, and DirectML execution providers. Its native per-target artifacts and
provider configuration remain untested packaging work, so the prototype should
require a CPU path and treat acceleration as optional.

## Existing `cua-som` behavior

`libs/python/som` combines:

- Ultralytics YOLO loading a PyTorch `model.pt` for icon boxes;
- EasyOCR for text detection and recognition;
- PyTorch device selection in priority order CUDA, MPS, CPU;
- one 1280-pixel detector pass on CPU, or 640/1280/1920 passes with augmented
  inference on CUDA/MPS, followed by torchvision NMS;
- normalized `[x1, y1, x2, y2]` boxes, although RFC #3931 proposes pixel-space
  regions tied to one source capture.

The default detector path calls `hf_hub_download` for
`microsoft/OmniParser-v2.0`, `icon_detect/model.pt` without a `revision`. At the
time of this spike the repository resolved to
`6600256cb0f1b07651e3bc86166196307bad7e2d`; the file advertised size is
40,623,819 bytes and linked SHA-256 is
`dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57`.
EasyOCR also downloads its selected detector and recognizer archives on first
use. The English defaults and exact version must be resolved into an install
manifest rather than delegated to EasyOCR at runtime.

The current tests mock model behavior and do not establish detector/OCR
quality, offline startup, weight identity, or cross-platform parity.

## Runtime candidates

| Runtime | License | Strengths | Costs and platform notes | Decision |
| --- | --- | --- | --- | --- |
| `ort` 2.0.0-rc.13 / ONNX Runtime | MIT OR Apache-2.0 wrapper; ONNX Runtime MIT | Documentation describes broad ONNX operator coverage; CPU plus CoreML on macOS, CUDA/TensorRT on Linux, DirectML on Windows | Not built or run in this spike; wrapper is still release-candidate; default crate features download native binaries during build; artifacts, hashes, provider availability, and dynamic-library loading must be owned per target | Evaluate in the next prototype |
| `tract-onnx` 0.23.7 | MIT OR Apache-2.0 | Self-contained Rust; the spike executed a synthetic identity graph offline on macOS arm64 | No YOLO/OCR graph was loaded; real operator support, cross-platform behavior, accuracy, and performance remain unknown; no equivalent native GPU-provider story | Retain as a CPU runtime candidate, not a validated fallback |
| RTen 0.26.0 | MIT OR Apache-2.0 | Rust CPU/Wasm runtime used by `ocrs` | Not built or run in this spike; general detector model/operator coverage is unevaluated | Evaluate through `ocrs` for OCR only |
| Candle | MIT OR Apache-2.0 | Native Rust tensors with CPU, CUDA, and Metal backends | No general drop-in ONNX path for this YOLO checkpoint; implies model-specific graph and preprocessing work | Defer |
| Burn ONNX import | MIT OR Apache-2.0 | Generates Rust model code with multiple backends | Import is a conversion/build step with operator-coverage risk and larger generated surface | Defer until a pinned ONNX graph exists |

The current Driver Rust workspace has no ONNX/tract/Candle/Burn/OCR runtime.
Adding perception therefore creates a new dependency and artifact boundary; it
should remain an optional worker as the RFC requires.

## Model export and validation path

1. Resolve the source checkpoint by repository commit and file SHA-256. Never
   use the floating Hugging Face repository head.
2. Run export in a pinned container or lockfile containing exact Python,
   PyTorch, Ultralytics, ONNX, and ONNX simplifier versions. Record the export
   command, opset, input layout, resize/letterbox rules, color order, scaling,
   output tensor names/shapes, and whether NMS is inside the graph.
3. Prefer a fixed detector input size first. Dynamic shapes and embedded NMS
   can be evaluated after the basic graph works across runtimes.
4. Validate the ONNX graph with `onnx.checker`, then run ONNX Runtime CPU and
   the PyTorch source on the same redistributable screenshots. Compare raw
   tensors before post-processing and final boxes after identical confidence
   filtering/NMS. Set tolerances before collecting the acceptance corpus.
5. Test loading with `tract-onnx` to identify nonportable operators. Keep tract
   only if output parity and latency are acceptable.
6. Publish the ONNX model only as an explicit, versioned optional artifact with
   byte size, SHA-256, source revision, conversion recipe, notices, and target
   compatibility. Parsing must make no network request.

The PyTorch checkpoint is serialized code/data rather than a stable exchange
format. It should only be opened in the pinned conversion environment and not
loaded by the Rust worker.

## OCR choices

| Choice | Fit | Decision |
| --- | --- | --- |
| `ocrs` 0.13.1 + RTen | Its documentation describes a Rust API, CPU/Wasm support, layout output, and models exported from PyTorch to ONNX/RTen | Engine candidate only. The current CC-BY-SA-4.0 models are no-go pending explicit approval or replacement; no OCR quality or runtime was tested here |
| Platform OCR (Vision, Windows OCR, Linux service) | May provide good native acceleration | Reject as the sole implementation: semantics, languages, availability, and redistribution differ by OS; it breaks the same-worker cross-platform contract |
| Tesseract | Mature, Apache-2.0 engine, broad languages | Include as an optional comparison; it adds native data/artifacts, and screenshot quality was not measured in this spike |
| EasyOCR | Closest behavioral match to `cua-som` | Reject for the Rust worker: Python/PyTorch runtime and implicit downloads defeat the extension boundary |
| Custom detector/recognizer through `ort` | Maximum control and one inference runtime | Keep as a later option if `ocrs` quality is insufficient; it requires owning preprocessing, decoding, language packs, and training/model provenance |

The `ocrs` library accepts caller-provided model objects, and its project docs
use a separate model-download script plus local model paths. Product integration
must provision exact reviewed files in the explicit extension-install
transaction and construct the library from those local paths.

## Licensing facts and unknowns

These are engineering findings, not legal advice.

- `libs/python/som/LICENSE` and `pyproject.toml` say AGPL-3.0-or-later, while
  its README says MIT. The package notice must be corrected before reuse.
- The OmniParser model card is tagged MIT at repository level but explicitly
  says `icon_detect` is AGPL and `icon_caption` is MIT. The checked
  `icon_detect/LICENSE` is AGPL-3.0. The proposed worker only needs
  `icon_detect`, so the general MIT tag does not clear redistribution.
- Ultralytics code is AGPL-3.0 unless covered by a separate enterprise license.
  Exporting with Ultralytics also leaves an unresolved question about license
  obligations for the converted graph and any bundled exporter-generated
  material.
- The exact copyright holder, source training dataset terms, and whether every
  dataset/input was licensed for commercial model redistribution are not
  enumerated in the model card. Those are release blockers for this weight.
- EasyOCR code is Apache-2.0, but the detector and English recognizer files have
  independent provenance. The code lists archive URLs and MD5 checksums, not
  complete per-weight license notices or SHA-256 hashes. Exact selected files
  and their notices remain unknown until resolved against a pinned EasyOCR
  revision.
- `ocrs`, RTen, tract, and the Rust `ort` wrapper are MIT OR Apache-2.0. The
  current `robertknight/ocrs` Hugging Face model repository is explicitly
  CC-BY-SA-4.0 at revision `df0edd170279ab971b53e094c627255a87e1a503` and
  says it uses HierText plus synthetic data. Whether ShareAlike is acceptable
  for the model bundle and what notices/source obligations apply is a release
  decision; a permissive engine license alone is insufficient.
- ONNX Runtime is MIT, but optional execution providers can introduce separate
  SDK/runtime terms (for example CUDA/TensorRT or Windows components). Ship
  only providers reviewed for each artifact.

Therefore both the current OmniParser icon weight and the cited `ocrs` model
family are a **no-go for bundling** in a permissively distributed Cua component
without explicit approval or replacement weights with clear redistribution
provenance.

## Expected artifacts and dependencies

An installable extension would need:

- one Rust worker executable per supported target;
- the selected inference runtime: either a statically integrated CPU runtime,
  or pinned ONNX Runtime native libraries and execution-provider dependencies;
- an icon detector ONNX graph plus metadata for input size/layout, normalization,
  output decoding, class semantics, and NMS;
- OCR text detector and recognizer graphs plus alphabet/language metadata;
- a manifest containing component version, protocol compatibility, every URL,
  byte size, SHA-256, license identifier/notice path, target, and optional
  acceleration requirements;
- deterministic image decode/resize and post-processing code;
- third-party notices and the exact conversion recipe;
- redistributable quality fixtures kept separate from private screenshots.

No model, tokenizer, or language data should be embedded in the default Driver
install. The worker should start and parse offline after explicit installation.

## Platform support expectations

- **macOS arm64/x86_64:** evaluate ONNX Runtime CPU and optional CoreML. This
  spike ran only tract on macOS arm64; x86_64, CoreML, and real graph behavior
  need native verification.
- **Windows x86_64/arm64:** evaluate ONNX Runtime CPU and optional DirectML.
  Neither architecture, native DLL packaging, nor provider availability was
  tested here.
- **Linux x86_64/aarch64:** evaluate ONNX Runtime CPU and optional CUDA/TensorRT.
  Neither architecture, glibc compatibility, provider packaging, nor CPU
  instruction dispatch was tested here.
- **Wasm:** RTen/ocrs can be relevant to a future browser worker, but RFC #3931
  targets a local optional worker. Wasm is not acceptance scope for the first
  release.

The next prototype should require a CPU path as the cross-platform comparison
point. Accelerator selection must be observable, never silently change output
semantics, and fall back explicitly.

## Reproducible Mac evidence

Environment:

```text
Apple M1 Ultra, arm64
macOS 26.6.1 (25G76)
rustc 1.97.1 (8bab26f4f 2026-07-14)
cargo 1.97.1 (c980f4866 2026-06-30)
```

Fixture identities:

```text
1c4c202054adc79f31433633008ec91b14938ac67d55eeca5f0031be3b5d02ba  screenshot.png
283c2228ceb4ebea062c89fce11e3008d65a4788bae026d9adf509e484d97df4  identity.onnx
```

Commands:

```sh
cargo build --offline --locked --release --manifest-path \
  libs/cua-driver/experiments/cua-perception-inference/Cargo.toml

cargo run --offline --locked --release --manifest-path \
  libs/cua-driver/experiments/cua-perception-inference/Cargo.toml

cargo test --offline --locked --release --manifest-path \
  libs/cua-driver/experiments/cua-perception-inference/Cargo.toml

cargo clippy --offline --locked --release --manifest-path \
  libs/cua-driver/experiments/cua-perception-inference/Cargo.toml -- -D warnings

shasum -a 256 \
  libs/cua-driver/experiments/cua-perception-inference/fixtures/*

uv run --isolated --no-project --with onnx==1.19.0 python \
  libs/cua-driver/experiments/cua-perception-inference/generate_fixture_model.py
```

The `--offline --locked` Cargo commands establish repeatability from the
already populated local Cargo cache and committed lockfile. This spike did not
test a clean machine or a vendored dependency build.

The experiment decodes the committed PNG, loads and optimizes a committed
169-byte ONNX identity graph through tract, runs a `[1,3,2,2]` tensor, and
checks exact output equality. It contains no Python runtime dependency and is
isolated from the repository Cargo workspace. These measurements apply only to
that tract identity fixture. They are not ORT, OCR, YOLO, detector, model-export,
or real inference benchmarks and cannot predict those workloads.

Measured tract identity-fixture release run after compilation:

```text
decoded=32x32 decode_us=431 model_load_us=2558 inference_us=32 output_shape=[1, 3, 2, 2]
maximum resident set size reported by /usr/bin/time: 6,307,840 bytes
release executable size: 27,837,648 bytes (unstripped local build)
```

## Next acceptance gate

Proceed only after selecting redistribution-cleared detector and OCR weights.
The next spike must use one pinned real detector ONNX artifact and one pinned
local OCR candidate, then report PyTorch/ORT/tract detector output parity, icon
precision/recall and OCR quality on a redistributable screenshot corpus, warm
and cold latency/RSS on macOS arm64 plus Windows and Linux CPU, offline startup,
corrupt/missing artifact errors, and artifact/license manifest validation.
