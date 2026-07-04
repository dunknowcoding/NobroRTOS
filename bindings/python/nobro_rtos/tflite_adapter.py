"""TFLite -> NobroRTOS model import (M136): adapt an EXISTING tool's model to run on
NobroRTOS, rather than reimplement it.

`nobro_ai` is the device runtime for already-trained, quantized models; this host-side
adapter is its front door for the TensorFlow world. It reads an int8 `.tflite` model
(the standard TFLite-Micro deployment format), extracts the fully-connected layer's int8
weights + per-tensor quantization, and emits the same manifest + weight blob the on-device
`nobro_ai` / `nobro_nn` int8 kernel consumes. So a model trained in TF/Keras and quantized
to int8 deploys onto NobroRTOS unchanged.

`tensorflow` is needed only to *read* the .tflite (via tf.lite.Interpreter); the conversion
and the reference int8 inference are plain Python, so the produced manifest is inspectable
and the on-device parity is checkable.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass
class TfliteDenseModel:
    """A single-fully-connected int8 classifier lifted out of a .tflite file."""

    input_len: int
    output_len: int
    weights: list[int]  # int8, [OUT][IN] row-major
    bias: list[int]  # int32, accumulator units
    input_scale: float
    input_zero: int
    weight_scale: float

    def quantize_input(self, x: list[float]) -> list[int]:
        """Quantize a float input to int8 the way TFLite's input tensor does."""
        out = []
        for v in x:
            q = round(v / self.input_scale) + self.input_zero
            out.append(max(-128, min(127, q)))
        return out

    def infer(self, x: list[float]) -> int:
        """Reference int8 inference (matches nobro_nn::dense_int8 + argmax). Returns the
        predicted class. Exact for argmax because the per-output scale is shared."""
        xq = self.quantize_input(x)
        best_j, best_acc = 0, None
        for j in range(self.output_len):
            acc = self.bias[j]
            row = self.weights[j * self.input_len:(j + 1) * self.input_len]
            for w, xv in zip(row, xq):
                acc += w * (xv - self.input_zero)
            if best_acc is None or acc > best_acc:
                best_acc, best_j = acc, j
        return best_j

    def to_manifest(self) -> dict:
        """The device-shaped manifest (same shape nn_export emits for nobro_ai)."""
        return {
            "source": "tflite",
            "input_len": self.input_len,
            "output_len": self.output_len,
            "weight_scale_milli": round(self.weight_scale * 1000),
            "input_scale_milli": round(self.input_scale * 1000),
            "input_zero": self.input_zero,
            "weights_len": len(self.weights),
        }


def load_tflite_dense(path: str) -> TfliteDenseModel:
    """Import an int8 .tflite dense classifier into a TfliteDenseModel via the TFLite
    interpreter (the authoritative reader of the FlatBuffer format)."""
    import tensorflow as tf  # only needed to read the .tflite

    interp = tf.lite.Interpreter(model_path=path)
    interp.allocate_tensors()
    details = interp.get_tensor_details()

    inp = interp.get_input_details()[0]
    out = interp.get_output_details()[0]
    input_scale, input_zero = inp["quantization"]
    input_len = int(inp["shape"][-1])
    output_len = int(out["shape"][-1])

    # the FullyConnected weights are the int8 2-D constant [OUT][IN]; bias is int32 1-D
    weights = None
    weight_scale = 1.0
    bias = None
    for d in details:
        arr = None
        try:
            arr = interp.get_tensor(d["index"])
        except ValueError:
            continue
        if arr is None:
            continue
        if arr.dtype.name == "int8" and arr.ndim == 2 and arr.shape == (output_len, input_len):
            weights = [int(v) for v in arr.flatten()]
            q = d["quantization"]
            weight_scale = float(q[0]) if q and q[0] else 1.0
        elif arr.dtype.name == "int32" and arr.ndim == 1 and arr.shape[0] == output_len:
            bias = [int(v) for v in arr.flatten()]

    if weights is None:
        raise ValueError("no int8 [OUT][IN] FullyConnected weights found in the .tflite")
    if bias is None:
        bias = [0] * output_len

    return TfliteDenseModel(
        input_len=input_len,
        output_len=output_len,
        weights=weights,
        bias=bias,
        input_scale=float(input_scale),
        input_zero=int(input_zero),
        weight_scale=weight_scale,
    )
