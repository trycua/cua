# Cua Perception Third-Party Notices

The sealed Cua Perception bundle contains the following separately licensed
components. Exact revisions and SHA-256 digests are recorded in
`model-ledger.json` and `source-inventory.json` in each bundle.

## ONNX Runtime

Copyright Microsoft Corporation. Distributed under the MIT License from
<https://github.com/microsoft/onnxruntime> release `v1.26.0`.

## OmniParser icon detector

The converted icon detector derives from Microsoft OmniParser v2.0 revision
`6600256cb0f1b07651e3bc86166196307bad7e2d`. The detector is distributed under
AGPL-3.0-only. Distribution remains subject to the release review recorded in
the model ledger. The corresponding source offer is described in
`SOURCE_OFFER.md`.

## Bundled exporter source snapshots

The review bundle includes pinned source snapshots for Ultralytics
(AGPL-3.0-only), PyTorch and torchvision (BSD-3-Clause), and ONNX and OnnxSlim
(Apache-2.0). Exact revisions, archive hashes, and required contents are in
`source-ledger.json`. These snapshots support review and reproduction of the
detector conversion; their inclusion does not complete the pending detector
license review.

## PaddleOCR models and dictionary

The PP-OCRv5 mobile detector, English recognizer, and recognizer dictionary
derive from pinned PaddlePaddle repositories and are distributed under the
Apache License 2.0.

This notice is informational and does not replace the applicable licenses.
