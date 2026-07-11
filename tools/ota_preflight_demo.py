#!/usr/bin/env python3
"""Contract-aware OTA preflight demo: sign -> verify -> admission -> boot, one script.

This is the "audit-survivable OTA" story made runnable on the host with no hardware. It
chains the checks a NobroRTOS node performs before it will run a new image, reusing the
real building blocks (never re-implementing crypto or contract rules):

  1. SIGN      the ASYMMETRIC contract (nobro_secure::security_v2): the manifest binds
               key_id, version, image geometry, and the SHA-256 measurement into one
               domain-separated signing digest, signed with Ed25519 against a pinned
               public key. (Falls back to the legacy HMAC scheme with a warning only
               when the host lacks the `cryptography` package; evidence records which.)
  2. VERIFY    re-measure + pinned-key Ed25519 verify + PERSISTENT anti-rollback floor
               (the PersistentBootController model: stage -> try -> confirm advances a
               monotonic floor that survives this process) + Cortex-M vector policy.
               The NEGATIVE cases run too: a tampered image, a wrong-key signature, and
               a downgrade below the *persisted* floor must all be rejected.
  3. ADMISSION validate the system contract bundle (capabilities, deadlines, startup DAG)
               via nobro_rtos.NobroContractBundle, then seal an admission report.
  4. BOOT      assemble the six-stage boot-report bundle and decode it through the host
               contract ABI (BootReportSummary + BootDiagnostic) - the same host decode a
               real node emits.

    python tools/ota_preflight_demo.py                 # synthesized image, full chain
    python tools/ota_preflight_demo.py --image app.bin # price a real firmware image
    python tools/ota_preflight_demo.py --min-version 5 # tighter anti-rollback floor

Emits _work/evidence/ota_preflight.json. Exit 0 only when the good image passes every
stage AND every negative case is correctly rejected. Bench-agnostic output.
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
# Domain separator - byte-identical to nobro_secure::security_v2::signing_digest.
SIGNING_DOMAIN = b"NobroRTOS Ed25519 image manifest v1"
KEY_ID = 1

try:
    from cryptography.hazmat.primitives.asymmetric.ed25519 import (  # noqa: E402
        Ed25519PrivateKey,
        Ed25519PublicKey,
    )
    HAVE_ED25519 = True
except ImportError:  # pragma: no cover - environments without `cryptography`
    HAVE_ED25519 = False


def signing_digest(manifest: dict) -> bytes:
    """SHA-256 digest exactly as the device computes it (security_v2)."""
    import hashlib
    import struct
    hasher = hashlib.sha256()
    hasher.update(SIGNING_DOMAIN)
    hasher.update(struct.pack("<6I", manifest["key_id"], manifest["version"],
                              manifest["image_len"], manifest["load_addr"],
                              manifest["entry_addr"], manifest["stack_top"]))
    hasher.update(bytes.fromhex(manifest["measurement"]))
    return hasher.digest()


# --- 1. SIGN ---------------------------------------------------------------------------

def synth_image() -> bytes:
    """A deterministic pseudo-firmware so the demo runs with no build/hardware."""
    body = bytes((i * 37 + 11) & 0xFF for i in range(4096))
    return b"NOBRO-OTA-DEMO-IMAGE\x00" + body


def make_manifest(image: bytes, version: int, key: bytes,
                  load_addr: int, entry_addr: int, stack_top: int,
                  signer=None) -> dict:
    measurement = sign_firmware.measure(image)
    manifest = {
        "key_id": KEY_ID,
        "version": version,
        "image_len": len(image),
        "load_addr": load_addr,
        "entry_addr": entry_addr,
        "stack_top": stack_top,
        "measurement": measurement.hex(),
    }
    if signer is not None:
        manifest["scheme"] = "ed25519-manifest-v1"
        manifest["signature"] = signer.sign(signing_digest(manifest)).hex()
    else:
        manifest["scheme"] = "hmac-legacy"
        manifest["signature"] = sign_firmware.sign(key, measurement, version).hex()
    return manifest


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


def verify(image: bytes, manifest: dict, key: bytes, min_version: int,
           pinned_public=None) -> dict:
    """SecureBoot verdict, mirroring verify_signed_boot's check order:
    length -> measurement -> rollback -> pinned key -> signature -> vectors."""
    if len(image) != manifest["image_len"]:
        return {"verdict": "RejectTampered", "accepted": False,
                "reason": "image length differs from manifest"}
    actual = sign_firmware.measure(image)
    if actual.hex() != manifest["measurement"]:
        return {"verdict": "RejectTampered", "accepted": False,
                "reason": "measurement mismatch (image changed after signing)"}
    if manifest["version"] < min_version:
        return {"verdict": "RejectRollback", "accepted": False,
                "reason": f"version {manifest['version']} below floor {min_version}"}
    if manifest.get("scheme") == "ed25519-manifest-v1":
        if pinned_public is None or manifest["key_id"] != KEY_ID:
            return {"verdict": "RejectSignature", "accepted": False,
                    "reason": "no pinned key for this key_id"}
        try:
            pinned_public.verify(bytes.fromhex(manifest["signature"]),
                                 signing_digest(manifest))
        except Exception:
            return {"verdict": "RejectSignature", "accepted": False,
                    "reason": "Ed25519 signature invalid against the pinned key"}
    else:
        expected_sig = sign_firmware.sign(key, actual, manifest["version"]).hex()
        if expected_sig != manifest["signature"]:
            return {"verdict": "RejectSignature", "accepted": False,
                    "reason": "signature does not match this key/version"}
    ok, why = vector_policy_ok(manifest)
    if not ok:
        return {"verdict": "RejectVectorPolicy", "accepted": False, "reason": why}
    return {"verdict": "Accept", "accepted": True, "reason": "measurement+signature+"
            "version+vectors all valid"}


# --- persistent boot state (PersistentBootController model) ----------------------------

class PersistentFloor:
    """Monotonic anti-rollback floor persisted across process runs - the host
    model of PersistentBootController's stage -> try -> confirm contract."""

    def __init__(self, path: str, initial: int):
        self.path = path
        self.state = {"generation": 0, "floor": initial, "confirmed": None}
        if os.path.isfile(path):
            with open(path, "r", encoding="utf-8") as f:
                stored = json.load(f)
            # The floor only ever ratchets upward, whatever the CLI says.
            if stored.get("floor", 0) > initial:
                self.state = stored

    def confirm(self, version: int) -> bool:
        if version < self.state["floor"]:
            return False
        self.state = {"generation": self.state["generation"] + 1,
                      "floor": version, "confirmed": version}
        with open(self.path, "w", encoding="utf-8") as f:
            json.dump(self.state, f)
        return True


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

    # Deterministic demo keypair (asymmetric path) - derived from the demo key
    # bytes so the evidence is reproducible; never a production key.
    signer = wrong_signer = pinned_public = None
    if HAVE_ED25519:
        signer = Ed25519PrivateKey.from_private_bytes(key)
        wrong_signer = Ed25519PrivateKey.from_private_bytes(bytes([0xA5]) * 32)
        pinned_public = signer.public_key()
    scheme = "ed25519-manifest-v1" if HAVE_ED25519 else "hmac-legacy"

    # Persistent monotonic rollback floor (PersistentBootController model).
    floor_store = PersistentFloor(
        os.path.join(args.out_dir, "ota_boot_state.json"), args.min_version)
    floor = floor_store.state["floor"]

    # 1. SIGN
    manifest = make_manifest(image, args.version, key, load_addr=0x26000,
                             entry_addr=0x26401, stack_top=0x20040000, signer=signer)
    print(f"[1/4] SIGN      {image_name} v{args.version} ({len(image)} B) "
          f"scheme={scheme}")
    print(f"                measurement {manifest['measurement'][:16]}... "
          f"sig {manifest['signature'][:16]}...")

    # 2. VERIFY - good path plus every rejection the preflight must catch.
    good = verify(image, manifest, key, floor, pinned_public)
    tampered_img = bytearray(image); tampered_img[0] ^= 0xFF
    tampered = verify(bytes(tampered_img), manifest, key, floor, pinned_public)
    rollback_manifest = make_manifest(image, max(floor - 1, 0), key,
                                      0x26000, 0x26401, 0x20040000, signer=signer)
    rollback = verify(image, rollback_manifest, key, floor, pinned_public)
    if HAVE_ED25519:
        forged_manifest = make_manifest(image, args.version, key, 0x26000, 0x26401,
                                        0x20040000, signer=wrong_signer)
        forged = verify(image, forged_manifest, key, floor, pinned_public)
    else:
        forged = {"verdict": "SkippedNoEd25519", "accepted": False,
                  "reason": "cryptography package unavailable"}
    verify_ok = (good["accepted"] and tampered["verdict"] == "RejectTampered"
                 and rollback["verdict"] == "RejectRollback"
                 and (not HAVE_ED25519 or forged["verdict"] == "RejectSignature"))
    # Confirm the accepted version: the persisted floor ratchets monotonically,
    # so the NEXT run rejects this version's downgrades even with a looser CLI.
    confirmed = good["accepted"] and floor_store.confirm(args.version)
    print(f"[2/4] VERIFY    good={good['verdict']}  tampered={tampered['verdict']}  "
          f"rollback={rollback['verdict']}  forged={forged['verdict']}  "
          f"floor={floor}->{floor_store.state['floor']}  "
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

    all_ok = bool(good["accepted"] and verify_ok and confirmed
                  and admission["admitted"] and boot["passing"])
    pack = {
        "tool": "ota preflight demo",
        "chain": ["sign", "verify", "admission", "boot"],
        "scheme": scheme,
        "image": {"name": image_name, "len": len(image), "version": args.version},
        "min_version": args.min_version,
        "persistent_floor": floor_store.state,
        "sign": {"measurement": manifest["measurement"],
                 "signature": manifest["signature"],
                 "key_id": manifest["key_id"],
                 "load_addr": manifest["load_addr"],
                 "entry_addr": manifest["entry_addr"],
                 "stack_top": manifest["stack_top"]},
        "verify": {"good": good, "tampered": tampered, "rollback": rollback,
                   "forged_key": forged, "ok": verify_ok},
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
