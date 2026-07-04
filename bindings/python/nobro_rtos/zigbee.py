"""IEEE 802.15.4 MAC frame contract for the NobroRTOS Zigbee gateway (M122).

A NobroRTOS node with a CC2530 co-processor (the cc2530_gateway app) captures raw
802.15.4 PSDUs off the air. This module is the host-side contract that decodes those
PSDUs into structured records the collector ingests: frame type, addressing, PAN/short
addresses, and a security flag - the same fields a Zigbee sniffer surfaces, but as a
stable dict schema keyed into the NobroRTOS host tooling.

Stdlib only; the decode is a pure function so it is unit-testable against known frame
vectors with no hardware.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import IntEnum


class FrameType(IntEnum):
    BEACON = 0
    DATA = 1
    ACK = 2
    MAC_COMMAND = 3


class AddrMode(IntEnum):
    NONE = 0
    RESERVED = 1
    SHORT = 2  # 16-bit
    EXTENDED = 3  # 64-bit


# MAC command frame identifiers (subset the gateway cares about).
MAC_COMMANDS = {
    0x01: "association_request",
    0x02: "association_response",
    0x04: "data_request",
    0x07: "beacon_request",
}


@dataclass
class MacFrame:
    """One decoded 802.15.4 MAC frame - the gateway->collector contract record."""

    frame_type: FrameType
    security: bool
    frame_pending: bool
    ack_request: bool
    pan_id_compression: bool
    seq: int | None
    dest_pan: int | None = None
    dest_addr: int | None = None
    src_pan: int | None = None
    src_addr: int | None = None
    command: str | None = None
    payload_len: int = 0
    raw_len: int = 0

    def to_record(self) -> dict:
        """Stable collector schema (small, JSON-friendly, hex for addresses)."""
        def hx(v, width):
            return None if v is None else f"0x{v:0{width}X}"

        return {
            "proto": "802.15.4",
            "type": self.frame_type.name.lower(),
            "seq": self.seq,
            "secured": self.security,
            "ack_req": self.ack_request,
            "dest_pan": hx(self.dest_pan, 4),
            "dest": hx(self.dest_addr, 4 if (self.dest_addr or 0) <= 0xFFFF else 16),
            "src": hx(self.src_addr, 4 if (self.src_addr or 0) <= 0xFFFF else 16),
            "command": self.command,
            "payload_len": self.payload_len,
        }


class ZigbeeDecodeError(ValueError):
    pass


def _read(psdu: bytes, off: int, n: int) -> tuple[int, int]:
    if off + n > len(psdu):
        raise ZigbeeDecodeError(f"frame truncated at offset {off} (+{n})")
    return int.from_bytes(psdu[off : off + n], "little"), off + n


def decode_mac_frame(psdu: bytes, has_fcs: bool = False) -> MacFrame:
    """Decode a raw 802.15.4 PSDU (as the CC2530 delivers it, FCS optional).

    Follows the 2006 MAC header layout: FCF (2) + seq (1) + addressing fields whose
    presence is driven by the FCF address modes and PAN-ID-compression bit.
    """
    if has_fcs and len(psdu) >= 2:
        psdu = psdu[:-2]
    fcf, off = _read(psdu, 0, 2)
    ftype = FrameType(fcf & 0x7)
    security = bool(fcf & (1 << 3))
    frame_pending = bool(fcf & (1 << 4))
    ack_request = bool(fcf & (1 << 5))
    pan_comp = bool(fcf & (1 << 6))
    dest_mode = AddrMode((fcf >> 10) & 0x3)
    src_mode = AddrMode((fcf >> 14) & 0x3)

    frame = MacFrame(
        frame_type=ftype,
        security=security,
        frame_pending=frame_pending,
        ack_request=ack_request,
        pan_id_compression=pan_comp,
        seq=None,
        raw_len=len(psdu),
    )

    # ACK frames carry only FCF + sequence number.
    seq, off = _read(psdu, off, 1)
    frame.seq = seq
    if ftype == FrameType.ACK:
        return frame

    addr_bytes = {AddrMode.SHORT: 2, AddrMode.EXTENDED: 8}
    if dest_mode in addr_bytes:
        frame.dest_pan, off = _read(psdu, off, 2)
        frame.dest_addr, off = _read(psdu, off, addr_bytes[dest_mode])
    if src_mode in addr_bytes:
        if not (pan_comp and dest_mode in addr_bytes):
            frame.src_pan, off = _read(psdu, off, 2)
        else:
            frame.src_pan = frame.dest_pan  # compressed: reuse dest PAN
        frame.src_addr, off = _read(psdu, off, addr_bytes[src_mode])

    if ftype == FrameType.MAC_COMMAND and off < len(psdu):
        cmd_id, off = _read(psdu, off, 1)
        frame.command = MAC_COMMANDS.get(cmd_id, f"cmd_0x{cmd_id:02X}")

    frame.payload_len = max(0, len(psdu) - off)
    return frame


@dataclass
class GatewayRollup:
    """Aggregate the gateway's captured frames into per-type counts for the collector."""

    counts: dict[str, int] = field(default_factory=dict)
    total: int = 0

    def ingest(self, frame: MacFrame) -> None:
        key = frame.frame_type.name.lower()
        self.counts[key] = self.counts.get(key, 0) + 1
        self.total += 1

    def to_record(self) -> dict:
        return {"proto": "802.15.4", "frames": self.total, "by_type": dict(self.counts)}
