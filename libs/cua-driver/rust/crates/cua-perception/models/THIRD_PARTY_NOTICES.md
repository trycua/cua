# Cua Perception Third-Party Notices

The sealed Cua Perception bundle contains the following separately licensed
components. Exact revisions and SHA-256 digests are recorded in
`model-ledger.json` and `source-inventory.json` in each bundle.

## ONNX Runtime

Copyright Microsoft Corporation. The reviewed candidate lock pins official
CPU-only archives from <https://github.com/microsoft/onnxruntime> release
`v1.26.0`, which is licensed under the MIT License. The assembled bundle's
manifest and SBOM identify the archive and library actually included.

## OmniParser icon detector

The converted icon detector derives from Microsoft OmniParser v2.0 revision
`6600256cb0f1b07651e3bc86166196307bad7e2d`. The release ledger records this
specific source model and converted detector artifact as AGPL-3.0-only.
Publication and redistribution remain blocked pending the release review
recorded in the model ledger. The corresponding-source review payload is
described in `SOURCE_OFFER.md`.

## Bundled exporter source snapshots

The review bundle includes a pinned Ultralytics source snapshot recorded as
AGPL-3.0-only, PyTorch and torchvision snapshots recorded as BSD-3-Clause, and
ONNX and OnnxSlim snapshots recorded as Apache-2.0. Exact revisions, archive
hashes, and required contents are in `source-ledger.json`. These artifact-level
records apply only to the named snapshots. Their inclusion supports review and
reproduction of the detector conversion but does not complete the pending
detector license review.

## Bundled exporter source snapshots

The review bundle includes pinned source snapshots for Ultralytics
(AGPL-3.0-only), PyTorch and torchvision (BSD-3-Clause), and ONNX and OnnxSlim
(Apache-2.0). Exact revisions, archive hashes, and required contents are in
`source-ledger.json`. These snapshots support review and reproduction of the
detector conversion; their inclusion does not complete the pending detector
license review.

## PaddleOCR models and dictionary

The PP-OCRv5 mobile detector, English recognizer, and recognizer dictionary
derive from `PaddlePaddle/PP-OCRv5_mobile_det_onnx` revision
`e6f4fa85f00e168c862bc462aebca69eef9b3d3d` and
`PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` revision
`3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b`. These specific artifacts are
recorded under the Apache License 2.0.

This notice is informational and does not replace the applicable licenses.
