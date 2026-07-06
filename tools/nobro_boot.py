#!/usr/bin/env python3
"""Host-side boot entry helpers for NobroRTOS devices.

The default `touch1200` mode mirrors the Arduino-compatible reset convention:
open the serial port at 1200 baud, lower DTR/RTS, close the port, then wait for
the port to re-enumerate. This is useful for boards whose USB bridge or
bootloader consumes the 1200-baud touch before user firmware runs.

The `command` mode sends a firmware-defined CDC command such as `DFU\\n` or
`BOOT!` to devices that expose a NobroRTOS runtime boot command channel.
"""

from __future__ import annotations

import argparse
import sys
import time
from typing import Iterable


def _serial_module():
    try:
        import serial  # type: ignore
        import serial.tools.list_ports  # type: ignore
    except ImportError as exc:
        raise SystemExit(
            "pyserial is required for serial boot control: pip install pyserial"
        ) from exc
    return serial


def _ports(serial_module) -> set[str]:
    return {p.device.upper() for p in serial_module.tools.list_ports.comports()}


def _wait_for_port_state(
    serial_module,
    port: str,
    *,
    present: bool,
    timeout_s: float,
) -> bool:
    deadline = time.monotonic() + timeout_s
    target = port.upper()
    while time.monotonic() < deadline:
        found = target in _ports(serial_module)
        if found == present:
            return True
        time.sleep(0.1)
    return False


def touch1200(args: argparse.Namespace) -> int:
    serial = _serial_module()
    before = _ports(serial)
    if args.port.upper() not in before:
        print(f"warning: {args.port} is not currently listed")

    if args.dry_run:
        print(f"open {args.port} at 1200 baud, drop DTR/RTS, close")
        return 0

    handle = serial.Serial()
    handle.port = args.port
    handle.baudrate = 1200
    handle.timeout = 0
    handle.dtr = False
    handle.rts = False
    handle.open()
    time.sleep(args.hold_s)
    handle.dtr = False
    handle.rts = False
    handle.close()

    disappeared = _wait_for_port_state(
        serial, args.port, present=False, timeout_s=args.disappear_timeout_s
    )
    reappeared = _wait_for_port_state(
        serial, args.port, present=True, timeout_s=args.appear_timeout_s
    )
    print(
        "RESULT: "
        f"{'PASS' if disappeared or reappeared else 'WARN'} "
        f"disappeared={int(disappeared)} reappeared={int(reappeared)}"
    )
    return 0 if disappeared or reappeared else 2


def command(args: argparse.Namespace) -> int:
    serial = _serial_module()
    payload = args.command.encode("utf-8")
    if args.newline and not payload.endswith(b"\n"):
        payload += b"\n"

    if args.dry_run:
        print(f"write {payload!r} to {args.port} at {args.baud} baud")
        return 0

    with serial.Serial(args.port, args.baud, timeout=args.timeout_s) as handle:
        time.sleep(args.settle_s)
        handle.write(payload)
        handle.flush()
    print(f"RESULT: PASS wrote {len(payload)} bytes")
    return 0


def list_ports(args: argparse.Namespace) -> int:
    serial = _serial_module()
    for port in serial.tools.list_ports.comports():
        print(f"{port.device}\t{port.description}\t{port.hwid}")
    return 0


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Enter a NobroRTOS device bootloader from the host side."
    )
    parser.add_argument("--dry-run", action="store_true")
    sub = parser.add_subparsers(dest="command_name", required=True)

    p_touch = sub.add_parser("touch1200", help="Arduino-compatible 1200-baud touch reset")
    p_touch.add_argument("--port", required=True)
    p_touch.add_argument("--hold-s", type=float, default=0.25)
    p_touch.add_argument("--disappear-timeout-s", type=float, default=8.0)
    p_touch.add_argument("--appear-timeout-s", type=float, default=20.0)
    p_touch.set_defaults(func=touch1200)

    p_command = sub.add_parser("command", help="send a firmware boot command over serial")
    p_command.add_argument("--port", required=True)
    p_command.add_argument("--baud", type=int, default=115200)
    p_command.add_argument("--command", required=True)
    p_command.add_argument("--newline", action="store_true")
    p_command.add_argument("--settle-s", type=float, default=0.2)
    p_command.add_argument("--timeout-s", type=float, default=1.0)
    p_command.set_defaults(func=command)

    p_list = sub.add_parser("list", help="list visible serial ports")
    p_list.set_defaults(func=list_ports)

    args = parser.parse_args(list(argv) if argv is not None else None)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
