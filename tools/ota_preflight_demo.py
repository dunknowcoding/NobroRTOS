#!/usr/bin/env python3
"""Contract-aware OTA preflight demo: sign -> verify -> admission -> boot, one script.

This is the "audit-survivable OTA" story made runnable on the host with no hardware. It
chains the four checks a NobroRTOS node performs before it will run a new image, reusing
the real building blocks (never re-implementing crypto or contract rules):

  1. SIGN      measurement = SHA-256(image); signature = HMAC(boot_key, measurement||ver)
               (tools/sign_firmware.py - byte-identical to nobro_secure::SecureBoot)
  2. VERIFY    re-measure + check signature + anti-rollback + Cortex-M vector policy,
               reproducing SecureBoot::verify verdicts (Accept / RejectTampered /
               RejectSignature / RejectRollback). Also runs the NEGATIVE cases so the
               preflight is shown to *reject* a tampered image and a rollback.
  3. ADMISSION validate the system contract bundle (capabilities, deadlines, startup DAG)
               via nobro_rtos.NobroContractBundle, then seal an admission report.
  4. BOOT      assemble the six-stage boot-report bundle and decode it through the host
               contract ABI (BootReportSummary + BootDiagnostic) - the same host decode a
               real node emits.

    python tools/ota_preflight_demo.py                 # synthesized image, full chain
    python tools/ota_preflight_demo.py --image app.bin # price a real firmware image
    python tools/ota_preflight_demo.py --min-version 5 # tighter anti-rollback floor

Emits _work/evidence/ota_preflight.json. Exit 0 only when the good image passes every
stage AND the tampered/rollback images are correctly rejected. Bench-agnostic output.
"""
import argparse
import json
import os
import sys

TOOLS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(TOOLS)
sys.path.insert(0, TOOLS)
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))

import sign_firmware  # noqa: E402  (measure/sign - identical to the device)
from nobro_rtos import (  # noqa: E402
    BootDiagnostic,
    BootReportSummary,
    Capability,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
    RosBridgeDescriptor,
    RosService,
    RosTopic,
    StartupDependency,
    ReportKind,
    plan_startup,
    seal_report,
)

DEFAULT_KEY = bytes([0x5A]) * 32
# Demo Cortex-M flash window (nRF52840-class): app somewhere in [0x1000, 0x100000).
POLICY_MIN_LOAD = 0x1000
POLICY_MAX_END = 0x100000


# --- 1. SIGN ---------------------------------------------------------------------------

def synth_image() -> bytes:
    """A deterministic pseudo-firmware so the demo runs with no build/hardware."""
    body = bytes((i * 37 + 11) & 0xFF for i in range(4096))
    return b"NOBRO-OTA-DEMO-IMAGE\x00" + body


def make_manifest(image: bytes, version: int, key: bytes,
                  load_addr: int, entry_addr: int, stack_top: int) -> dict:
    measurement = sign_firmware.measure(image)
    signature = sign_firmware.sign(key, measurement, version)
    return {
        "version": version,
        "image_len": len(image),
        "load_addr": load_addr,
        "entry_addr": entry_addr,
        "stack_top": stack_top,
        "measurement": measurement.hex(),
        "signature": signature.hex(),
    }


# --- 2. VERIFY (mirrors nobro_secure::SecureBoot::verify + BootVectorPolicy) -----------

def vector_policy_ok(manifest: dict) -> tuple[bool, str]:
    load, entry, stack, ln = (manifest["load_addr"], manifest["entry_addr"],
                              manifest["stack_top"], manifest["image_len"])
    if ln <= 0:
        return False, "empty image"
    if load < POLICY_MIN_LOAD or load + ln > POLICY_MAX_END:
        return False, "load address outside flash window"
    if entry % 2 == 0:
        return False, "entry not a Thumb (odd) address"
    if not (load <= entry < load + ln):
        return False, "entry outside image"
    if stack % 8 != 0:
        return False, "stack pointer not 8-byte aligned"
    return True, "ok"


def verify(image: bytes, manifest: dict, key: bytes, min_version: int) -> dict:
    """Return the SecureBoot verdict for this image against its manifest + floor."""
    actual = sign_firmware.measure(image)
    if actual.hex() != manifest["measurement"]:
        return {"verdict": "RejectTampered", "accepted": False,
                "reason": "measurement mismatch (image changed after signing)"}
    expected_sig = sign_firmware.sign(key, actual, manifest["version"]).hex()
    if expected_sig != manifest["signature"]:
        return {"verdict": "RejectSignature", "accepted": False,
                "reason": "signature does not match this key/version"}
    if manifest["version"] < min_version:
        return {"verdict": "RejectRollback", "accepted": False,
                "reason": f"version {manifest['version']} below floor {min_version}"}
    ok, why = vector_policy_ok(manifest)
    if not ok:
        return {"verdict": "RejectVectorPolicy", "accepted": False, "reason": why}
    return {"verdict": "Accept", "accepted": True, "reason": "measurement+signature+"
            "version+vectors all valid"}


# --- 3. ADMISSION (real contract bundle) ----------------------------------------------

def reference_bundle() -> NobroContractBundle:
    """A valid system contract: hard-RT kernel + bus + AI + telemetry, startup DAG."""
    return NobroContractBundle(
        metadata={"profile": "ota-preflight"},
        modules=(
            ModuleSpec("kernel", Criticality.HARD_REALTIME,
                       MemoryBudget(12 * 1024, 2 * 1024, 1),
                       period_us=20_000, max_jitter_us=10),
            ModuleSpec("bus", Criticality.DRIVER, MemoryBudget(8 * 1024, 1024, 1),
                       requires=(Capability.BUS0,), owns=(Capability.BUS0,)),
            ModuleSpec("ai", Criticality.USER, MemoryBudget(16 * 1024, 6 * 1024, 1),
                       requires=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
                       owns=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT)),
            ModuleSpec("telemetry", Criticality.BEST_EFFORT,
                       MemoryBudget(8 * 1024, 1024, 1),
                       requires=(Capability.STREAM,), owns=(Capability.STREAM,)),
        ),
        ros_bridges=(
            RosBridgeDescriptor("robot_core", "serial",
                                topics=(RosTopic("/imu", "sensor_msgs/Imu", 4, 128),),
                                services=(RosService("/reset", 16, 16, 50_000),)),
        ),
        startup_dependencies=(
            StartupDependency("bus", "kernel"),
            StartupDependency("ai", "bus"),
            StartupDependency("telemetry", "bus"),
        ),
    )


def run_admission() -> dict:
    bundle = reference_bundle()
    # to_json() raises on any contract violation; this IS the validation gate.
    bundle.to_json()
    order = plan_startup(bundle.modules, bundle.startup_dependencies).order
    flash = sum(m.memory.flash_bytes for m in bundle.modules)
    ram = sum(m.memory.ram_bytes for m in bundle.modules)
    pool = sum(m.memory.pool_slots for m in bundle.modules)
    return {
        "admitted": True,
        "module_count": len(bundle.modules),
        "startup_order": list(order),
        "flash_used_bytes": flash,
        "ram_used_bytes": ram,
        "pool_used_slots": pool,
    }


# --- 4. BOOT (six-stage host-decoded report bundle) -----------------------------------

def healthy_boot_reports(admission: dict) -> dict:
    mods = admission["module_count"]
    return {"reports": {
        "board_profile": seal_report(ReportKind.BOARD_PROFILE, {"max_modules": mods}),
        "board_package": seal_report(ReportKind.BOARD_PACKAGE, {"valid": 1}),
        "manifest": seal_report(ReportKind.MANIFEST,
                                {"valid": 1, "module_count": mods,
                                 "flash_used_bytes": admission["flash_used_bytes"],
                                 "ram_used_bytes": admission["ram_used_bytes"]}),
        "adapter_compatibility": seal_report(ReportKind.ADAPTER_COMPAT, {"compatible": 1}),
        "admission": seal_report(ReportKind.ADMISSION,
                                 {"admitted": 1, "module_count": mods,
                                  "startup_len": len(admission["startup_order"]),
                                  "flash_used_bytes": admission["flash_used_bytes"],
                                  "ram_used_bytes": admission["ram_used_bytes"],
                                  "pool_used_slots": admission["pool_used_slots"]}),
        "runtime": seal_report(ReportKind.RUNTIME, {"state": 3, "module_count": mods}),
    }}


def run_boot(admission: dict) -> dict:
    summary = BootReportSummary.from_dict(healthy_boot_reports(admission))
    code = summary.diagnostic_code()
    diag = BootDiagnostic.decode(code)
    return {
        "passing": summary.passing,
        "diagnostic_code": f"0x{code:08X}",
        "first_stage": summary.first_diagnostic.stage,
        "decoded": {"stage": diag.stage, "status": diag.status_class.name.lower()},
        "status_counts": summary.status_counts(),
    }


# --- orchestration --------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser(description="Contract-aware OTA preflight demo.")
    ap.add_argument("--image", help="firmware image to sign (default: synthesized)")
    ap.add_argument("--version", type=int, default=8, help="candidate image version")
    ap.add_argument("--min-version", type=int, default=5, help="anti-rollback floor")
    ap.add_argument("--key-hex", help="32-byte boot key as 64 hex chars")
    ap.add_argument("--out-dir", default=os.path.join(ROOT, "_work", "evidence"))
    args = ap.parse_args()

    key = bytes.fromhex(args.key_hex) if args.key_hex else DEFAULT_KEY
    if len(key) != 32:
        sys.exit("boot key must be 32 bytes")
    if args.image:
        with open(args.image, "rb") as f:
            image = f.read()
        image_name = os.path.basename(args.image)
    else:
        image = synth_image()
        image_name = "<synthesized>"

    # 1. SIGN
    manifest = make_manifest(image, args.version, key,
                             load_addr=0x26000, entry_addr=0x26401, stack_top=0x20040000)
    print(f"[1/4] SIGN      {image_name} v{args.version} ({len(image)} B)")
    print(f"                measurement {manifest['measurement'][:16]}... "
          f"sig {manifest['signature'][:16]}...")

    # 2. VERIFY - good path plus the two rejections the preflight must catch.
    good = verify(image, manifest, key, args.min_version)
    tampered_img = bytearray(image); tampered_img[0] ^= 0xFF
    tampered = verify(bytes(tampered_img), manifest, key, args.min_version)
    rollback_manifest = make_manifest(image, args.min_version - 1, key,
                                      0x26000, 0x26401, 0x20040000)
    rollback = verify(image, rollback_manifest, key, args.min_version)
    verify_ok = (good["accepted"] and tampered["verdict"] == "RejectTampered"
                 and rollback["verdict"] == "RejectRollback")
    print(f"[2/4] VERIFY    good={good['verdict']}  "
          f"tampered={tampered['verdict']}  rollback={rollback['verdict']}  "
          f"=> {'PASS' if verify_ok else 'FAIL'}")

    # 3. ADMISSION - only meaningful if the image was accepted for install.
    admission = run_admission()
    print(f"[3/4] ADMISSION admitted={admission['admitted']} "
          f"modules={admission['module_count']} "
          f"startup={'->'.join(admission['startup_order'])} "
          f"flash={admission['flash_used_bytes']}B ram={admission['ram_used_bytes']}B")

    # 4. BOOT
    boot = run_boot(admission)
    print(f"[4/4] BOOT      passing={boot['passing']} "
          f"diagnostic={boot['diagnostic_code']} "
          f"({boot['decoded']['stage']}/{boot['decoded']['status']})")

    all_ok = bool(good["accepted"] and verify_ok and admission["admitted"]
                  and boot["passing"])
    pack = {
        "tool": "ota preflight demo",
        "chain": ["sign", "verify", "admission", "boot"],
        "image": {"name": image_name, "len": len(image), "version": args.version},
        "min_version": args.min_version,
        "sign": {"measurement": manifest["measurement"],
                 "signature": manifest["signature"],
                 "load_addr": manifest["load_addr"],
                 "entry_addr": manifest["entry_addr"],
                 "stack_top": manifest["stack_top"]},
        "verify": {"good": good, "tampered": tampered, "rollback": rollback,
                   "ok": verify_ok},
        "admission": admission,
        "boot": boot,
        "result": "PASS" if all_ok else "FAIL",
    }
    os.makedirs(args.out_dir, exist_ok=True)
    out = os.path.join(args.out_dir, "ota_preflight.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(pack, f, indent=2)
    print(f"\nOTA preflight: {pack['result']}  ->  {os.path.relpath(out, ROOT)}")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
