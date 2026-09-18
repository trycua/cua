"""Regenerate the synthetic ONNX identity model used by the Rust smoke test."""

import hashlib
from pathlib import Path

import onnx
from onnx import TensorProto, helper

shape = [1, 3, 2, 2]
graph = helper.make_graph(
    [helper.make_node("Identity", ["input"], ["output"])],
    "cua-perception-identity-fixture",
    [helper.make_tensor_value_info("input", TensorProto.FLOAT, shape)],
    [helper.make_tensor_value_info("output", TensorProto.FLOAT, shape)],
)
model = helper.make_model(
    graph,
    producer_name="cua-perception-inference-spike",
    opset_imports=[helper.make_opsetid("", 18)],
)
model.ir_version = 10
onnx.checker.check_model(model)
output_path = Path(__file__).parent / "fixtures" / "identity.onnx"
onnx.save(model, output_path)

EXPECTED_SHA256 = "283c2228ceb4ebea062c89fce11e3008d65a4788bae026d9adf509e484d97df4"
actual_sha256 = hashlib.sha256(output_path.read_bytes()).hexdigest()
if actual_sha256 != EXPECTED_SHA256:
    raise RuntimeError(
        f"fixture hash mismatch: expected {EXPECTED_SHA256}, got {actual_sha256}"
    )
