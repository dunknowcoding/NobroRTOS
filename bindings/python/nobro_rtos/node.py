"""Live NobroRTOS node control over a serial link (the M200 host binding).

Drive and monitor a running NobroRTOS board from Python:

    from nobro_rtos.node import NobroNode

    with NobroNode("<your-serial-port>") as node:
        for report in node.reports(seconds=5):
            print(report.name, report.fields)
        node.request_dfu()   # boards with self-DFU reboot to their bootloader

Two on-wire formats are decoded, matching what NobroRTOS apps emit today:

- status lines:  ``NOBRO-<NAME> key=value key=value ...``
- telemetry:     one JSON object per line (the collector/jsonl_bridge schema)

Opening a real port requires ``pyserial``; parsing and the fake-transport path
are stdlib-only so tools and tests can run without hardware.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from typing import Any, Iterator


@dataclass
class StatusReport:
    """One decoded ``NOBRO-<NAME> k=v ...`` status line."""

    name: str
    fields: dict[str, Any] = field(default_factory=dict)
    raw: str = ""


def _coerce(value: str) -> Any:
    try:
        return int(value)
    except ValueError:
        try:
            return float(value)
        except ValueError:
            return value


def parse_status_line(line: str) -> StatusReport | None:
    """Decode a ``NOBRO-<NAME> key=value ...`` line, or None if it is not one."""
    line = line.strip()
    if not line.startswith("NOBRO-"):
        return None
    head, _, rest = line.partition(" ")
    name = head[len("NOBRO-"):]
    if not name:
        return None
    fields: dict[str, Any] = {}
    for token in rest.split():
        key, sep, value = token.partition("=")
        if sep:
            fields[key] = _coerce(value)
    return StatusReport(name=name, fields=fields, raw=line)


def parse_telemetry_line(line: str) -> dict[str, Any] | None:
    """Decode one JSONL telemetry line, or None if the line is not valid JSON."""
    line = line.strip()
    if not line.startswith("{"):
        return None
    try:
        decoded = json.loads(line)
    except ValueError:
        return None
    return decoded if isinstance(decoded, dict) else None


class NobroNode:
    """A live NobroRTOS node reachable over a serial port.

    ``transport`` may be any object with ``readline() -> bytes`` and
    ``write(bytes)`` (and optionally ``close()``); when omitted, the named
    serial port is opened with pyserial.

    ``dtr`` matters on real hardware: boards behind a USB-UART bridge treat
    DTR-on-open as a reset (leave it False, the default), while native-USB CDC
    boards (e.g. UNO R4) discard their output until the host asserts the
    control lines - pass ``dtr=True`` for those (asserts DTR and RTS).
    """

    def __init__(
        self,
        port: str | None = None,
        baud: int = 115200,
        timeout: float = 1.0,
        transport: Any | None = None,
        dtr: bool = False,
    ) -> None:
        if transport is not None:
            self._io = transport
        elif port is not None:
            try:
                import serial  # pyserial, needed only for real hardware
            except ImportError as exc:  # pragma: no cover
                raise RuntimeError(
                    "pyserial is required to open a serial port: pip install pyserial"
                ) from exc
            handle = serial.Serial()
            handle.port = port
            handle.baudrate = baud
            handle.timeout = timeout
            handle.dtr = dtr
            handle.rts = dtr
            handle.open()
            self._io = handle
        else:
            raise ValueError("provide either a serial port name or a transport")

    # -- monitoring ---------------------------------------------------------

    def lines(self, seconds: float) -> Iterator[str]:
        """Yield raw text lines from the node for up to ``seconds``."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            raw = self._io.readline()
            if not raw:
                continue
            text = raw.decode(errors="ignore").strip() if isinstance(raw, bytes) else str(raw).strip()
            if text:
                yield text

    def reports(self, seconds: float) -> Iterator[StatusReport]:
        """Yield decoded NOBRO status reports for up to ``seconds``."""
        for text in self.lines(seconds):
            report = parse_status_line(text)
            if report is not None:
                yield report

    def telemetry(self, seconds: float) -> Iterator[dict[str, Any]]:
        """Yield decoded JSONL telemetry samples for up to ``seconds``."""
        for text in self.lines(seconds):
            sample = parse_telemetry_line(text)
            if sample is not None:
                yield sample

    def wait_report(self, name: str, seconds: float) -> StatusReport | None:
        """Return the first status report whose name matches, or None on timeout."""
        for report in self.reports(seconds):
            if report.name == name:
                return report
        return None

    # -- control ------------------------------------------------------------

    def send_line(self, text: str) -> None:
        """Send one command line (newline appended) to the node."""
        self._io.write((text + "\n").encode())

    def request_dfu(self) -> None:
        """Ask a self-DFU-capable app to reboot into its bootloader."""
        self.send_line("DFU")

    # -- lifecycle ----------------------------------------------------------

    def close(self) -> None:
        close = getattr(self._io, "close", None)
        if callable(close):
            close()

    def __enter__(self) -> "NobroNode":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()
