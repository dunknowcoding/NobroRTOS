#!/usr/bin/env python3
"""Keyword spotting over mono PCM audio.

The tool is intentionally adapter-neutral: it can classify a 16 kHz mono WAV file or
read a serial node that emits base64 PCM16 frames with this marker protocol:

<<<PCM16_BEGIN rate=16000 samples=16000>>>
<base64 little-endian int16 PCM>
<<<PCM16_END>>>

Models are JSON exports from tools/train_kws_pipeline.py. Exit 0 means a keyword
classification was produced, or the software self-test passed.
"""

import argparse
import base64
import io
import json
import re
import sys
import time
import wave

import numpy as np

FRAMES = 15
BANDS = 8
N_IN = FRAMES * BANDS
RATE = 16000


def pcm_features(raw):
    x = np.zeros(RATE, dtype=np.float32)
    x[: min(len(raw), RATE)] = raw[:RATE] / 32768.0
    feats = []
    hop = RATE // FRAMES
    for t in range(FRAMES):
        seg = x[t * hop:t * hop + 1024]
        if len(seg) < 1024:
            seg = np.pad(seg, (0, 1024 - len(seg)))
        spec = np.abs(np.fft.rfft(seg * np.hanning(1024))) ** 2
        edges = np.logspace(np.log10(4), np.log10(len(spec) - 1), BANDS + 1).astype(int)
        for b in range(BANDS):
            e = spec[edges[b]:max(edges[b + 1], edges[b] + 1)].mean()
            feats.append(float(np.log10(e + 1e-9)))
    f = np.array(feats)
    f = (f - f.mean()) / (f.std() + 1e-6)
    return [float(v) for v in f]


def read_wav(path):
    with wave.open(path, "rb") as f:
        if f.getframerate() != RATE or f.getnchannels() != 1:
            raise ValueError("expected 16 kHz mono WAV")
        return np.frombuffer(f.readframes(f.getnframes()), dtype=np.int16)


def read_serial_pcm(port, timeout_s):
    import serial

    sp = serial.Serial()
    sp.port = port
    sp.baudrate = 115200
    sp.timeout = 1
    sp.dtr = False
    sp.rts = False
    sp.open()
    try:
        buf = ""
        t0 = time.time()
        while time.time() - t0 < timeout_s:
            buf += sp.read(65536).decode("utf-8", "replace")
            m = re.search(
                r"<<<PCM16_BEGIN rate=(\d+) samples=(\d+)>>>\s*\n(.*?)\n<<<PCM16_END>>>",
                buf,
                re.S,
            )
            if not m:
                continue
            rate = int(m.group(1))
            if rate != RATE:
                raise ValueError(f"expected {RATE} Hz PCM, got {rate} Hz")
            data = base64.b64decode(m.group(3).strip())
            return np.frombuffer(data, dtype="<i2")
    finally:
        sp.close()
    raise TimeoutError("no PCM16 frame received")


def load_model(path):
    with open(path, "r", encoding="utf-8") as f:
        model = json.load(f)
    input_len = int(model["input_len"])
    output_len = int(model.get("output_len", 2))
    weights = [float(v) for v in model["weights"]]
    bias = [float(v) for v in model.get("bias", [0.0] * output_len)]
    if input_len != N_IN:
        raise ValueError(f"expected {N_IN} input features, got {input_len}")
    if len(weights) != input_len * output_len:
        raise ValueError("model weights do not match input/output lengths")
    if len(bias) != output_len:
        raise ValueError("model bias length does not match output length")
    return {
        "name": model.get("name", "kws"),
        "labels": list(model.get("labels", ["no", "yes"])),
        "input_len": input_len,
        "output_len": output_len,
        "weights": weights,
        "bias": bias,
        "threshold": float(model.get("threshold", 0.0)),
    }


def predict(features, model):
    scores = []
    for j in range(model["output_len"]):
        row = model["weights"][j * model["input_len"]:(j + 1) * model["input_len"]]
        scores.append(sum(w * x for w, x in zip(row, features)) + model["bias"][j])
    if len(scores) == 1:
        pred = int(scores[0] >= model["threshold"])
        score = scores[0]
    else:
        pred = max(range(len(scores)), key=lambda i: scores[i])
        score = scores[pred] - min(scores)
    label = model["labels"][pred] if pred < len(model["labels"]) else str(pred)
    return label, pred, score, scores


def synth_tone(freq_hz):
    t = np.arange(RATE, dtype=np.float32)
    return (12000.0 * np.sin(2 * np.pi * freq_hz * t / RATE)).astype(np.int16)


def selftest():
    low = pcm_features(synth_tone(500.0))
    high = pcm_features(synth_tone(3000.0))
    weights = []
    for cls in (0, 1):
        for frame in range(FRAMES):
            for band in range(BANDS):
                low_band = 3
                high_band = 6
                if cls == 0:
                    weights.append(1.0 if band == low_band else (-1.0 if band == high_band else 0.0))
                else:
                    weights.append(1.0 if band == high_band else (-1.0 if band == low_band else 0.0))
    model = {
        "name": "kws-selftest",
        "labels": ["low", "high"],
        "input_len": N_IN,
        "output_len": 2,
        "weights": weights,
        "bias": [0.0, 0.0],
        "threshold": 0.0,
    }
    low_label, _, low_score, _ = predict(low, model)
    high_label, _, high_score, _ = predict(high, model)
    ok = low_label == "low" and high_label == "high"
    print(
        f"AUDIO-KWS-SELFTEST low={low_label} high={high_label} "
        f"low_score={low_score:.3f} high_score={high_score:.3f}"
    )
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--wav", help="16 kHz mono WAV file")
    src.add_argument("--serial", help="serial port emitting PCM16 marker frames")
    ap.add_argument("--model", help="JSON model exported by train_kws_pipeline.py")
    ap.add_argument("--timeout", type=float, default=30.0)
    ap.add_argument("--expect", default=None, help="optional expected label gate")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.model:
        ap.error("--model is required unless --selftest is used")
    if not args.wav and not args.serial:
        ap.error("one of --wav or --serial is required unless --selftest is used")

    raw = read_wav(args.wav) if args.wav else read_serial_pcm(args.serial, args.timeout)
    model = load_model(args.model)
    label, pred, score, scores = predict(pcm_features(raw), model)
    ok = args.expect is None or label == args.expect
    source = args.wav if args.wav else args.serial
    print(
        f"AUDIO-KWS source={source} model={model['name']} label={label} class={pred} "
        f"score={score:.3f} features={N_IN} samples={len(raw)} scores="
        + ",".join(f"{s:.3f}" for s in scores)
    )
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
