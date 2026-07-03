#!/usr/bin/env python3
"""Mesh gateway: bridge NobroRTOS collector telemetry to MQTT / HTTP (M128).

Reads a collector snapshot (JSON) and renders each node's telemetry as MQTT publish
messages (topic + payload) and an HTTP POST body, so a NobroRTOS mesh feeds a broker or a
cloud endpoint. It formats and (optionally) sends; --selftest formats synthetic nodes so
it is verifiable with no broker. Network sending uses only stdlib.

  python3 tools/gateway.py snapshot.json --mqtt-prefix nobro --broker <host>
  python3 tools/gateway.py snapshot.json --http http://<host>/ingest
  python3 tools/gateway.py --selftest
"""
import argparse
import json
import sys
import urllib.request


def to_mqtt(prefix, snapshot):
    msgs = []
    for name, b in snapshot.get("boards", {}).items():
        base = f"{prefix}/{name}"
        for key in ("all_pass", "power_W", "arch"):
            if key in b:
                msgs.append((f"{base}/{key}", str(b[key])))
        # a compact retained status message per node
        msgs.append((f"{base}/status", json.dumps({
            "arch": b.get("arch", "unknown"),
            "ok": bool(b.get("all_pass", b.get("pass", True))),
        })))
    net = snapshot.get("network_rollup", snapshot.get("network", {}))
    if net:
        msgs.append((f"{prefix}/$rollup", json.dumps({
            "nodes": net.get("nodes"),
            "total_power_W": net.get("total_power_W"),
        })))
    return msgs


def to_http_body(snapshot):
    return json.dumps({
        "source": "nobrortos-mesh",
        "nodes": [
            {"node": n, "ok": bool(b.get("all_pass", b.get("pass", True))),
             "arch": b.get("arch", "unknown")}
            for n, b in snapshot.get("boards", {}).items()
        ],
    })


def http_post(url, body):
    req = urllib.request.Request(url, data=body.encode(), method="POST",
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status


def selftest():
    snap = {
        "boards": {
            "imu_board": {"arch": "cortex-m4f", "all_pass": 1, "power_W": 0.42},
            "power_node": {"arch": "riscv32imc", "all_pass": 1, "power_W": 1.06},
            "pico2": {"arch": "cortex-m33", "all_pass": 0},
        },
        "network_rollup": {"nodes": 3, "total_power_W": 1.48},
    }
    mqtt = to_mqtt("nobro", snap)
    body = to_http_body(snap)
    print("MQTT messages:")
    for t, p in mqtt:
        print(f"  {t} = {p}")
    print("\nHTTP body:")
    print(f"  {body}")
    # invariants: a status per node + a rollup; the failing node shows ok=false
    status_msgs = [m for m in mqtt if m[0].endswith("/status")]
    rollup = [m for m in mqtt if m[0].endswith("$rollup")]
    pico_ok = json.loads(next(p for t, p in mqtt if t == "nobro/pico2/status"))["ok"]
    body_ok = len(json.loads(body)["nodes"]) == 3
    ok = len(status_msgs) == 3 and len(rollup) == 1 and pico_ok is False and body_ok
    print(f"\nRESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("snapshot", nargs="?")
    ap.add_argument("--mqtt-prefix", default="nobro")
    ap.add_argument("--broker", help="MQTT broker host (requires paho-mqtt if sending)")
    ap.add_argument("--http", help="HTTP endpoint to POST the rollup to")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not args.snapshot:
        ap.print_help()
        return 0
    snap = json.load(open(args.snapshot))
    mqtt = to_mqtt(args.mqtt_prefix, snap)
    for t, p in mqtt:
        print(f"MQTT {t} = {p}")
    if args.http:
        try:
            print(f"HTTP POST {args.http} -> {http_post(args.http, to_http_body(snap))}")
        except Exception as e:  # noqa: BLE001
            print(f"HTTP POST failed: {e}")
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
