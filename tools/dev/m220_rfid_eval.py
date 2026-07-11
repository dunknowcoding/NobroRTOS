#!/usr/bin/env python3
"""M220 hardware gate: RC522 UID read on Arduino UNO R4 WiFi via NiusWireless.

Discovers compatible USB interfaces read-only by default. With `--execute`, builds
tools/dev/bench/UnoR4RfidRc522Verify, uploads over serial/DFU, and watches for:
  M220 RESULT: PASS NiusWireless_RC522_UID

Examples:
  python tools/dev/m220_rfid_eval.py
  python tools/dev/m220_rfid_eval.py --execute --auto
  python tools/dev/m220_rfid_eval.py --execute --port <PORT> --dfu-bootloader
  python tools/dev/m220_rfid_eval.py --compile-only

Requires: arduino-cli, renesas_uno core, NiusWireless library (sibling repo or --library).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SKETCH = os.path.join(REPO, "tools", "dev", "bench", "UnoR4RfidRc522Verify")
BUILD = os.path.join(REPO, "_work", "m220_unor4_rfid_build")
FQBN = "arduino:renesas_uno:unor4wifi"
PASS_RE = re.compile(r"M220 RESULT: PASS NiusWireless_RC522_UID")
FAIL_RE = re.compile(r"M220 RESULT: FAIL (.+)")
# Renesas SAM-BA bootloader vs runtime composite (ESP bridge) on UNO R4 WiFi.
BOOTLOADER_PID = 0x006D
RUNTIME_PID = 0x1002
ARDUINO_VID = 0x2341


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, **kw)


def port_openable(port: str) -> bool:
    try:
        import serial  # type: ignore
        with serial.Serial(port, 115200, timeout=0.2):
            return True
    except Exception:
        return False


def discover_unor4_ports() -> tuple[str | None, str | None]:
    """Return (upload_port, monitor_port) from arduino-cli + pyserial VID/PID."""
    upload_port: str | None = None
    monitor_port: str | None = None
    try:
        r = subprocess.run(
            ["arduino-cli", "board", "list", "--format", "json"],
            capture_output=True, text=True, check=False,
        )
        if r.returncode == 0:
            for entry in json.loads(r.stdout):
                fqbn = entry.get("matching_boards", [{}])[0].get("fqbn", "")
                if fqbn != FQBN:
                    continue
                port = entry.get("port", {}).get("address")
                if port:
                    upload_port = port
    except (json.JSONDecodeError, IndexError, KeyError):
        pass

    try:
        import serial.tools.list_ports as list_ports  # type: ignore
        for info in list_ports.comports():
            if info.vid != ARDUINO_VID:
                continue
            if info.pid == BOOTLOADER_PID:
                upload_port = info.device
            elif info.pid == RUNTIME_PID:
                monitor_port = info.device
                if upload_port is None:
                    upload_port = info.device
    except ImportError:
        pass

    if monitor_port is None:
        monitor_port = upload_port
    return upload_port, monitor_port


def default_niuswireless_lib() -> str | None:
    sibling = os.path.normpath(os.path.join(REPO, "..", "NiusWireless"))
    if os.path.isfile(os.path.join(sibling, "library.properties")):
        return sibling
    return None


def compile_sketch(library: str | None) -> bool:
    os.makedirs(BUILD, exist_ok=True)
    cmd = [
        "arduino-cli", "compile",
        "--fqbn", FQBN,
        "--build-path", BUILD,
        "--export-binaries",
        SKETCH,
    ]
    if library:
        cmd.extend(["--library", library])
    r = run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stdout)
        print(r.stderr, file=sys.stderr)
    return r.returncode == 0


def is_bootloader_port(port: str) -> bool:
    try:
        import serial.tools.list_ports as list_ports  # type: ignore
        for info in list_ports.comports():
            if info.device.upper() == port.upper():
                return info.pid == BOOTLOADER_PID
    except ImportError:
        pass
    return False


def upload(port: str, *, dfu_bootloader: bool, retries: int) -> bool:
    """Upload a precompiled build dir. Skip 1200-bps touch when already in bootloader."""
    if not port_openable(port):
        print(f"Port {port} is not openable (Windows error 31 = replug USB or single-tap RESET)",
              file=sys.stderr)
        return False
    props = ["upload.use_1200bps_touch=false"] if dfu_bootloader else []
    for attempt in range(1, retries + 1):
        cmd = [
            "arduino-cli", "upload",
            "-p", port,
            "--fqbn", FQBN,
            "--input-dir", BUILD,
            SKETCH,
        ]
        for prop in props:
            cmd.extend(["--upload-property", prop])
        if attempt > 1:
            print(f"Upload retry {attempt}/{retries} — tap RESET once if the port is in bootloader",
                  flush=True)
            time.sleep(2.0)
        r = run(cmd, capture_output=True, text=True)
        if r.returncode == 0:
            return True
        print(r.stdout)
        print(r.stderr, file=sys.stderr)
    return False


def wait_for_port(port: str, timeout_s: float = 15.0) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if port_openable(port):
            return True
        time.sleep(0.5)
    return False


def monitor(port: str, timeout_s: float) -> tuple[bool, str]:
    try:
        import serial  # type: ignore
    except ImportError:
        print("pyserial required for monitor: pip install pyserial", file=sys.stderr)
        return False, "missing_pyserial"

    if not wait_for_port(port, min(timeout_s, 20.0)):
        return False, f"port_not_open:{port}"

    deadline = time.time() + timeout_s
    buf = ""
    with serial.Serial(port, 115200, timeout=0.5) as ser:
        ser.reset_input_buffer()
        while time.time() < deadline:
            chunk = ser.read(512)
            if chunk:
                text = chunk.decode("utf-8", errors="replace")
                print(text, end="", flush=True)
                buf += text
                if PASS_RE.search(buf):
                    return True, "pass"
                m = FAIL_RE.search(buf)
                if m and m.group(1) != "rc522_not_found":
                    return False, m.group(1)
            else:
                time.sleep(0.05)
    if FAIL_RE.search(buf):
        return False, FAIL_RE.search(buf).group(1)  # type: ignore[union-attr]
    return False, "timeout"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--auto", action="store_true",
                    help="Detect compatible upload/monitor interfaces by USB VID/PID")
    ap.add_argument("--port", default=os.environ.get("M220_UPLOAD_PORT"),
                    help="Upload port (default: auto-detect by USB identity)")
    ap.add_argument("--monitor-port", default=os.environ.get("M220_MONITOR_PORT"),
                    help="Serial monitor port after reset")
    ap.add_argument("--library", default=os.environ.get("NIUSWIRELESS_LIB"),
                    help="Path to NiusWireless library")
    ap.add_argument("--timeout", type=float, default=45.0,
                    help="Seconds to wait for tag PASS line")
    ap.add_argument("--compile-only", action="store_true")
    ap.add_argument("--execute", action="store_true",
                    help="Authorize compile/upload/monitor; default is read-only discovery")
    ap.add_argument("--skip-upload", action="store_true")
    ap.add_argument("--skip-compile", action="store_true")
    ap.add_argument(
        "--dfu-bootloader",
        action="store_true",
        help="Board is already in its native-USB bootloader; skip 1200-bps touch",
    )
    ap.add_argument("--upload-retries", type=int, default=3)
    args = ap.parse_args()

    if not args.execute and not args.compile_only:
        upload_port, monitor_port = discover_unor4_ports()
        print(f"Read-only discovery: upload={upload_port or '?'} monitor={monitor_port or '?'}")
        print("RESULT: PASS discovery")
        return 0

    library = args.library or default_niuswireless_lib()
    if not library:
        print("warning: NiusWireless library not found; set --library or clone sibling repo",
              file=sys.stderr)

    if not args.skip_compile:
        if not compile_sketch(library):
            print("RESULT: FAIL compile")
            return 1

    if args.compile_only:
        print("RESULT: PASS compile")
        return 0

    upload_port = args.port
    monitor_port = args.monitor_port
    if args.auto or not upload_port:
        detected_up, detected_mon = discover_unor4_ports()
        upload_port = upload_port or detected_up
        if not monitor_port:
            monitor_port = detected_mon
        print(f"Auto-detect: upload={upload_port or '?'} monitor={monitor_port or '?'}",
              flush=True)

    if not upload_port and not args.skip_upload:
        print("RESULT: FAIL no_port (no compatible runtime or bootloader interface found)",
              file=sys.stderr)
        return 1

    if not args.skip_upload:
        dfu = (
            args.dfu_bootloader
            or os.environ.get("M220_DFU_BOOTLOADER") == "1"
            or is_bootloader_port(upload_port)
        )
        if dfu:
            print(f"Bootloader upload on {upload_port} (no 1200-bps touch)", flush=True)
        else:
            print(f"Runtime upload on {upload_port} (1200-bps touch -> bootloader)", flush=True)
        if not upload(upload_port, dfu_bootloader=dfu, retries=args.upload_retries):
            print("RESULT: FAIL upload\n"
                  "  Re-run read-only discovery, then retry with the detected interface; "
                  "use --dfu-bootloader only when discovery identifies the bootloader.",
                  file=sys.stderr)
            return 1
        print("Upload OK — waiting for board reset...", flush=True)
        time.sleep(4.0)

    monitor_port = monitor_port or upload_port
    print(f"Monitoring {monitor_port} for up to {args.timeout:.0f}s (present a tag)...",
          flush=True)
    ok, reason = monitor(monitor_port, args.timeout)
    if ok:
        print("RESULT: PASS M220")
        return 0
    print(f"RESULT: FAIL {reason}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
