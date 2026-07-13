#!/usr/bin/env python3
"""Secure-boot firmware signer - the authority side of nobro_secure::SecureBoot.

Produces the signature the device verifies before booting an image:
  measurement = SHA-256(image)
  signature   = HMAC-SHA256(boot_key, measurement || version_le)
matching the Rust SecureBoot::measure/sign byte-for-byte (a pinned vector is asserted on
both sides). Stdlib only.

  python3 tools/sign_firmware.py firmware.bin --version 2 --key-hex <64 hex chars>
  python3 tools/sign_firmware.py --selftest
"""
import argparse
import hashlib
import hmac
import json
import struct
import sys


def measure(image: bytes) -> bytes:
    return hashlib.sha256(image).digest()


def sign(boot_key: bytes, measurement: bytes, version: int) -> bytes:
    msg = measurement + struct.pack("<I", version)
    return hmac.new(boot_key, msg, hashlib.sha256).digest()


def selftest() -> int:
    key = bytes([0x5A]) * 32
    m = measure(b"nobro")
    sig = sign(key, m, 1)
    # the same vector nobro_secure's secure_boot_tests pins
    ok = list(sig[:4]) == [0xBB, 0x49, 0x2F, 0x39]
    print(f"measurement(nobro) = {m.hex()}")
    print(f"sig[:4]            = {list(sig[:4])} (device pin: [187, 73, 47, 57])")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("image", nargs="?")
    ap.add_argument("--version", type=int, default=1)
    ap.add_argument("--key-hex", help="32-byte boot key as 64 hex chars")
    ap.add_argument("--load-addr", type=lambda s: int(s, 0), default=0,
                    help="optional image load address for manifest output")
    ap.add_argument("--entry-addr", type=lambda s: int(s, 0), default=0,
                    help="optional reset/entry address for manifest output")
    ap.add_argument("--stack-top", type=lambda s: int(s, 0), default=0,
                    help="optional initial stack pointer for manifest output")
    ap.add_argument("--manifest-out", help="write signed image manifest JSON")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest or not args.image:
        return selftest()
    key = bytes.fromhex(args.key_hex) if args.key_hex else bytes([0x5A]) * 32
    if len(key) != 32:
        sys.exit("boot key must be 32 bytes")
    image = open(args.image, "rb").read()
    m = measure(image)
    sig = sign(key, m, args.version)
    print(f"image        : {args.image} ({len(image)} bytes)")
    print(f"version      : {args.version}")
    print(f"measurement  : {m.hex()}")
    print(f"signature    : {sig.hex()}")
    if args.manifest_out:
        manifest = {
            "version": args.version,
            "image_len": len(image),
            "load_addr": args.load_addr,
            "entry_addr": args.entry_addr,
            "stack_top": args.stack_top,
            "measurement": m.hex(),
            "signature": sig.hex(),
        }
        with open(args.manifest_out, "w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=2)
        print(f"manifest     : {args.manifest_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
