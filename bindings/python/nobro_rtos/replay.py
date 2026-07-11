"""Capability replay-trace decoding for NobroRTOS host tooling (time-travel diagnostics).

`nobro_kernel::CapabilityTrace<N>` records every authorized capability use as a fixed
record (seq, at_us, module, capability, op, args, result). Exported to the host, those
records turn "the module did something wrong at some point" into an auditable,
replayable file: order by sequence, filter by module/capability/op (the host mirror of
`CapabilityReplayScope`), and emit JSON for the evidence pack.

Export wire format (little-endian, 28 bytes per record):

    seq u32 | at_us u64 | module u8 | capability u8 | op u8 | pad u8 |
    arg0 u32 | arg1 u32 | result u32

`module` encodes `ModuleId`: 0..=8 are the unit variants in declaration order
(kernel, hal, bus, radio, sensor, actuator, stream, crypto, ai); `0x80 | n` is
`App(n)`. `capability` and `op` are the kernel's explicit discriminants. Stdlib-only.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

# nobro_kernel::CapabilityTraceOp (#[repr(u8)], explicit discriminants)
OP_NAMES = {1: "acquire", 2: "release", 3: "read", 4: "write", 5: "invoke", 6: "fault"}
# nobro_kernel::ModuleId unit variants, declaration order; App(n) = 0x80 | n
MODULE_NAMES = {0: "kernel", 1: "hal", 2: "bus", 3: "radio", 4: "sensor",
                5: "actuator", 6: "stream", 7: "crypto", 8: "ai"}
# nobro_kernel::Capability explicit discriminants
CAPABILITY_NAMES = {0: "timebase", 1: "deadline_timer", 2: "event_capture", 3: "bus0",
                    4: "bus1", 5: "radio", 6: "servo_pwm", 7: "stream", 8: "crypto",
                    9: "sample_pool", 10: "host_report", 11: "ai_inference",
                    12: "ai_endpoint", 13: "mailbox", 14: "alarm", 15: "kv_store"}
APP_FLAG = 0x80

RECORD_FMT = "<IQBBBxIII"
RECORD_SIZE = struct.calcsize(RECORD_FMT)  # 28


def module_name(code: int) -> str:
    if code & APP_FLAG:
        return f"app{code & 0x7F}"
    return MODULE_NAMES.get(code, f"module{code}")


@dataclass
class TraceRecord:
    seq: int
    at_us: int
    module: int
    capability: int
    op: int
    arg0: int
    arg1: int
    result: int

    @property
    def module_name(self) -> str:
        return module_name(self.module)

    @property
    def capability_name(self) -> str:
        return CAPABILITY_NAMES.get(self.capability, f"cap{self.capability}")

    @property
    def op_name(self) -> str:
        return OP_NAMES.get(self.op, f"op{self.op}")

    def to_dict(self) -> dict:
        return {
            "seq": self.seq, "at_us": self.at_us,
            "module": self.module_name, "capability": self.capability_name,
            "op": self.op_name, "arg0": self.arg0, "arg1": self.arg1,
            "result": self.result,
        }


def encode_record(r: TraceRecord) -> bytes:
    """Encode one record in the export wire format (test vectors / simulators)."""
    return struct.pack(RECORD_FMT, r.seq, r.at_us, r.module, r.capability, r.op,
                       r.arg0, r.arg1, r.result)


def decode_trace(blob: bytes) -> list[TraceRecord]:
    """Decode a trace blob (concatenated fixed records); trailing partials ignored."""
    out = []
    for off in range(0, len(blob) - RECORD_SIZE + 1, RECORD_SIZE):
        seq, at_us, module, cap, op, a0, a1, res = struct.unpack_from(RECORD_FMT, blob, off)
        out.append(TraceRecord(seq, at_us, module, cap, op, a0, a1, res))
    return out


def replay(records: list[TraceRecord], module: str | None = None,
           capability: str | None = None, op: str | None = None) -> list[TraceRecord]:
    """Deterministic replay: sequence order, optionally scoped (the host mirror of
    `CapabilityReplayScope`, extended with op)."""
    ordered = sorted(records, key=lambda r: r.seq)
    return [r for r in ordered
            if (module is None or r.module_name == module)
            and (capability is None or r.capability_name == capability)
            and (op is None or r.op_name == op)]


def to_audit(records: list[TraceRecord]) -> dict:
    """A capability audit: counts per (module, capability, op) + the ordered trace."""
    by: dict[tuple, int] = {}
    faults = 0
    for r in records:
        by[(r.module_name, r.capability_name, r.op_name)] = \
            by.get((r.module_name, r.capability_name, r.op_name), 0) + 1
        if r.op_name == "fault":
            faults += 1
    return {
        "kind": "capability_replay",
        "records": len(records),
        "faults": faults,
        "by_scope": [{"module": m, "capability": c, "op": o, "count": n}
                     for (m, c, o), n in sorted(by.items())],
        "trace": [r.to_dict() for r in replay(records)],
    }
