"""6LoWPAN / Thread frame contract for NobroRTOS interop with NiusThread (M127).

NiusThread runs an OpenThread stack on the nRF's 802.15.4 radio; Thread traffic is IEEE
802.15.4 MAC frames whose payload is 6LoWPAN (RFC 4944 / 6282: mesh + fragment + IPHC
headers wrapping IPv6). NobroRTOS's CC2530 gateway (M122) captures those frames off the
air, and this host contract classifies the 6LoWPAN stack on top of the 802.15.4 MAC
decode from `nobro_rtos.zigbee` - so a NiusThread node's traffic surfaces as structured
collector records, the same way the NiusZigbee gateway does.

Stdlib only; the decode is a pure function, unit-testable against 6LoWPAN frame vectors.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum

from .zigbee import MacFrame, decode_mac_frame


class LowpanKind(Enum):
    NALP = "not_lowpan"          # 00xxxxxx (not a LoWPAN frame)
    UNCOMPRESSED_IPV6 = "ipv6"   # 0x41 (01000001)
    HC1 = "hc1"                  # 0x42 (legacy)
    BC0 = "bc0"                  # 0x50 broadcast header
    MESH = "mesh"                # 10xxxxxx
    FRAG_FIRST = "frag_first"    # 11000xxx
    FRAG_N = "frag_n"            # 11100xxx
    IPHC = "iphc"                # 011xxxxx (RFC 6282 compressed IPv6)


def classify_dispatch(b: int) -> LowpanKind:
    """Classify a 6LoWPAN dispatch byte (RFC 4944 sec 5.1 + RFC 6282)."""
    if b == 0x41:
        return LowpanKind.UNCOMPRESSED_IPV6
    if b == 0x42:
        return LowpanKind.HC1
    if b == 0x50:
        return LowpanKind.BC0
    if (b & 0xE0) == 0x60:
        return LowpanKind.IPHC          # 011xxxxx
    if (b & 0xC0) == 0x80:
        return LowpanKind.MESH          # 10xxxxxx
    if (b & 0xF8) == 0xC0:
        return LowpanKind.FRAG_FIRST    # 11000xxx
    if (b & 0xF8) == 0xE0:
        return LowpanKind.FRAG_N        # 11100xxx
    return LowpanKind.NALP


# Thread MLE (Mesh Link Establishment) rides on UDP port 19788.
THREAD_MLE_PORT = 19788


@dataclass
class LowpanHeaders:
    """The decoded 6LoWPAN header stack sitting above the 802.15.4 MAC."""

    kinds: list[LowpanKind] = field(default_factory=list)
    mesh_hops_left: int | None = None
    frag_datagram_size: int | None = None
    frag_offset: int | None = None
    is_thread_lowpan: bool = False

    def to_record(self) -> dict:
        return {
            "l2": "802.15.4",
            "l3": "6lowpan",
            "headers": [k.value for k in self.kinds],
            "mesh_hops_left": self.mesh_hops_left,
            "frag_size": self.frag_datagram_size,
            "thread": self.is_thread_lowpan,
        }


def decode_lowpan(payload: bytes) -> LowpanHeaders:
    """Walk the 6LoWPAN header chain (mesh -> fragment -> IPHC/IPv6) of a MAC payload.

    Returns the header kinds seen and key fields; `is_thread_lowpan` is set when the
    payload carries a real 6LoWPAN IP frame (IPHC or uncompressed IPv6), i.e. Thread
    network traffic rather than raw MAC.
    """
    hdr = LowpanHeaders()
    off = 0
    n = len(payload)
    saw_ip = False
    while off < n:
        kind = classify_dispatch(payload[off])
        hdr.kinds.append(kind)
        if kind == LowpanKind.MESH:
            # 10 V F HopsLeft(4); if hops==0xF a byte of deep hops follows
            b = payload[off]
            hops = b & 0x0F
            off += 1
            if hops == 0x0F and off < n:
                hdr.mesh_hops_left = payload[off]
                off += 1
            else:
                hdr.mesh_hops_left = hops
            # originator + final address: V/F pick short(2) vs extended(8)
            v_short = bool(b & 0x20)
            f_short = bool(b & 0x10)
            off += (2 if v_short else 8) + (2 if f_short else 8)
            continue
        if kind == LowpanKind.FRAG_FIRST:
            # 11000 + datagram_size(11 bits) + tag(16)
            if off + 4 <= n:
                hdr.frag_datagram_size = ((payload[off] & 0x07) << 8) | payload[off + 1]
                hdr.frag_offset = 0
            off += 4
            continue
        if kind == LowpanKind.FRAG_N:
            if off + 5 <= n:
                hdr.frag_datagram_size = ((payload[off] & 0x07) << 8) | payload[off + 1]
                hdr.frag_offset = payload[off + 4] * 8
            off += 5
            continue
        if kind in (LowpanKind.IPHC, LowpanKind.UNCOMPRESSED_IPV6):
            saw_ip = True
            break
        break  # NALP / HC1 / BC0: stop walking

    hdr.is_thread_lowpan = saw_ip
    return hdr


@dataclass
class ThreadFrame:
    """A captured Thread frame: the 802.15.4 MAC + its 6LoWPAN header stack."""

    mac: MacFrame
    lowpan: LowpanHeaders

    def to_record(self) -> dict:
        rec = self.mac.to_record()
        rec.update(self.lowpan.to_record())
        rec["proto"] = "thread"
        return rec


def decode_thread_frame(psdu: bytes, has_fcs: bool = False) -> ThreadFrame:
    """Decode a captured 802.15.4 PSDU as a Thread frame: MAC header + 6LoWPAN stack."""
    if has_fcs and len(psdu) >= 2:
        psdu = psdu[:-2]
    mac = decode_mac_frame(psdu)
    # locate the MAC payload start by re-decoding lengths (mac.payload_len from the end)
    payload = psdu[len(psdu) - mac.payload_len:] if mac.payload_len else b""
    lowpan = decode_lowpan(payload)
    return ThreadFrame(mac=mac, lowpan=lowpan)


def decode_thread_record(psdu: bytes, has_fcs: bool = False) -> dict | None:
    """Return a collector-ready Thread record, or `None` when the PSDU is not 6LoWPAN."""
    frame = decode_thread_frame(psdu, has_fcs=has_fcs)
    if not frame.lowpan.is_thread_lowpan:
        return None
    return frame.to_record()


@dataclass
class ThreadRollup:
    """Aggregate Thread/6LoWPAN observations from captured 802.15.4 PSDUs."""

    frames: int = 0
    thread_frames: int = 0
    headers: dict[str, int] = field(default_factory=dict)

    def ingest(self, psdu: bytes, has_fcs: bool = False) -> ThreadFrame:
        self.frames += 1
        frame = decode_thread_frame(psdu, has_fcs=has_fcs)
        if frame.lowpan.is_thread_lowpan:
            self.thread_frames += 1
            for kind in frame.lowpan.kinds:
                self.headers[kind.value] = self.headers.get(kind.value, 0) + 1
        return frame

    def to_record(self) -> dict:
        return {
            "proto": "thread",
            "l2": "802.15.4",
            "l3": "6lowpan",
            "frames": self.frames,
            "thread_frames": self.thread_frames,
            "headers": dict(self.headers),
        }
