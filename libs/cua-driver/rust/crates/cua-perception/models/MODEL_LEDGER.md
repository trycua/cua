# Cua Perception Model Ledger

This directory records the reviewed inputs for an offline worker package. It
does not contain weights. A distributor must place the named files beside a
copy of `model-manifest.template.json`, replace the platform-specific ONNX
Runtime fields, and preserve the listed hashes. The worker performs no network
access and refuses missing or mismatched files before creating a model session.

## OmniParser icon detector

- Source: `microsoft/OmniParser-v2.0` on Hugging Face
- Revision: `6600256cb0f1b07651e3bc86166196307bad7e2d`
- Immutable source URL: `https://huggingface.co/microsoft/OmniParser-v2.0/resolve/6600256cb0f1b07651e3bc86166196307bad7e2d/icon_detect/model.pt`
- Source file: `icon_detect/model.pt`
- Source SHA-256: `dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57`
- License shipped with the detector: AGPL-3.0
- Converted file: `omniparser-icon-detect-1280-opset17.onnx`
- Converted SHA-256: `d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2`
- Converted size: 80,933,219 bytes
- Input: `images`, float32 `[1,3,1280,1280]`
- Output: `output0`, float32 `[1,5,33600]`, YOLOv8-style `cx,cy,w,h,class_score`

The reviewed conversion used Python 3.12.13, PyTorch 2.8.0, torchvision
0.23.0, ultralytics 8.3.199, ONNX 1.19.0, ONNX Runtime 1.22.1, opset 17,
fixed 1280 input, graph simplification enabled, and model-side NMS disabled.
This records conversion provenance; the Rust worker itself has no Python
runtime dependency. The converted file is an explicitly supplied release
input because the reviewed byte-for-byte export is not published at the
upstream source URL. The assembler rejects any substitute digest.

Every assembled review bundle also contains the exact `model.pt`, pinned source
snapshots for Ultralytics, PyTorch, torchvision, and ONNX, and the executable
command in `export_omniparser_detector.py`. `conversion-recipe.json` is the
machine-readable authority for parameters and expected output. No local patch
was applied. These principal sources do not include every transitive build or
host dependency, so successful reproduction must still match the recorded
ONNX digest and does not by itself complete the pending license review.

## PP-OCRv5 detector

- Source: official PaddlePaddle `PP-OCRv5_mobile_det_onnx` Hugging Face repository
- Revision: `e6f4fa85f00e168c862bc462aebca69eef9b3d3d`
- Immutable ONNX URL: `https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_det_onnx/resolve/e6f4fa85f00e168c862bc462aebca69eef9b3d3d/inference.onnx`
- ONNX SHA-256: `a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d`
- `inference.yml` SHA-256: `98069072e1b6b37d727fd9d9f11725faa46d6ea0de012f2ed26caea011c37699`
- License: Apache-2.0
- Input/output: `x` / `fetch_name_0`
- Reviewed preprocessing: BGR, `1/255`, ImageNet mean/std, CHW
- Reviewed postprocessing: DB threshold 0.3, box threshold 0.6, unclip ratio 1.5

## PP-OCRv5 English recognizer

- Source: official PaddlePaddle `en_PP-OCRv5_mobile_rec_onnx` Hugging Face repository
- Revision: `3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b`
- Immutable ONNX URL: `https://huggingface.co/PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx/resolve/3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b/inference.onnx`
- Immutable dictionary URL: `https://huggingface.co/PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx/resolve/3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b/inference.yml`
- ONNX SHA-256: `b5f833dfc5d0eb71da397b4efa06ebeee9b431b690a47d6af40d77d8eabc557f`
- `inference.yml` SHA-256: `27e91d0582f40168aa218303c76e184bc78fa7a5d105aad0cfbad8458b441067`
- License: Apache-2.0
- Input/output: `x` / `fetch_name_0`
- Reviewed input: BGR float32 `[1,3,48,320]`
- Dictionary: `PostProcess.character_dict` in the hashed `inference.yml`

## Packaging checklist

1. Obtain each artifact from its pinned source revision and verify SHA-256.
2. Convert OmniParser with the recorded toolchain and compare the converted
   SHA-256; a different conversion must receive a new ledger entry.
3. Package a CPU-only ONNX Runtime dynamic library and record its exact version
   and SHA-256 in the copied manifest. Set `intra_threads` to the bounded CPU
   concurrency selected for that package and target.
4. Keep every artifact path relative to the manifest. Absolute paths, parent
   traversal, symlinks, empty files, and hash mismatches are rejected.
5. Start with `cua-perception --manifest <path> --onnx-runtime-library <path>`.
   Use `--fixture` only for deterministic protocol tests.

PP-OCR DB detections are returned as axis-aligned bounds. Rotated probability
map regions are not perspective-corrected before recognition; health and parse
identity expose this limitation so a caller can decide whether it is suitable.

## ONNX Runtime CPU library

All three targets use the official CPU-only archives at immutable tag
`v1.26.0`. The assembler verifies each platform archive, extracts exactly one
target member, and verifies its digest:

- macOS arm64 archive `7a1280bbb1701ea514f71828765237e7896e0f2e1cd332f1f70dbd5c3e33aca3`; `libonnxruntime.dylib` `cb0462c3fd35ad722e8772313030a33c182f3d4c6b33f4e5e1fcb2ce3199b86c`
- Windows x64 archive `6ebe99b5564bf4d029b6e93eac9ff423682b6212eade769e9ca3f685eaf500b4`; `onnxruntime.dll` `b2ba7ca16e0e4fe71ad5148744ab885a2f5809e52a0c3de4d9ba3853a03977f9`
- Linux x64 archive `1254da24fb389cf39dc0ff3451ab48301740ffbfcbaf646849df92f80ee92c57`; `libonnxruntime.so` `5bd5bedf736fc501692435d0ec4f6e8b2bdf48cd30af8e6d00d61b3ddc9a7ab8`

`scripts/artifacts.lock.json` is the machine-readable authority for URLs,
archive members, sizes, target identities, and hashes. `SOURCE_OFFER.md` and
`THIRD_PARTY_NOTICES.md` must accompany every assembled bundle.
