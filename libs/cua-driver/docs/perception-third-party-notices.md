# Cua perception third-party notices

This document describes the reviewed candidate inputs. It is not a substitute
for the `NOTICE`, model ledger, source ledger, SBOM, and license texts shipped
with an exact `cua-perception` artifact. If those authenticated records are
missing or disagree with this summary, do not publish or redistribute the
artifact.

The default Cua Driver remains a separate MIT-licensed component and does not
include the extension, model weights, or ONNX Runtime library.

## OmniParser icon detector

- Origin: `microsoft/OmniParser-v2.0` model repository.
- Pinned revision: `6600256cb0f1b07651e3bc86166196307bad7e2d`.
- Source model: `icon_detect/model.pt`, SHA-256
  `dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57`.
- Reviewed converted ONNX artifact: SHA-256
  `d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2`.
- Artifact license recorded by the release ledger: AGPL-3.0-only.

The release ledger marks this artifact `license-review-required`. Publication
must remain blocked until that review passes. A distributor must preserve the
AGPL license and notices and provide the corresponding source and pinned
conversion/export material required for the converted artifact. Offering the
covered perception service over a network also requires the applicable AGPL
corresponding-source path. The artifact must not be presented as MIT merely
because Driver invokes it through an extension protocol.

The OmniParser source repository's license for repository content and the
separate model repository's artifact terms are different records. Verify the
exact model revision above.

## PP-OCRv5

The reviewed OCR inputs are official PaddlePaddle model repositories and are
recorded as Apache-2.0:

| Role | Repository revision | Reviewed ONNX SHA-256 |
| --- | --- | --- |
| Mobile detector | `PaddlePaddle/PP-OCRv5_mobile_det` at `e6f4fa85f00e168c862bc462aebca69eef9b3d3d` | `a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d` |
| English recognizer | `PaddlePaddle/PP-OCRv5_mobile_rec` at `3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b` | `b5f833dfc5d0eb71da397b4efa06ebeee9b431b690a47d6af40d77d8eabc557f` |

The recognizer dictionary comes from the pinned recognizer repository's
`inference.yml`, SHA-256
`27e91d0582f40168aa218303c76e184bc78fa7a5d105aad0cfbad8458b441067`.
Preserve the Apache-2.0 license and any notices in the assembled artifact.

## ONNX Runtime and conversion provenance

The worker loads a CPU-only ONNX Runtime dynamic library from the extension
package. Its upstream is Microsoft's
[`microsoft/onnxruntime`](https://github.com/microsoft/onnxruntime) project,
which publishes ONNX Runtime under the MIT license. Assembly must obtain the
library from a reviewed upstream release and record its exact version, target,
SHA-256, license, notices, and source location in the artifact manifest and
SBOM. The checked-in template intentionally contains no release version or
hash, so it is not evidence that a particular runtime shipped.

The reviewed OmniParser conversion used Python 3.12.13, PyTorch 2.8.0,
torchvision 0.23.0, Ultralytics 8.3.199, ONNX 1.19.0, ONNX Runtime 1.22.1, opset
17, a fixed 1280 input, graph simplification, and no model-side NMS. These are
conversion-provenance inputs; the Rust worker has no Python runtime dependency.
The ONNX Runtime version used during conversion does not establish which
runtime library was packaged for a release.
