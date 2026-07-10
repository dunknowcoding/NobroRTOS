#!/usr/bin/env python3
"""Train a tiny quantized MLP motion classifier for NobroRTOS, on-device inference.

Pipeline (the "NN tools -> embedded" path):
  1. synthesize IMU |accel| windows for two classes (idle / active),
  2. extract the SAME 3 integer features the firmware computes (variance, range,
     mean-abs successive diff), normalized to int8 [0,127],
  3. train a 3 -> 8 -> 2 MLP (numpy, manual backprop),
  4. quantize to int8 weights + integer inference with fixed shifts,
  5. validate that the *integer* model matches the labels, and
  6. emit core/adapters/nn-motion-ai/src/nn_weights.rs for the firmware to embed.

No PyTorch/TF needed for a model this small; numpy is enough and the export is the
real artifact. Run: python tools/train_motion_nn.py

Besides the firmware weights, this also emits a contract-shaped "model card" into the
block editor (packages/block-editor/models.json) so the browser ML block can offer this
trained model as a drop-in inference block whose app.json entry matches AiModelContract.
"""
import argparse
import json
import os
import numpy as np

WINDOW = 32           # u16 samples per inference window
HIDDEN = 8
FEAT = 3
SHIFT1 = 5            # requantize layer-1 accumulator back toward int8 before ReLU
RNG = np.random.default_rng(7)
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "core", "adapters", "nn-motion-ai", "src", "nn_weights.rs")
MODELS_JSON = os.path.join(HERE, "..", "packages", "block-editor", "models.json")

# Model identity + AiModelContract values, kept in lockstep with the firmware adapter
# core/adapters/nn-motion-ai/src/lib.rs (MODEL_ID + contract()): on-device int8 MLP,
# <=64 input bytes (<=32 u16 samples), 4 output bytes, 256-byte scratch arena, 2ms budget.
PRESET = "nn_motion"
MODEL_ID = 0x4E4E4D31           # "NNM1"
BACKEND = "on_device"
INPUT_BYTES_MAX = 64
OUTPUT_BYTES_MAX = 4
ARENA_BYTES = 256
TIMEOUT_US = 2_000
STALE_AFTER_US = 100_000        # 100ms freshness floor (AiModelContract requires > 0)
CLASSES = ["idle", "active"]


def write_model_card(models_json, acc_milli):
    """Merge this model's card into the block-editor catalog (create/update its key)."""
    card = {
        "preset": PRESET,
        "label": "Motion NN (idle/active)",
        "source": "tools/train_motion_nn.py",
        "classes": CLASSES,
        "arch": {"window": WINDOW, "hidden": HIDDEN, "feat": FEAT},
        "train_acc_milli": acc_milli,
        # The exact fields AiModelContract.to_dict() emits for app.json ai_models[].
        "contract": {
            "model_id": MODEL_ID,
            "backend": BACKEND,
            "input_bytes_max": INPUT_BYTES_MAX,
            "output_bytes_max": OUTPUT_BYTES_MAX,
            "arena_bytes": ARENA_BYTES,
            "timeout_us": TIMEOUT_US,
            "stale_after_us": STALE_AFTER_US,
        },
    }
    catalog = {}
    if os.path.exists(models_json):
        try:
            with open(models_json, encoding="utf-8") as f:
                catalog = json.load(f)
        except (json.JSONDecodeError, OSError):
            catalog = {}
    catalog[PRESET] = card
    os.makedirs(os.path.dirname(models_json), exist_ok=True)
    with open(models_json, "w", encoding="utf-8") as f:
        json.dump(catalog, f, indent=2, sort_keys=True)
    return models_json


def synth_window(active):
    base = 1000.0
    if active:
        # motion: larger noise + a low-freq sway
        t = np.arange(WINDOW)
        sway = RNG.uniform(60, 220) * np.sin(2 * np.pi * RNG.uniform(0.05, 0.2) * t + RNG.uniform(0, 6))
        noise = RNG.normal(0, RNG.uniform(40, 110), WINDOW)
        w = base + sway + noise
    else:
        # idle: small noise around 1 g
        w = base + RNG.normal(0, RNG.uniform(3, 14), WINDOW)
    return np.clip(w, 0, 4095).astype(np.int64)


def features_int(w):
    """Exactly what the firmware computes (integer math), then clamp to [0,4095]."""
    n = len(w)
    mean = int(w.sum() // n)
    var = int(((w - mean) ** 2).sum() // n)
    rng = int(w.max() - w.min())
    mad = int(np.abs(np.diff(w)).sum() // (n - 1))
    f0 = min(var >> 4, 4095)
    f1 = min(rng, 4095)
    f2 = min(mad, 4095)
    return np.array([f0, f1, f2], dtype=np.int64)


def make_dataset(n_each):
    X, y = [], []
    for _ in range(n_each):
        X.append(features_int(synth_window(False))); y.append(0)
        X.append(features_int(synth_window(True))); y.append(1)
    return np.array(X, dtype=np.float64), np.array(y, dtype=np.int64)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--models-json", default=MODELS_JSON,
                    help="block-editor model catalog to update (default: "
                         "packages/block-editor/models.json)")
    ap.add_argument("--no-model-card", action="store_true",
                    help="skip updating the block-editor model catalog")
    args = ap.parse_args()

    Xi, y = make_dataset(600)
    feat_max = np.maximum(Xi.max(axis=0), 1).astype(np.int64)
    # int8 normalized features [0,127] - the firmware uses the same feat_max.
    Xn = np.clip((Xi * 127 / feat_max), 0, 127)
    X = Xn / 127.0  # float [0,1] for training

    # --- train MLP 3 -> 8 -> 2 (numpy, manual SGD) ---
    W1 = RNG.normal(0, 0.6, (FEAT, HIDDEN)); b1 = np.zeros(HIDDEN)
    W2 = RNG.normal(0, 0.6, (HIDDEN, 2)); b2 = np.zeros(2)
    Y = np.eye(2)[y]
    lr = 0.2
    for epoch in range(900):
        z1 = X @ W1 + b1; h = np.maximum(z1, 0)
        z2 = h @ W2 + b2
        ez = np.exp(z2 - z2.max(axis=1, keepdims=True)); p = ez / ez.sum(axis=1, keepdims=True)
        g2 = (p - Y) / len(X)
        dW2 = h.T @ g2; db2 = g2.sum(axis=0)
        gh = (g2 @ W2.T) * (z1 > 0)
        dW1 = X.T @ gh; db1 = gh.sum(axis=0)
        W2 -= lr * dW2; b2 -= lr * db2; W1 -= lr * dW1; b1 -= lr * db1
    acc_float = (p.argmax(1) == y).mean()

    # --- quantize to int8 + integer inference (matches the firmware exactly) ---
    s1 = np.abs(W1).max() / 127.0
    s2 = np.abs(W2).max() / 127.0
    W1q = np.round(W1 / s1).astype(np.int32)
    W2q = np.round(W2 / s2).astype(np.int32)
    # bias in the same units as the int accumulators
    b1q = np.round(b1 / s1 * 127).astype(np.int32)   # b1 added to acc1 (scale: s1 * x_int)
    b2q = np.round(b2 / s2 * 127).astype(np.int32)

    def infer_int(xint):  # xint: int8 features [0,127]
        acc1 = (W1q.T * xint).sum(axis=1) + b1q          # (HIDDEN,)
        h = np.clip(acc1 >> SHIFT1, 0, 127)              # ReLU + requantize to int8
        acc2 = (W2q.T * h).sum(axis=1) + b2q             # (2,)
        return int(acc2.argmax()), acc2

    Xint = np.clip((Xi * 127 / feat_max), 0, 127).astype(np.int64)
    preds = np.array([infer_int(x)[0] for x in Xint])
    acc_int = (preds == y).mean()
    print(f"float acc={acc_float:.3f}  int8 acc={acc_int:.3f}  (n={len(X)})")

    # --- emit Rust ---
    def arr2d(a):
        return "[" + ", ".join("[" + ", ".join(str(int(v)) for v in row) + "]" for row in a) + "]"

    def arr1d(a):
        return "[" + ", ".join(str(int(v)) for v in a) + "]"

    rs = f"""//! GENERATED by tools/train_motion_nn.py - do not edit by hand.
//! A {FEAT}->{HIDDEN}->2 int8 MLP motion classifier (idle/active), trained on synthetic
//! IMU |accel| windows. Integer-only inference; see NnMotionClassifier.
pub const WINDOW: usize = {WINDOW};
pub const HIDDEN: usize = {HIDDEN};
pub const FEAT: usize = {FEAT};
pub const SHIFT1: u32 = {SHIFT1};
/// Per-feature normalization maxima (feature -> int8 via f * 127 / FEAT_MAX[i]).
pub const FEAT_MAX: [i32; FEAT] = {arr1d(feat_max)};
/// Layer 1 weights [HIDDEN][FEAT] and bias [HIDDEN] (int8 / i32).
pub const W1: [[i8; FEAT]; HIDDEN] = {arr2d(W1q.T)};
pub const B1: [i32; HIDDEN] = {arr1d(b1q)};
/// Layer 2 weights [2][HIDDEN] and bias [2].
pub const W2: [[i8; HIDDEN]; 2] = {arr2d(W2q.T)};
pub const B2: [i32; 2] = {arr1d(b2q)};
/// Training accuracy of the exported integer model (x1000).
pub const TRAIN_ACC_MILLI: u32 = {int(acc_int * 1000)};
"""
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w") as f:
        f.write(rs)
    print(f"wrote {os.path.relpath(OUT)}")

    if not args.no_model_card:
        path = write_model_card(args.models_json, int(acc_int * 1000))
        print(f"wrote {os.path.relpath(path)} (block-editor ML block: {PRESET})")


if __name__ == "__main__":
    main()
