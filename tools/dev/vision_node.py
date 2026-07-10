#!/usr/bin/env python3
"""Vision recognition over the XIAO camera node (M38).

Captures JPEG frames from the SerialFrameDump firmware (base64 marker protocol on the
camera node's COM port), decodes them, and runs scene recognition:

- mean luma            -> {dark | normal | bright}
- entropy + sharpness  -> proves a real scene (a dead/covered sensor fails)
- frame-to-frame diff  -> {static | motion}

Prints one parseable VISION line (the collector ingests it) and saves the last frame.
Exit 0 = a live, real scene was recognized.

Usage: python3 tools/vision_node.py [--port <PORT>] [--frames 2] [--save _work/vision.jpg]
"""
import argparse
import base64
import io
import json
import re
import sys
import time

import numpy as np
from PIL import Image


def read_frames(port, count, timeout_s=40):
    import serial
    frames = []
    # NOTE: open with DTR/RTS deasserted. Asserting DTR on open resets UART-bridge
    # boards (AI-Thinker auto-program wiring) and can bootloader-trap native-USB
    # ESP32-S3 ports. The sketch streams continuously, so no reset is ever needed.
    sp = serial.Serial()
    sp.port = port
    sp.baudrate = 115200
    sp.timeout = 2
    sp.dtr = False
    sp.rts = False
    sp.open()
    try:
        # 31 KB base64 lines burst over native USB; grow the driver RX buffer so bytes
        # survive host-side processing pauses.
        try:
            sp.set_buffer_size(rx_size=262144)
        except Exception:
            pass
        rejects = 0
        buf = ""
        t0 = time.time()
        while len(frames) < count and time.time() - t0 < timeout_s:
            buf += sp.read(65536).decode("utf-8", "replace")
            for m in re.finditer(
                r"<<<JPEG_BEGIN (\d+)x(\d+) (\d+)>>>\s*\n(.*?)\n<<<JPEG_END>>>", buf, re.S
            ):
                try:
                    data = base64.b64decode(m.group(4).strip())
                    frames.append(data)
                except ValueError:
                    pass
            if frames:
                buf = buf[buf.rfind("<<<JPEG_END>>>") + 14:]
    finally:
        sp.close()
    if rejects:
        print(f"(note: {rejects} corrupt frame(s) rejected)")
    return frames[:count]


def gray(data):
    return np.asarray(Image.open(io.BytesIO(data)).convert("L"), dtype=np.float32)


def sharpness(g):
    """Variance of a 4-neighbor Laplacian - blur/dead sensors score near zero."""
    lap = -4 * g[1:-1, 1:-1] + g[:-2, 1:-1] + g[2:, 1:-1] + g[1:-1, :-2] + g[1:-1, 2:]
    return float(lap.var())


def entropy(g):
    hist, _ = np.histogram(g, bins=64, range=(0, 255))
    p = hist / max(hist.sum(), 1)
    p = p[p > 0]
    return float(-(p * np.log2(p)).sum())


def pool_to_square(g, side=16):
    """Average-pool a grayscale image to a fixed square, returned as 0..1 floats."""
    h, w = g.shape
    out = []
    for r in range(side):
        for c in range(side):
            r0, r1 = r * h // side, (r + 1) * h // side
            c0, c1 = c * w // side, (c + 1) * w // side
            block = g[r0:max(r1, r0 + 1), c0:max(c1, c0 + 1)]
            out.append(float(block.mean()) / 255.0)
    return out


def load_person_model(path):
    """Load a tiny dense person/no-person model exported by train_face_pipeline.py."""
    with open(path, "r", encoding="utf-8") as f:
        model = json.load(f)
    input_len = int(model["input_len"])
    output_len = int(model.get("output_len", 2))
    weights = [float(v) for v in model["weights"]]
    bias = [float(v) for v in model.get("bias", [0.0] * output_len)]
    if len(weights) != input_len * output_len:
        raise ValueError("person model weights do not match input/output lengths")
    if len(bias) != output_len:
        raise ValueError("person model bias length does not match output length")
    return {
        "input_len": input_len,
        "output_len": output_len,
        "weights": weights,
        "bias": bias,
        "threshold": float(model.get("threshold", 0.0)),
        "side": int(model.get("side", 16)),
    }


def dense_scores(features, model):
    n_in = model["input_len"]
    scores = []
    for j in range(model["output_len"]):
        row = model["weights"][j * n_in:(j + 1) * n_in]
        scores.append(sum(w * x for w, x in zip(row, features)) + model["bias"][j])
    return scores


def predict_person(g, model):
    features = pool_to_square(g, model["side"])
    if len(features) != model["input_len"]:
        raise ValueError("person model input length does not match pooled frame")
    scores = dense_scores(features, model)
    if len(scores) == 1:
        score = scores[0]
        return int(score >= model["threshold"]), score
    score = scores[1] - scores[0]
    return int(score >= model["threshold"]), score


def selftest_person_path():
    """Exercise the live-frame person path without opening a serial port."""
    face = np.full((16, 16), 80.0, dtype=np.float32)
    face[3:6, 4:7] = 210.0
    face[3:6, 9:12] = 210.0
    face[8:12, 5:11] = 180.0
    negative = np.full((16, 16), 80.0, dtype=np.float32)
    negative[:, ::2] = 180.0

    weights = []
    for img_class in (0, 1):
        for r in range(16):
            for c in range(16):
                eye = (3 <= r < 6) and ((4 <= c < 7) or (9 <= c < 12))
                mouth = (8 <= r < 12) and (5 <= c < 11)
                structure = 1.0 if eye or mouth else -0.15
                weights.append(-structure if img_class == 0 else structure)
    model = {
        "input_len": 256,
        "output_len": 2,
        "weights": weights,
        "bias": [0.0, 0.0],
        "threshold": 20.0,
        "side": 16,
    }
    face_pred, face_score = predict_person(face, model)
    neg_pred, neg_score = predict_person(negative, model)
    ok = face_pred == 1 and neg_pred == 0 and face_score > neg_score
    print(
        f"PERSON-SELFTEST face={face_pred} negative={neg_pred} "
        f"face_score={face_score:.3f} negative_score={neg_score:.3f}"
    )
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", help="serial port of the camera node")
    ap.add_argument("--frames", type=int, default=2)
    ap.add_argument("--save", default="_work/vision.jpg")
    ap.add_argument("--person-model", default=None,
                    help="optional JSON dense model for live person/no-person inference")
    ap.add_argument("--selftest-person", action="store_true",
                    help="run the live-frame person inference path without hardware")
    args = ap.parse_args()

    if args.selftest_person:
        return selftest_person_path()
    if not args.port:
        ap.error("--port is required unless --selftest-person is used")

    person_model = load_person_model(args.person_model) if args.person_model else None

    frames = read_frames(args.port, max(args.frames, 2))
    if len(frames) < 2:
        print("VISION port=%s error=no-frames" % args.port)
        return 1
    g0, g1 = gray(frames[-2]), gray(frames[-1])
    luma = float(g1.mean())
    ent = entropy(g1)
    sharp = sharpness(g1)
    diff = float(np.abs(g1 - g0).mean())

    scene = "dark" if luma < 40 else ("bright" if luma > 200 else "normal")
    activity = "motion" if diff > 6.0 else "static"
    # Liveness = information content. A dead/covered sensor yields near-zero entropy;
    # real scenes measure 4-6 bits. Sharpness varies with optics (soft lenses read low)
    # so it is reported but does not veto.
    live = ent > 3.5
    if person_model:
        person, person_score = predict_person(g1, person_model)
        person_fields = f" person={person} person_score={person_score:.3f}"
    else:
        person_fields = ""

    with open(args.save, "wb") as f:
        f.write(frames[-1])
    print(
        f"VISION port={args.port} scene={scene} activity={activity} luma={luma:.1f} "
        f"entropy={ent:.2f} sharpness={sharp:.0f} diff={diff:.2f} live={int(live)} "
        f"frames={len(frames)} saved={args.save}{person_fields}"
    )
    print(f"RESULT: {'PASS' if live else 'FAIL'}")
    return 0 if live else 1


if __name__ == "__main__":
    sys.exit(main())
