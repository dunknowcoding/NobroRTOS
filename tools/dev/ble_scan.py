#!/usr/bin/env python3
"""BLE telemetry scanner for NobroRTOS advertising nodes (M123).

A NobroRTOS board running the ble_adv app broadcasts connectionless
ADV_NONCONN_IND frames named "NOBRO" whose manufacturer data (test company id
0xFFFF) carries [beat u32 LE, status u8]. This scanner (requires ``bleak``)
listens on the PC's Bluetooth adapter and PASSes once it sees the beat counter
advance - proof of live over-the-air telemetry with no connection or pairing.

  python3 tools/ble_scan.py --seconds 30
"""
import argparse
import asyncio
import struct
import sys

from bleak import BleakScanner

COMPANY_ID = 0xFFFF  # Bluetooth SIG test/prototyping id used by the ble_adv app


async def scan(seconds: float, name: str) -> int:
    beats = []

    def on_adv(device, adv):
        if adv.local_name != name:
            return
        blob = adv.manufacturer_data.get(COMPANY_ID)
        if not blob or len(blob) < 5:
            return
        beat = struct.unpack_from("<I", blob, 0)[0]
        status = blob[4]
        if not beats or beat != beats[-1]:
            print(f"  {name} rssi={adv.rssi} beat={beat} status={status}")
            beats.append(beat)

    scanner = BleakScanner(on_adv)
    await scanner.start()
    try:
        await asyncio.sleep(seconds)
    finally:
        await scanner.stop()

    ok = len(beats) >= 2 and beats[-1] != beats[0]
    print(f"distinct beats seen: {len(beats)}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=30.0)
    ap.add_argument("--name", default="NOBRO", help="advertised device name to match")
    args = ap.parse_args()
    return asyncio.run(scan(args.seconds, args.name))


if __name__ == "__main__":
    sys.exit(main())
