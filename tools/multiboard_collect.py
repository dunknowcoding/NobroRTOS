#!/usr/bin/env python3
"""NobroRTOS multi-board data collector (autonomous, no DFU).

Aggregates heterogeneous boards into one snapshot:
  * a NobroRTOS nRF52 board, read over J-Link straight from its NOBRO_* RAM report
    (the board keeps running; we halt-read-resume), and
  * any board speaking the INA "JSONL bridge" protocol over a COM port -- e.g. the
    ESP32-C3 running the third-party `ina3221`/INA_series_sensor app. NobroRTOS did
    not write that firmware; this shows NobroRTOS tooling ingesting an external
    app's protocol and collecting across architectures at once.

Everything here is non-destructive: J-Link halt/resume + serial read, no flashing,
no reset-to-DFU. Exit code 0 = all configured boards delivered valid data.

Usage:  python tools/multiboard_collect.py [--ina-port COM20] [--samples 3]
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time

JLINK = r"C:\Program Files\SEGGER\JLink_V924a\JLink.exe"
SPI_IMU_MAGIC = 0x4E425350  # "NBSP" - spi_imu_demo report


def read_nrf52_report(addr=0x20000038, words=9, device="nRF52840_xxAA"):
    """Halt-read a NOBRO_* report from a running NobroRTOS board, then resume it."""
    script = (
        f"si SWD\nspeed 4000\nconnect\nhalt\nmem32 0x{addr:08X},{words}\ng\nq\n"
    )
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
    vals = []
    for line in out.splitlines():
        m = re.match(r"^[0-9A-Fa-f]{8} = (.+)$", line.strip())
        if m:
            vals += [int(x, 16) for x in m.group(1).split()]
    return vals[:words]


def read_ina_bridge(port, seconds=3):
    """Drive the INA JSONL bridge (START/STOP) and return the last sample dict."""
    import serial  # pyserial
    sp = serial.Serial()
    sp.port = port
    sp.baudrate = 115200
    sp.timeout = 0.3
    sp.dtr = False  # do NOT toggle DTR/RTS: that auto-resets ESP32 boards
    sp.rts = False
    sp.open()
    time.sleep(0.4)
    sp.write(b"START\n")
    last = None
    t0 = time.time()
    while time.time() - t0 < seconds:
        line = sp.readline().decode(errors="ignore").strip()
        if line.startswith("{"):
            try:
                j = json.loads(line)
                if "channels" in j:
                    last = j
            except json.JSONDecodeError:
                pass
    try:
        sp.write(b"STOP\n")
    except Exception:
        pass
    sp.close()
    return last


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ina-port", default="COM20")
    ap.add_argument("--samples", type=int, default=3)
    args = ap.parse_args()

    print("=== NobroRTOS multi-board collection ===")
    ts = time.strftime("%H:%M:%S")
    snapshot = {"t": ts, "boards": {}}

    # 1) NobroRTOS nRF52 board over J-Link.
    nrf_ok = False
    try:
        w = read_nrf52_report()
        if len(w) >= 9 and w[0] == SPI_IMU_MAGIC:
            imu = {
                "kind": "nRF52840 / NobroRTOS / MPU-9250 (SPI)",
                "all_pass": w[3],
                "who_am_i": f"0x{w[4]:02X}",
                "accel_mg": w[5],
                "reads": w[6],
                "errors": w[7],
            }
            snapshot["boards"]["board1"] = imu
            nrf_ok = w[3] == 1
            print(f"[board1  nRF52/NobroRTOS] accel={w[5]} mg  who=0x{w[4]:02X}  "
                  f"reads={w[6]}  err={w[7]}  all_pass={w[3]}")
        else:
            print(f"[board1] no NobroRTOS report (read {len(w)} words, "
                  f"magic={hex(w[0]) if w else 'none'})")
    except Exception as e:
        print(f"[board1] J-Link read failed: {e}")

    # 2) INA JSONL-bridge board over COM (a third-party app).
    ina_ok = False
    try:
        ina = read_ina_bridge(args.ina_port, args.samples)
        if ina and len(ina.get("channels", [])) == 3:
            ch = ina["channels"]
            snapshot["boards"]["ina_bridge"] = {
                "kind": f"{ina.get('chip')} via JSONL bridge ({args.ina_port})",
                "bus_V": ina["bus_V"],
                "channels": ch,
            }
            ina_ok = True
            print(f"[{args.ina_port} {ina.get('chip')}] bus={ina['bus_V']:.3f} V  " +
                  "  ".join(f"ch{i}: {c['current_A']*1000:6.1f} mA / {c['power_W']:.3f} W"
                            for i, c in enumerate(ch)))
        else:
            print(f"[{args.ina_port}] no INA samples")
    except Exception as e:
        print(f"[{args.ina_port}] read failed: {e}")

    ok = nrf_ok and ina_ok
    print("\n--- unified snapshot ---")
    print(json.dumps(snapshot, indent=2))
    print(f"\nRESULT: nRF52={'OK' if nrf_ok else 'FAIL'}  "
          f"INA={'OK' if ina_ok else 'FAIL'}  => {'PASS' if ok else 'FAIL'}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
