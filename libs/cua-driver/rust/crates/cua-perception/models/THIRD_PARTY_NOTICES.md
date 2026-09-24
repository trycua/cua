# Cua Perception Third-Party Notices

This Cua Perception bundle contains the following separately licensed
components. Exact revisions and SHA-256 digests are recorded in
`model-ledger.json` and `source-inventory.json` in each bundle. The full
license texts are in this `notices/` directory:

- `AGPL-3.0-only.txt`: GNU Affero General Public License, version 3.
- `Apache-2.0.txt`: Apache License, Version 2.0.
- `onnxruntime-LICENSE.txt` and `onnxruntime-ThirdPartyNotices.txt`: the MIT
  License and third-party notices shipped with the pinned ONNX Runtime archive.

Cua Driver and the Cua Perception worker are MIT licensed. The extension as a
whole is not: each component below keeps its own license.

## ONNX Runtime

Copyright Microsoft Corporation. The bundle includes a CPU-only library from
the official <https://github.com/microsoft/onnxruntime> release `v1.26.0`, which
is licensed under the MIT License. Its license and third-party notices are
copied verbatim from the pinned release archive. The bundle's manifest and SBOM
identify the exact archive and library.

## OmniParser icon detector

The converted icon detector derives from Microsoft OmniParser v2.0 revision
`6600256cb0f1b07651e3bc86166196307bad7e2d`. This source model and the converted
detector are licensed under AGPL-3.0-only, which comes from the Ultralytics
YOLO model the detector was fine-tuned from. Cua distributes the detector under
that license and does not relicense it. `SOURCE_OFFER.md` describes the bundled
corresponding source. If you redistribute this bundle, or let users interact
with the detector over a network, the AGPL-3.0 obligations apply to you.

## Bundled exporter source snapshots

The bundle includes a pinned Ultralytics source snapshot licensed under
AGPL-3.0-only, PyTorch and torchvision snapshots licensed under BSD-3-Clause,
and ONNX and OnnxSlim snapshots licensed under Apache-2.0. Each snapshot
archive keeps its own license file. Exact revisions, archive hashes, and
required contents are in `source-ledger.json`.

## PaddleOCR models and dictionary

The PP-OCRv5 mobile detector, English recognizer, and recognizer dictionary
derive from `PaddlePaddle/PP-OCRv5_mobile_det_onnx` revision
`e6f4fa85f00e168c862bc462aebca69eef9b3d3d` and
`PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` revision
`3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b`. These artifacts are licensed under
the Apache License 2.0.

The components are provided without warranty, as their licenses state. This
notice is informational, is not legal advice, and does not replace the
applicable licenses.
