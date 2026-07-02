#!/usr/bin/env python3
"""Battery gauge from the INA3221 power monitor (M160).

Maps a measured bus voltage through a 1S Li-ion open-circuit-voltage curve to a
state-of-charge estimate, plus charge/discharge direction from the shunt current.
Runs a curve self-test (known OCV -> SoC vectors), then reads live samples from the
third-party INA3221 JSONL bridge (START/STOP protocol) and prints a gauge per channel.
A USB-powered rail (>4.25 V) is reported as EXTERNAL rather than a fake percentage.

Usage: python tools/battery_gauge.py [--port COM20] [--seconds 3] [--selftest-only]
"""
import argparse
import json
import sys
import time

# 1S Li-ion OCV -> SoC anchor points (rested cell, room temperature).
OCV_CURVE = [
    (3000, 0), (3300, 5), (3500, 10), (3600, 20), (3700, 40),
    (3800, 60), (3900, 75), (4000, 88), (4100, 96), (4200, 100),
]


def soc_from_mv(mv):
    """Piecewise-linear SoC (percent) from bus millivolts; clamps at the curve ends."""
    if mv <= OCV_CURVE[0][0]:
        return 0
    if mv >= OCV_CURVE[-1][0]:
        return 100
    for (v0, s0), (v1, s1) in zip(OCV_CURVE, OCV_CURVE[1:]):
        if v0 <= mv <= v1:
            return round(s0 + (s1 - s0) * (mv - v0) / (v1 - v0))
    return 0


def classify(mv, current_ua):
    if mv < 500:
        return "ABSENT"
    if mv > 4250:
        return "EXTERNAL"  # USB/bench rail, not a 1S cell
    if current_ua > 5000:
        return "CHARGING"
    if current_ua < -5000:
        return "DISCHARGING"
    return "RESTING"


def selftest():
    vectors = [(3000, 0), (3650, 30), (3700, 40), (3850, 68), (4200, 100),
               (2500, 0), (4700, 100)]
    bad = 0
    for mv, want in vectors:
        got = soc_from_mv(mv)
        ok = abs(got - want) <= 2
        print(f"  ocv {mv} mV -> {got}% (expect ~{want}%) {'OK' if ok else 'FAIL'}")
        bad += 0 if ok else 1
    return bad == 0


def read_live(port, seconds):
    import serial
    sp = serial.Serial(port, 115200, timeout=1)
    time.sleep(0.4)
    sp.write(b"START\n")
    last = None
    t0 = time.time()
    while time.time() - t0 < seconds:
        ln = sp.readline().decode(errors="ignore").strip()
        if ln.startswith("{"):
            try:
                j = json.loads(ln)
                if "channels" in j:
                    last = j
            except ValueError:
                pass
    sp.write(b"STOP\n")
    sp.close()
    return last


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", default="COM20")
    ap.add_argument("--seconds", type=float, default=3.0)
    ap.add_argument("--selftest-only", action="store_true")
    args = ap.parse_args()

    print("== curve self-test ==")
    ok = selftest()
    if args.selftest_only:
        print(f"RESULT: {'PASS' if ok else 'FAIL'}")
        return 0 if ok else 1

    print(f"== live gauge ({args.port}) ==")
    sample = read_live(args.port, args.seconds)
    if not sample:
        print("no sample from the INA3221 bridge")
        print("RESULT: FAIL")
        return 1
    for i, ch in enumerate(sample.get("channels", [])):
        mv = int(1000 * float(ch.get("bus_V", 0.0)))
        ua = int(1_000_000 * float(ch.get("current_A", 0.0)))
        state = classify(mv, ua)
        soc = soc_from_mv(mv)
        gauge = f"{soc:3d}%" if state not in ("ABSENT", "EXTERNAL") else "  - "
        print(f"  ch{i}: {mv} mV {ua / 1000.0:8.1f} mA  {state:11} soc={gauge}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
