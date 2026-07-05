#!/usr/bin/env python3
"""nRF-radio -> WiFi bridge (M125): forward captured 802.15.4 frames onto the WiFi collector.

An nRF node's CC2530 co-processor receives real 802.15.4 frames off the air (the
cc2530_gateway app, M122). This bridge reads those captured frames from the node's
NOBRO_CC2530_GATEWAY_REPORT (over J-Link) and delivers them to the WiFi collector over
TCP - the same sink the ESP32 WiFi telemetry nodes use - so radio-domain traffic and
WiFi-domain traffic converge in one collector. The nRF radio and the ESP32 WiFi are
bridged: a frame that arrived over 802.15.4 leaves over WiFi.

  python3 tools/radio_wifi_bridge.py --selftest        # decode+forward logic, no hardware
  python3 tools/radio_wifi_bridge.py --collector-host 127.0.0.1 --collector-port 9099 \
      --report-addr 0x20000038 --seconds 40             # live: J-Link -> collector
"""
import argparse
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "bindings" / "python"))
from nobro_rtos.zigbee import decode_mac_frame  # noqa: E402

JLINK = os.environ.get("JLINK_EXE", r"C:\Program Files\SEGGER\JLink\JLink.exe")


def read_report_words(addr: int, count: int, device: str = "nRF52840_xxAA") -> list[int]:
    """Read `count` u32 words at `addr` from a running target via J-Link mem32."""
    script = f"si SWD\nspeed 4000\nconnect\nhalt\nmem32 0x{addr:08X}, {count}\ng\nq\n"
    with tempfile.NamedTemporaryFile("w", suffix=".jlink", delete=False) as f:
        f.write(script)
        path = f.name
    try:
        out = subprocess.run(
            [JLINK, "-device", device, "-if", "SWD", "-speed", "4000",
             "-autoconnect", "1", "-NoGui", "1", "-CommandFile", path],
            capture_output=True, text=True, timeout=30,
        ).stdout
    finally:
        os.unlink(path)
    words: list[int] = []
    for line in out.splitlines():
        line = line.strip()
        if "=" in line and line[:8].isalnum() and line.split("=")[0].strip()[:2] != "":
            parts = line.split("=", 1)[1].split()
            for p in parts:
                try:
                    words.append(int(p, 16))
                except ValueError:
                    pass
    return words


def frame_from_report(words: list[int]) -> dict | None:
    """Decode the captured 802.15.4 frame out of a NOBRO_CC2530_GATEWAY_REPORT (NZGW)."""
    if len(words) < 21 or words[0] != 0x4E5A4757:
        return None
    last_len = words[11]
    if not last_len:
        return None
    frame_bytes = b"".join(struct.pack("<I", w) for w in words[12:20])[:last_len]
    rec = decode_mac_frame(frame_bytes).to_record()
    rec["transport"] = "radio->wifi-bridge"
    rec["radio"] = "802.15.4"
    return rec


def send_to_collector(host: str, port: int, record: dict) -> bool:
    try:
        with socket.create_connection((host, port), timeout=5) as s:
            s.sendall((json.dumps(record) + "\n").encode())
        return True
    except OSError:
        return False


def selftest() -> int:
    # a synthetic report carrying the real beacon-request PSDU captured in M122
    words = [0x4E5A4757, 1, 1, 1, 8, 0xF, 5, 0, 2, 0, 3,
             8, 0xFFEA0803, 0x07FFFFFF, 0, 0, 0, 0, 0, 0, 0]
    rec = frame_from_report(words)
    print("bridged record:", json.dumps(rec))
    ok = rec is not None and rec["transport"] == "radio->wifi-bridge" \
        and rec["command"] == "beacon_request"
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--collector-host", default="127.0.0.1")
    ap.add_argument("--collector-port", type=int, default=9099)
    ap.add_argument("--report-addr", default="0x20000038")
    ap.add_argument("--seconds", type=float, default=40.0)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    addr = int(args.report_addr, 16)
    deadline = time.time() + args.seconds
    seen = set()
    bridged = 0
    while time.time() < deadline:
        words = read_report_words(addr, 21)
        rec = frame_from_report(words)
        if rec:
            key = json.dumps(rec, sort_keys=True)
            if key not in seen:
                seen.add(key)
                if send_to_collector(args.collector_host, args.collector_port, rec):
                    bridged += 1
                    print(f"bridged radio frame -> WiFi collector: {json.dumps(rec)[:100]}")
        time.sleep(2)
    print(f"radio->WiFi frames bridged: {bridged}")
    print(f"RESULT: {'PASS' if bridged >= 1 else 'FAIL (no radio traffic captured this window)'}")
    return 0 if bridged >= 1 else 1


if __name__ == "__main__":
    sys.exit(main())
