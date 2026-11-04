#!/usr/bin/env python3
"""Re-run the reviewed OmniParser detector export and verify its exact output."""

from __future__ import annotations

import argparse
from hashlib import sha256
from pathlib import Path
import shutil

EXPECTED_INPUT = "dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57"
EXPECTED_OUTPUT = "d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2"
EXPECTED_VERSIONS = {
    "torch": "2.8.0",
    "torchvision": "0.23.0",
    "ultralytics": "8.3.199",
    "onnx": "1.19.0",
    "onnxslim": "0.1.68",
}


def digest(path: Path) -> str:
    return sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_model", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    if digest(args.source_model) != EXPECTED_INPUT:
        parser.error("source model SHA-256 differs from the sealed source input")

    import onnx  # type: ignore[import-not-found]
    import onnxslim  # type: ignore[import-not-found]
    import torch  # type: ignore[import-not-found]
    import torchvision  # type: ignore[import-not-found]
    import ultralytics  # type: ignore[import-not-found]
    from ultralytics import YOLO  # type: ignore[import-not-found]

    actual_versions = {
        "torch": torch.__version__.split("+")[0],
        "torchvision": torchvision.__version__.split("+")[0],
        "ultralytics": ultralytics.__version__,
        "onnx": onnx.__version__,
        "onnxslim": onnxslim.__version__,
    }
    if actual_versions != EXPECTED_VERSIONS:
        parser.error(f"exporter versions differ: expected {EXPECTED_VERSIONS}, got {actual_versions}")

    exported = Path(YOLO(str(args.source_model)).export(
        format="onnx", imgsz=1280, opset=17, simplify=True, dynamic=False, nms=False
    ))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if exported.resolve() != args.output.resolve():
        shutil.copyfile(exported, args.output)
    if digest(args.output) != EXPECTED_OUTPUT:
        parser.error("export completed, but output differs from the reviewed ONNX digest")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
