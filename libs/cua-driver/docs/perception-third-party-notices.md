# Cua perception third-party notices

This document describes the reviewed candidate inputs. It is not a substitute
for the `NOTICE`, model ledger, source ledger, SBOM, and license texts shipped
with an exact `cua-perception` artifact. If those authenticated records are
missing or disagree with this summary, do not publish or redistribute the
artifact.

The default Cua Driver remains a separate MIT-licensed component and does not
include the extension, model weights, or ONNX Runtime library.

## Before you install, redistribute, or host the extension

This section summarizes known precautions. It is not legal advice. Consult
your own counsel before redistributing the extension or offering it as part of
a service.

- **The Driver license does not cover the extension.** Cua Driver stays MIT
  licensed with or without the extension, because it talks to the extension
  worker as a separate process over a protocol. The extension artifact has its
  own licenses, and its OmniParser detector is AGPL-3.0-only. Cua does not
  relicense that detector and does not describe it as MIT.
- **The extension is opt-in.** A default Driver installation never installs the
  extension or downloads model weights. If your organization does not accept
  AGPL-licensed components, do not install the extension. Driver keeps working
  and `parse_visual_regions` returns `not_installed`.
- **Private use is unrestricted.** Running the extension on your own machines
  does not trigger the AGPL's source-distribution obligations.
- **Redistribution carries the AGPL obligations.** If you copy the extension
  artifact to anyone else, include the AGPL-3.0 license text, the notices, the
  model and source ledgers, the SBOM, and the corresponding source, including
  the pinned source model and the conversion/export material listed below.
- **Hosted services can trigger the network-use clause.** The shipped ONNX file
  is a converted, and therefore modified, version of the upstream model. If
  users interact with that model remotely through your service, AGPL-3.0
  section 13 can require you to offer those users its corresponding source.
  This applies to any product that embeds Cua Driver with the extension enabled.
- **The corresponding source has an upstream limit.** Cua provides the pinned
  upstream checkpoint, the conversion recipe, and the export script. The
  upstream model repository does not publish training data or training code,
  and Cua cannot supply them.
- **The upstream origin is an Ultralytics YOLO model.** The OmniParser icon
  detector is a fine-tuned YOLO model, and its AGPL-3.0 terms come from
  Ultralytics. Ultralytics sells separate commercial licenses. Cua cannot grant
  any right beyond the AGPL-3.0 terms, so you need your own agreement with the
  rights holders if you need different terms.
- **No warranty.** The extension and its model artifacts are provided without
  warranty, as their licenses state. Detection output can be wrong. Do not use
  it as the sole basis for irreversible actions.

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
AGPL-3.0-only license and notices and provide the corresponding source and
pinned conversion/export material required for this artifact. Network use of a
covered modified version may also trigger the license's corresponding-source
requirements. The artifact must not be presented as MIT merely because Driver
invokes it through an extension protocol.

The OmniParser source repository's license for repository content and the
separate model repository's artifact terms are different records. Verify the
exact model revision above.

## PP-OCRv5

The reviewed OCR inputs are official PaddlePaddle model repositories and are
recorded as Apache-2.0:

| Role | Repository revision | Reviewed ONNX SHA-256 |
| --- | --- | --- |
| Mobile detector | `PaddlePaddle/PP-OCRv5_mobile_det_onnx` at `e6f4fa85f00e168c862bc462aebca69eef9b3d3d` | `a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d` |
| English recognizer | `PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` at `3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b` | `b5f833dfc5d0eb71da397b4efa06ebeee9b431b690a47d6af40d77d8eabc557f` |

The recognizer dictionary comes from the pinned recognizer repository's
`inference.yml`, SHA-256
`27e91d0582f40168aa218303c76e184bc78fa7a5d105aad0cfbad8458b441067`.
Preserve the Apache-2.0 license and any notices in the assembled artifact.

## ONNX Runtime and conversion provenance

The worker loads a CPU-only ONNX Runtime dynamic library from the extension
package. Its upstream is Microsoft's
[`microsoft/onnxruntime`](https://github.com/microsoft/onnxruntime) project,
which publishes ONNX Runtime under the MIT license. The reviewed candidate lock
pins official CPU-only ONNX Runtime 1.26.0 archives for each supported target.
Assembly must verify the selected archive and library hashes and record the
exact version, target, SHA-256, license, notices, and source location in the
artifact manifest and SBOM. These candidate records are not evidence that a
particular runtime shipped.

The reviewed OmniParser conversion used Python 3.12.13, PyTorch 2.8.0,
torchvision 0.23.0, Ultralytics 8.3.199, ONNX 1.19.0, ONNX Runtime 1.22.1, opset
17, a fixed 1280 input, graph simplification, and no model-side NMS. These are
conversion-provenance inputs; the Rust worker has no Python runtime dependency.
The ONNX Runtime version used during conversion does not establish which
runtime library was packaged for a release.
