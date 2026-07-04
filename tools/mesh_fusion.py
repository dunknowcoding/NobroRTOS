#!/usr/bin/env python3
"""Cross-protocol mesh fusion (M126): one rollup from BLE + WiFi NobroRTOS nodes.

Nodes reach the collector over whatever radio they have - a BLE-advertising nRF node
(ble_adv app, manufacturer-data telemetry) and WiFi TCP nodes (Esp32/UnoR4 telemetry
sketches) - and this gateway fuses them into a single mesh snapshot keyed by node id,
protocol-tagged. PASS requires live samples from BOTH protocols in one run: proof that
the mesh is heterogeneous, not one radio pretending twice.

  python3 tools/mesh_fusion.py --seconds 30 [--tcp-port 9099]
"""
import argparse
import asyncio
import json
import socket
import struct
import sys
import threading
import time

COMPANY_ID = 0xFFFF  # matches nobro_iot::BleAdvBuilder in the ble_adv app


def tcp_ingest(port: float, seconds: float, nodes: dict, lock: threading.Lock):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", int(port)))
    srv.listen(2)
    srv.settimeout(seconds)
    deadline = time.time() + seconds
    try:
        while time.time() < deadline:
            try:
                conn, addr = srv.accept()
            except socket.timeout:
                break
            conn.settimeout(max(1.0, deadline - time.time()))
            buf = b""
            try:
                while time.time() < deadline:
                    chunk = conn.recv(1024)
                    if not chunk:
                        break
                    buf += chunk
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        try:
                            j = json.loads(line.decode(errors="ignore"))
                        except ValueError:
                            continue
                        with lock:
                            nodes[f"wifi:{j.get('chip', addr[0])}"] = {
                                "protocol": "wifi-tcp",
                                "samples": nodes.get(
                                    f"wifi:{j.get('chip', addr[0])}", {}
                                ).get("samples", 0) + 1,
                                "last": j,
                            }
            except socket.timeout:
                pass
            finally:
                conn.close()
    finally:
        srv.close()


async def ble_ingest(seconds: float, nodes: dict, lock: threading.Lock):
    from bleak import BleakScanner

    def on_adv(_device, adv):
        if adv.local_name != "NOBRO":
            return
        blob = adv.manufacturer_data.get(COMPANY_ID)
        if not blob or len(blob) < 5:
            return
        beat = struct.unpack_from("<I", blob, 0)[0]
        with lock:
            entry = nodes.get("ble:NOBRO", {"protocol": "ble-adv", "samples": 0})
            entry["samples"] += 1
            entry["last"] = {"beat": beat, "status": blob[4], "rssi": adv.rssi}
            nodes["ble:NOBRO"] = entry

    scanner = BleakScanner(on_adv)
    await scanner.start()
    try:
        await asyncio.sleep(seconds)
    finally:
        await scanner.stop()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=30.0)
    ap.add_argument("--tcp-port", type=int, default=9099)
    args = ap.parse_args()

    nodes: dict = {}
    lock = threading.Lock()
    tcp = threading.Thread(
        target=tcp_ingest, args=(args.tcp_port, args.seconds, nodes, lock), daemon=True
    )
    tcp.start()
    asyncio.run(ble_ingest(args.seconds, nodes, lock))
    tcp.join(timeout=5)

    protocols = set()
    print("mesh snapshot:")
    for name, info in sorted(nodes.items()):
        protocols.add(info["protocol"])
        print(f"  {name:24s} {info['protocol']:9s} samples={info['samples']:4d} last={json.dumps(info['last'])[:70]}")
    ok = {"ble-adv", "wifi-tcp"} <= protocols
    print(f"protocols fused: {sorted(protocols)}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
