"""Regenerate the small CPU inference fixture using the ONNX Python API."""

from pathlib import Path

import onnx
from onnx import TensorProto, helper


def main():
    graph = helper.make_graph(
        [
            helper.make_node("Mul", ["input", "scale"], ["scaled"]),
            helper.make_node("Add", ["scaled", "bias"], ["output"]),
        ],
        "affine",
        [helper.make_tensor_value_info("input", TensorProto.FLOAT, [3])],
        [helper.make_tensor_value_info("output", TensorProto.FLOAT, [3])],
        [
            helper.make_tensor("scale", TensorProto.FLOAT, [], [2.0]),
            helper.make_tensor("bias", TensorProto.FLOAT, [], [1.0]),
        ],
    )
    model = helper.make_model(
        graph,
        producer_name="oxel-example",
        ir_version=8,
        opset_imports=[helper.make_opsetid("", 13)],
    )
    onnx.checker.check_model(model, full_check=True)
    onnx.save_model(model, Path(__file__).with_name("affine.onnx"))


if __name__ == "__main__":
    main()
