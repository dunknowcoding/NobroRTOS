"""Host-side NN training + export for the nobro-nn / nobro-ai device crates.

The division of labor: the device runs `nobro-nn` inference blocks over flat weight
arrays; this module is where those arrays come from. It trains small dense models in
pure Python (no framework needed for MCU-sized problems), quantizes them to symmetric
int8, and packs a weight blob + manifest whose magic/checksum match `nobro-ai`'s
`ModelManifest::validate` exactly - a bad export fails on-device at load, not at
inference.

    from nobro_rtos.nn_export import train_dense, quantize_int8, export_model

    w, b = train_dense(samples, labels, in_len=2, out_len=2, epochs=200)
    blob, manifest = export_model("gate-net", 1, w, b)
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

MODEL_MAGIC = 0x4E42_4D4C  # "NBML", must match nobro_ai::MODEL_MAGIC


def fnv1a(data: bytes) -> int:
    """FNV-1a 32-bit - byte-for-byte the checksum nobro-ai validates."""
    h = 0x811C9DC5
    for b in data:
        h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
    return h


# ----------------------------------------------------------------- inference (reference)

def dense(inputs: list[float], weights: list[float], bias: list[float]) -> list[float]:
    """Reference implementation of nobro_nn::dense ([OUT][IN] row-major weights)."""
    n_in = len(inputs)
    n_out = len(bias)
    return [
        sum(weights[j * n_in + i] * inputs[i] for i in range(n_in)) + bias[j]
        for j in range(n_out)
    ]


def softmax(xs: list[float]) -> list[float]:
    m = max(xs)
    es = [math.exp(x - m) for x in xs]
    s = sum(es)
    return [e / s for e in es]


# ----------------------------------------------------------------- training

def train_dense(
    samples: list[list[float]],
    labels: list[int],
    in_len: int,
    out_len: int,
    epochs: int = 300,
    lr: float = 0.05,
) -> tuple[list[float], list[float]]:
    """Train a single dense layer + softmax with cross-entropy gradient descent.

    Pure Python on purpose: MCU-scale models are tiny, and users should be able to
    retrain without installing a framework. Returns ([out][in] weights, bias).
    """
    w = [0.0] * (out_len * in_len)
    b = [0.0] * out_len
    for _ in range(epochs):
        for x, y in zip(samples, labels):
            p = softmax(dense(x, w, b))
            for j in range(out_len):
                grad = p[j] - (1.0 if j == y else 0.0)
                b[j] -= lr * grad
                for i in range(in_len):
                    w[j * in_len + i] -= lr * grad * x[i]
    return w, b


def evaluate(
    samples: list[list[float]],
    labels: list[int],
    weights: list[float],
    bias: list[float],
) -> float:
    """Classification accuracy of the trained layer."""
    hits = 0
    for x, y in zip(samples, labels):
        out = dense(x, weights, bias)
        if out.index(max(out)) == y:
            hits += 1
    return hits / len(samples) if samples else 0.0


# ----------------------------------------------------------------- quantization + export

def quantize_int8(values: list[float]) -> tuple[bytes, int]:
    """Symmetric per-tensor int8 quantization; returns (bytes, scale_milli).

    scale_milli is the dequant step in milli-units, matching
    nobro_ai::WeightFormat::Int8 { scale_milli }: real = int8 * scale_milli / 1000.
    """
    peak = max((abs(v) for v in values), default=0.0)
    scale = peak / 127.0 if peak > 0 else 1.0 / 1000.0
    scale_milli = max(1, round(scale * 1000))
    q = bytes(
        struct.pack("b", max(-127, min(127, round(v / (scale_milli / 1000.0)))))[0]
        for v in values
    )
    return q, scale_milli


def dequantize_int8(blob: bytes, scale_milli: int) -> list[float]:
    return [struct.unpack("b", bytes([raw]))[0] * scale_milli / 1000.0 for raw in blob]


@dataclass
class ExportedModel:
    name: str
    version: int
    input_len: int
    output_len: int
    scale_milli: int
    weights: bytes  # quantized [OUT][IN] weights then bias, one blob
    weights_crc: int

    def manifest_fields(self) -> dict:
        """The fields a device-side nobro_ai::ModelManifest is built from."""
        return {
            "magic": MODEL_MAGIC,
            "name": self.name,
            "version": self.version,
            "input_len": self.input_len,
            "output_len": self.output_len,
            "scale_milli": self.scale_milli,
            "weights_crc": self.weights_crc,
            "weights_len": len(self.weights),
        }


def export_model(
    name: str,
    version: int,
    weights: list[float],
    bias: list[float],
    input_len: int | None = None,
) -> ExportedModel:
    """Quantize weights+bias into one blob and compute the manifest checksum."""
    out_len = len(bias)
    in_len = input_len if input_len is not None else len(weights) // max(out_len, 1)
    blob, scale_milli = quantize_int8(list(weights) + list(bias))
    return ExportedModel(
        name=name,
        version=version,
        input_len=in_len,
        output_len=out_len,
        scale_milli=scale_milli,
        weights=blob,
        weights_crc=fnv1a(blob),
    )
