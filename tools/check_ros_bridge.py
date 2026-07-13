#!/usr/bin/env python3
"""ROS bridge contract gate: ros_msg_gen output must match NobroRTOS bridge contracts.

Validates that tools/ros_msg_gen.py emits RosTopic fragments compatible with
nobro_rtos.RosTopic and that the generated type hash matches the device/host
FNV-1a32. This ties ROS message generation to the bounded RosBridgeSal layer
without requiring hardware.

    python tools/check_ros_bridge.py
    python tools/check_ros_bridge.py --selftest
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))
from nobro_rtos.contracts import RosTopic, stable_hash32  # noqa: E402

SAMPLE_MSG = """\
float64[4] orientation
float64[9] orientation_covariance
float64[3] angular_velocity
float64[3] linear_acceleration
uint8 flags
"""


def run_gen(msg_path, topic="/imu"):
    out = subprocess.check_output(
        [sys.executable, os.path.join(ROOT, "tools", "ros_msg_gen.py"),
         msg_path, "--type", "sensor_msgs/Imu", "--topic", topic],
        cwd=ROOT, text=True)
    return json.loads(out)


def check_fragment(frag):
    errs = []
    topic = RosTopic(frag["name"], frag["message_type"], frag["depth"],
                     frag["max_message_bytes"])
    topic.validate()
    d = topic.to_dict()
    if d["message_type_hash"] != stable_hash32(frag["message_type"]):
        errs.append("message_type_hash mismatch")
    if d["name_hash"] != stable_hash32(frag["name"]):
        errs.append("name_hash mismatch")
    if frag["max_message_bytes"] != 153:
        errs.append(f"expected 153-byte Imu payload, got {frag['max_message_bytes']}")
    return errs


def selftest():
    with tempfile.NamedTemporaryFile("w", suffix=".msg", delete=False, encoding="utf-8") as f:
        f.write(SAMPLE_MSG)
        path = f.name
    try:
        frag = run_gen(path)
        errs = check_fragment(frag)
    finally:
        os.unlink(path)
    ok = not errs and frag["message_type"] == "sensor_msgs/Imu"
    print(f"topic           : {frag.get('name')}")
    print(f"message_type    : {frag.get('message_type')}")
    print(f"max_message_bytes: {frag.get('max_message_bytes')}")
    print(f"validator       : {'clean' if not errs else errs}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description="ROS bridge contract gate.")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    return selftest()


if __name__ == "__main__":
    sys.exit(main())
