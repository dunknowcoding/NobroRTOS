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
PACKED_DENSE_LAYOUT_VERSION = 1


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


def _round_away_from_zero(value: float) -> int:
    """Deterministic tie handling shared by Q4/Q2 export paths."""
    if not math.isfinite(value):
        raise ValueError("quantization input must be finite")
    return math.floor(value + 0.5) if value >= 0 else math.ceil(value - 0.5)


def _packed_parameters(format: str) -> tuple[int, int, int]:
    normalized = format.lower()
    if normalized == "q4":
        return 4, -7, 7
    if normalized == "q2":
        return 2, -1, 1
    raise ValueError("packed format must be 'q4' or 'q2'")


def quantize_packed(
    values: list[float], columns: int, format: str
) -> tuple[bytes, int, list[int]]:
    """Quantize and row-byte-pack a dense [OUT][IN] weight matrix.

    Low lanes are stored first; every output row starts on a byte boundary and
    unused high lanes are zero. Returns `(blob, scale_micros, quantized_values)`.
    """
    bits, lower, upper = _packed_parameters(format)
    if columns <= 0 or not values or len(values) % columns:
        raise ValueError("packed weights require a non-empty rectangular matrix")
    if any(not math.isfinite(value) for value in values):
        raise ValueError("quantization input must be finite")
    peak = max(abs(value) for value in values)
    scale = peak / upper if peak > 0 else 1.0 / 1_000_000.0
    scale_micros = max(1, _round_away_from_zero(scale * 1_000_000.0))
    exact_scale = scale_micros / 1_000_000.0
    quantized = [
        max(lower, min(upper, _round_away_from_zero(value / exact_scale)))
        for value in values
    ]
    lanes = 8 // bits
    rows = len(values) // columns
    row_bytes = (columns + lanes - 1) // lanes
    blob = bytearray(row_bytes * rows)
    mask = (1 << bits) - 1
    for row in range(rows):
        for column in range(columns):
            value = quantized[row * columns + column]
            byte = row * row_bytes + column // lanes
            shift = (column % lanes) * bits
            blob[byte] |= (value & mask) << shift
    return bytes(blob), scale_micros, quantized


def unpack_packed(blob: bytes, rows: int, columns: int, format: str) -> list[int]:
    """Decode the stable packed layout for reference tests and inspection."""
    bits, _, _ = _packed_parameters(format)
    if rows <= 0 or columns <= 0:
        raise ValueError("packed shape must be non-empty")
    lanes = 8 // bits
    row_bytes = (columns + lanes - 1) // lanes
    if len(blob) != rows * row_bytes:
        raise ValueError("packed blob length does not match shape")
    sign = 1 << (bits - 1)
    modulus = 1 << bits
    values: list[int] = []
    for row in range(rows):
        for column in range(columns):
            raw = (blob[row * row_bytes + column // lanes] >> ((column % lanes) * bits)) & (modulus - 1)
            values.append(raw - modulus if raw & sign else raw)
    return values


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


@dataclass
class PackedExportedModel:
    name: str
    version: int
    input_len: int
    output_len: int
    format: str
    input_scale_micros: int
    weight_scale_micros: int
    output_scale_micros: int
    input_offset: int
    output_offset: int
    multiplier: int
    shift: int
    activation_min: int
    activation_max: int
    packed_weights: bytes
    bias: tuple[int, ...]
    weights_crc: int

    @property
    def maturity(self) -> str:
        return "candidate" if self.format == "q4" else "experimental"

    def manifest_fields(self) -> dict:
        return {
            "magic": MODEL_MAGIC,
            "name": self.name,
            "version": self.version,
            "layout_version": PACKED_DENSE_LAYOUT_VERSION,
            "input_len": self.input_len,
            "output_len": self.output_len,
            "format": self.format,
            "input_scale_micros": self.input_scale_micros,
            "weight_scale_micros": self.weight_scale_micros,
            "output_scale_micros": self.output_scale_micros,
            "input_offset": self.input_offset,
            "output_offset": self.output_offset,
            "multiplier": self.multiplier,
            "shift": self.shift,
            "activation_min": self.activation_min,
            "activation_max": self.activation_max,
            "weights_crc": self.weights_crc,
            "weights_len": len(self.packed_weights),
            "maturity": self.maturity,
        }

    def dequantized_weights(self) -> list[float]:
        scale = self.weight_scale_micros / 1_000_000.0
        return [
            value * scale
            for value in unpack_packed(
                self.packed_weights, self.output_len, self.input_len, self.format
            )
        ]

    def dequantized_bias(self) -> list[float]:
        scale = (
            self.input_scale_micros
            * self.weight_scale_micros
            / 1_000_000_000_000.0
        )
        return [value * scale for value in self.bias]

    def infer(self, inputs: list[float]) -> tuple[list[int], list[float]]:
        """Run the exported integer contract and return raw and real outputs."""
        if len(inputs) != self.input_len:
            raise ValueError("input does not match packed model shape")
        input_scale = self.input_scale_micros / 1_000_000.0
        raw_input = [
            max(
                -128,
                min(127, _round_away_from_zero(value / input_scale) - self.input_offset),
            )
            for value in inputs
        ]
        weights = unpack_packed(
            self.packed_weights, self.output_len, self.input_len, self.format
        )
        raw_output: list[int] = []
        for row in range(self.output_len):
            accumulator = self.bias[row]
            for column in range(self.input_len):
                accumulator += (raw_input[column] + self.input_offset) * weights[
                    row * self.input_len + column
                ]
            value = _cmsis_requantize(accumulator, self.multiplier, self.shift)
            value += self.output_offset
            raw_output.append(max(self.activation_min, min(self.activation_max, value)))
        output_scale = self.output_scale_micros / 1_000_000.0
        real_output = [
            (value - self.output_offset) * output_scale for value in raw_output
        ]
        return raw_output, real_output


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


def export_packed_model(
    name: str,
    version: int,
    weights: list[float],
    bias: list[float],
    format: str = "q4",
    input_len: int | None = None,
    input_scale: float = 1.0,
    output_scale: float | None = None,
    input_offset: int = 0,
    output_offset: int = 0,
) -> PackedExportedModel:
    """Export Q4/Q2 weights plus bounded i32 accumulator bias.

    Q4 is a promotion candidate after target admission/equivalence gates. Q2 is
    intentionally marked experimental because its accuracy loss is workload
    dependent; callers must evaluate it before deployment.
    """
    out_len = len(bias)
    if out_len == 0:
        raise ValueError("bias/output shape must be non-empty")
    in_len = input_len if input_len is not None else len(weights) // out_len
    if in_len <= 0 or len(weights) != in_len * out_len:
        raise ValueError("weights do not match the requested dense shape")
    normalized = format.lower()
    if not math.isfinite(input_scale) or input_scale <= 0:
        raise ValueError("input_scale must be finite and positive")
    if input_offset < -127 or input_offset > 128 or output_offset < -127 or output_offset > 128:
        raise ValueError("input/output offsets must be in -127..=128")
    blob, weight_scale_micros, _ = quantize_packed(weights, in_len, normalized)
    input_scale_micros = max(1, _round_away_from_zero(input_scale * 1_000_000.0))
    exact_input_scale = input_scale_micros / 1_000_000.0
    exact_weight_scale = weight_scale_micros / 1_000_000.0
    accumulator_scale = exact_input_scale * exact_weight_scale
    requested_output_scale = accumulator_scale if output_scale is None else output_scale
    if not math.isfinite(requested_output_scale) or requested_output_scale <= 0:
        raise ValueError("output_scale must be finite and positive")
    output_scale_micros = max(
        1, _round_away_from_zero(requested_output_scale * 1_000_000.0)
    )
    exact_output_scale = output_scale_micros / 1_000_000.0
    multiplier, shift = _quantize_multiplier(accumulator_scale / exact_output_scale)
    bias_q = tuple(_round_away_from_zero(value / accumulator_scale) for value in bias)
    if any(value < -(1 << 31) or value > (1 << 31) - 1 for value in bias_q):
        raise ValueError("bias does not fit the signed i32 accumulator contract")
    return PackedExportedModel(
        name=name,
        version=version,
        input_len=in_len,
        output_len=out_len,
        format=normalized,
        input_scale_micros=input_scale_micros,
        weight_scale_micros=weight_scale_micros,
        output_scale_micros=output_scale_micros,
        input_offset=input_offset,
        output_offset=output_offset,
        multiplier=multiplier,
        shift=shift,
        activation_min=-128,
        activation_max=127,
        packed_weights=blob,
        bias=bias_q,
        weights_crc=fnv1a(blob),
    )


def _quantize_multiplier(real_multiplier: float) -> tuple[int, int]:
    if not math.isfinite(real_multiplier) or real_multiplier <= 0:
        raise ValueError("requantization multiplier must be finite and positive")
    significand, shift = math.frexp(real_multiplier)
    multiplier = _round_away_from_zero(significand * (1 << 31))
    if multiplier == 1 << 31:
        multiplier //= 2
        shift += 1
    if shift < -31 or shift > 30 or multiplier <= 0:
        raise ValueError("requantization scale is outside the device range")
    return multiplier, shift


def _cmsis_requantize(value: int, multiplier: int, shift: int) -> int:
    left_shift = max(shift, 0)
    right_shift = max(-shift, 0)
    shifted_bits = (value * (1 << left_shift)) & 0xFFFFFFFF
    shifted = shifted_bits if shifted_bits < 0x80000000 else shifted_bits - 0x100000000
    high = ((1 << 30) + shifted * multiplier) >> 31
    if right_shift == 0:
        return high
    mask = (1 << right_shift) - 1
    remainder = high & mask
    result = high >> right_shift
    threshold = (mask >> 1) + int(result < 0)
    return result + int(remainder > threshold)
