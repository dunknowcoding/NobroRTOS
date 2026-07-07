#!/usr/bin/env python3
"""NiusZigbee gateway -> NobroRTOS host contract bridge (M122).

A NobroRTOS node running the cc2530_gateway app captures 802.15.4 frames with its
CC2530 and exposes per-type counts plus the most recent raw PSDU in the fixed
NOBRO_CC2530_GATEWAY_REPORT struct. This tool turns that report into collector records
using the nobro_rtos.zigbee contract: it decodes the captured frame and emits the
gateway rollup + the decoded frame as JSON the collector ingests.

  python3 tools/zigbee_gateway.py --report 4E5A4757,1,1,1,8,F,5,0,2,0,3,8,0803FFEA,07FFFFFF,...
  python3 tools/zigbee_gateway.py --selftest

Reading the struct off a probe-only board is left to the caller's flashing tool (the
report is a plain memory struct); this stage is the decode + contract, stdlib only.
"""
import argparse
import json
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[0].parent / "bindings" / "python"))
from nobro_rtos.thread import ThreadRollup, decode_thread_record  # noqa: E402
from nobro_rtos.zigbee import GatewayRollup, decode_mac_frame  # noqa: E402

FrameType = __import__("nobro_rtos.zigbee", fromlist=["FrameType"]).FrameType


def report_to_records(words: list[int]) -> dict:
    """Decode the 21-word NOBRO_CC2530_GATEWAY_REPORT into collector records.

    Layout (u32 LE): magic, version, completed, all_pass, fw_version, pongs,
    frames_total, beacons, data, acks, commands, last_len, last_frame[8 words], checksum.
    """
    if len(words) < 21 or words[0] != 0x4E5A4757:
        raise ValueError("not a NOBRO_CC2530_GATEWAY_REPORT (bad magic)")
    (fw, pongs, total, beacons, data, acks, commands, last_len) = words[4:12]
    frame_bytes = b"".join(struct.pack("<I", w) for w in words[12:20])[:last_len]

    roll = GatewayRollup()
    roll.total = total
    roll.counts = {
        k: v for k, v in (
            ("beacon", beacons), ("data", data), ("ack", acks), ("mac_command", commands)
        ) if v
    }
    out = {
        "node": "cc2530-gateway",
        "fw_version": f"0x{fw:04X}",
        "pongs": pongs,
        "rollup": roll.to_record(),
    }
    if last_len:
        out["last_frame"] = decode_mac_frame(frame_bytes).to_record()
        thread = decode_thread_record(frame_bytes)
        if thread:
            roll_thread = ThreadRollup()
            roll_thread.ingest(frame_bytes)
            out["thread_rollup"] = roll_thread.to_record()
            out["last_thread_frame"] = thread
    return out


def selftest() -> int:
    # A synthetic report equivalent to the live bench capture (beacon-request PSDU).
    words = [
        0x4E5A4757, 1, 1, 1,  # magic, version, completed, all_pass
        0x0008, 0x000F, 5, 0, 2, 0, 3,  # fw, pongs, total, beacon, data, ack, command
        8,  # last_len
        0xFFEA0803, 0x07FFFFFF,  # last_frame words (03 08 EA FF FF FF FF 07)
        0, 0, 0, 0, 0, 0,  # remaining last_frame words
        0,  # checksum
    ]
    rec = report_to_records(words)
    ok_zigbee = (
        rec["rollup"]["frames"] == 5
        and rec["last_frame"]["command"] == "beacon_request"
        and rec["last_frame"]["dest_pan"] == "0xFFFF"
    )

    thread_psdu = bytes([
        0x61, 0x88, 0x3D,
        0x34, 0x12, 0x02, 0x00,
        0x01, 0x00,
        0x7A, 0x33, 0x3A, 0x05,
    ])
    thread_words = [
        0x4E5A4757, 1, 1, 1,
        0x0008, 0x000F, 1, 0, 1, 0, 0,
        len(thread_psdu),
    ]
    thread_words += [
        int.from_bytes(thread_psdu[i:i + 4].ljust(4, b"\0"), "little")
        for i in range(0, 32, 4)
    ]
    thread_words += [0]
    thread_rec = report_to_records(thread_words)
    print(json.dumps({"zigbee": rec, "thread": thread_rec}, indent=2))
    ok_thread = (
        thread_rec["last_thread_frame"]["proto"] == "thread"
        and thread_rec["thread_rollup"]["thread_frames"] == 1
        and "iphc" in thread_rec["thread_rollup"]["headers"]
    )
    ok = ok_zigbee and ok_thread
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--report", help="comma-separated hex u32 words of the report struct")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest or not args.report:
        return selftest()
    words = [int(w, 16) for w in args.report.split(",")]
    print(json.dumps(report_to_records(words), indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
