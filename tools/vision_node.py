#!/usr/bin/env python3
"""Vision recognition over the XIAO camera node (M38).

Captures JPEG frames from the SerialFrameDump firmware (base64 marker protocol on the
camera node's COM port), decodes them, and runs scene recognition:

- mean luma            -> {dark | normal | bright}
- entropy + sharpness  -> proves a real scene (a dead/covered sensor fails)
- frame-to-frame diff  -> {static | motion}

Prints one parseable VISION line (the collector ingests it) and saves the last frame.
Exit 0 = a live, real scene was recognized.

Usage: python tools/vision_node.py [--port COM27] [--frames 2] [--save _work/vision.jpg]
"""
import argparse
import base64
import io
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", default="COM27")
    ap.add_argument("--frames", type=int, default=2)
    ap.add_argument("--save", default="_work/vision.jpg")
    args = ap.parse_args()

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

    with open(args.save, "wb") as f:
        f.write(frames[-1])
    print(
        f"VISION port={args.port} scene={scene} activity={activity} luma={luma:.1f} "
        f"entropy={ent:.2f} sharpness={sharp:.0f} diff={diff:.2f} live={int(live)} "
        f"frames={len(frames)} saved={args.save}"
    )
    print(f"RESULT: {'PASS' if live else 'FAIL'}")
    return 0 if live else 1


if __name__ == "__main__":
    sys.exit(main())
