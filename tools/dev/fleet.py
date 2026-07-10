#!/usr/bin/env python3
"""Fleet management + time-series ingestion for the NobroRTOS bench (M187, M188).

Ingests collector snapshots (JSON, one object per run) into a compact per-node
time-series (M187), then renders a fleet view (M188): each node's architecture, last
firmware/health, uptime-in-snapshots, and a rollup by CPU architecture. With --selftest
it runs on synthetic snapshots so it is verifiable with no hardware attached.

Usage:
  python tools/fleet.py --ingest run1.json run2.json ...   # append to fleet_ts.jsonl
  python tools/fleet.py --view                             # render the fleet
  python tools/fleet.py --selftest
"""
import argparse
import json
import os
import sys
import time

TS_FILE = os.path.join(os.path.dirname(__file__), "..", "_work", "fleet_ts.jsonl")


def ingest(snapshots, ts_path):
    os.makedirs(os.path.dirname(ts_path), exist_ok=True)
    rows = 0
    with open(ts_path, "a") as f:
        for snap in snapshots:
            t = snap.get("t", time.time())
            for name, b in snap.get("boards", {}).items():
                rec = {
                    "t": t,
                    "node": name,
                    "arch": b.get("arch", "unknown"),
                    "pass": bool(b.get("all_pass", b.get("pass", True))),
                    "power_w": b.get("power_W", 0.0),
                }
                f.write(json.dumps(rec) + "\n")
                rows += 1
    return rows


def load_ts(ts_path):
    if not os.path.exists(ts_path):
        return []
    return [json.loads(x) for x in open(ts_path) if x.strip()]


def view(ts_path):
    rows = load_ts(ts_path)
    if not rows:
        print("no time-series yet (ingest some snapshots first)")
        return 1
    nodes = {}
    for r in rows:
        n = nodes.setdefault(r["node"], {"seen": 0, "ok": 0, "arch": r["arch"], "last_t": 0})
        n["seen"] += 1
        n["ok"] += 1 if r["pass"] else 0
        n["arch"] = r["arch"]
        n["last_t"] = max(n["last_t"], r["t"])
    print("=== NobroRTOS Fleet ===")
    print(f"{'node':18} {'arch':26} {'health':8} {'seen':>5}")
    arch_roll = {}
    for name, n in sorted(nodes.items()):
        health = f"{n['ok']}/{n['seen']}"
        flag = "OK" if n["ok"] == n["seen"] else "DEGR"
        print(f"{name:18} {n['arch']:26} {flag:8} {n['seen']:>5}")
        a = arch_roll.setdefault(n["arch"], [0, 0])
        a[0] += 1
        a[1] += 1 if n["ok"] == n["seen"] else 0
    print("\n-- by architecture --")
    for arch, (total, ok) in sorted(arch_roll.items()):
        print(f"  {arch:26} {ok}/{total} nodes healthy")
    return 0


def selftest():
    tmp = os.path.join(os.path.dirname(__file__), "..", "_work", "fleet_selftest.jsonl")
    if os.path.exists(tmp):
        os.remove(tmp)
    snaps = [
        {"t": 1, "boards": {
            "imu_node": {"arch": "cortex-m4f/nrf52840", "all_pass": 1, "power_W": 0.1},
            "pico2": {"arch": "cortex-m33/rp2350", "all_pass": 1},
            "esp32c3": {"arch": "riscv32imc/esp32c3", "all_pass": 1}}},
        {"t": 2, "boards": {
            "imu_node": {"arch": "cortex-m4f/nrf52840", "all_pass": 1, "power_W": 0.1},
            "pico2": {"arch": "cortex-m33/rp2350", "all_pass": 1},
            "esp32c3": {"arch": "riscv32imc/esp32c3", "all_pass": 0}}},  # a degrade
    ]
    n = ingest(snaps, tmp)
    rows = load_ts(tmp)
    ok = n == 6 and len(rows) == 6
    # esp32c3 must read 1/2 healthy
    per = {}
    for r in rows:
        per.setdefault(r["node"], []).append(r["pass"])
    ok = ok and per["esp32c3"] == [True, False] and per["imu_node"] == [True, True]
    print("time-series rows:", n)
    view(tmp)
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    os.remove(tmp)
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ingest", nargs="+")
    ap.add_argument("--view", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if args.ingest:
        snaps = [json.load(open(p)) for p in args.ingest]
        print("ingested rows:", ingest(snaps, TS_FILE))
        return 0
    if args.view:
        return view(TS_FILE)
    ap.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
