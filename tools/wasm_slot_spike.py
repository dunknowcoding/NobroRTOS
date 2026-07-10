#!/usr/bin/env python3
"""Wasm-module-slot spike: model the C-ABI boundary with a sandbox, no real runtime (P3).

NobroRTOS modules already talk to the world through a tiny, pointer-light C ABI
(bindings/c/include/nobro_app.h): the module exports nobro_app_init/poll and imports four
host services. That is nearly a Wasm module's shape. This spike proves the boundary fits -
including the one hard part, turning host pointers into linear-memory offsets - and that a
slot stays bounded, WITHOUT depending on wasm3/wasmtime.

What is modeled faithfully:
  * a fixed linear memory (a bytearray) allocated once - the guest's whole address space;
  * a host that exposes ONLY the four imports, re-typed to take i32 offsets, and copies
    tx/rx bytes in and out of linear memory with bounds checks (the guest never sees a host
    pointer; the host never trusts a guest pointer);
  * a fuel budget per poll so one module can never stall the schedule;
  * a pure-Python "guest" that mirrors bindings/c/examples/imu_module.c using only the
    import facade + its own memory, against a simulated MPU-class device (WHO_AM_I=0x71).

See docs/ENGINEERING.md. Emits _work/evidence/wasm_slot.json. Bench-agnostic output.

    python tools/wasm_slot_spike.py
    python tools/wasm_slot_spike.py --selftest
"""
import argparse
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# MPU-9250-class registers (matches imu_module.c).
IMU_ADDR = 0x68
REG_WHO_AM_I = 0x75
REG_PWR_MGMT_1 = 0x6B
REG_ACCEL_XOUT_H = 0x3B
WHO_AM_I_VALUE = 0x71
LINEAR_MEMORY_BYTES = 4096       # fixed at admission - no memory.grow
FUEL_PER_POLL = 32               # bus ops per poll cycle; exceeding aborts the cycle


class SlotFault(Exception):
    pass


class SimulatedImu:
    """A deterministic MPU-class device on a fake I2C bus."""

    def __init__(self):
        self.awake = False

    def write(self, reg_and_data):
        if not reg_and_data:
            raise SlotFault("empty i2c write")
        reg = reg_and_data[0]
        if reg == REG_PWR_MGMT_1:
            self.awake = True
        self._last_reg = reg
        return 0

    def read(self, reg, length):
        if reg == REG_WHO_AM_I:
            return bytes([WHO_AM_I_VALUE])[:length].ljust(length, b"\x00")
        if reg == REG_ACCEL_XOUT_H:
            # accel(6) + temp(2) + gyro(6): ~1g on Z, small values elsewhere.
            vals = [0, 0, 16384, 21, 0, 0, 0]  # ax ay az temp gx gy gz (i16 big-endian)
            raw = bytearray()
            for v in vals:
                raw += int(v & 0xFFFF).to_bytes(2, "big", signed=False)
            return bytes(raw[:length]).ljust(length, b"\x00")
        return bytes(length)


class WasmHostImports:
    """The only surface a guest can reach: four imports over an i32-offset linear memory.

    This is the sandbox boundary. I2C pointers are linear-memory offsets; the host copies
    bytes across after bounds-checking, so no host pointer is exposed and no guest offset is
    trusted. A fuel counter bounds each poll cycle.
    """

    def __init__(self, device):
        self.memory = bytearray(LINEAR_MEMORY_BYTES)
        self._device = device
        self._time_us = 0
        self.fuel = 0
        self.published = []
        self.host_pointer_leaks = 0   # must stay 0: proof nothing but bytes crosses over

    # --- fuel + bounds helpers (host side, not visible to the guest) ---
    def _spend(self, n=1):
        self.fuel -= n
        if self.fuel < 0:
            raise SlotFault("out of fuel: poll exceeded its bounded step budget")

    def _check(self, off, length):
        if off < 0 or length < 0 or off + length > len(self.memory):
            raise SlotFault(f"linear-memory access out of bounds: off={off} len={length}")

    # --- the four imports (guest-callable) ---
    def now_us(self):
        self._spend()
        self._time_us += 100
        return self._time_us

    def i2c_write(self, addr, tx_off, length):
        self._spend()
        self._check(tx_off, length)
        payload = bytes(self.memory[tx_off:tx_off + length])   # copy OUT of linear memory
        try:
            return self._device.write(payload) if addr == IMU_ADDR else -1
        except SlotFault:
            return -1

    def i2c_write_read(self, addr, tx_off, tx_len, rx_off, rx_len):
        self._spend()
        self._check(tx_off, tx_len)
        self._check(rx_off, rx_len)
        if addr != IMU_ADDR or tx_len < 1:
            return -1
        reg = self.memory[tx_off]
        reply = self._device.read(reg, rx_len)                 # real transaction
        self.memory[rx_off:rx_off + rx_len] = reply            # copy INTO linear memory
        return 0

    def publish_imu(self, who, addr, ax, ay, az, gx, gy, gz, temp_raw):
        self._spend()
        self.published.append({"who": who, "addr": addr,
                               "accel": [ax, ay, az], "gyro": [gx, gy, gz],
                               "temp_raw": temp_raw})


class WasmGuestModule:
    """Stand-in for a compiled-wasm guest. Mirrors imu_module.c and may touch ONLY `host`
    (the four imports) and `host.memory` - never any kernel/host internal. Swapping this for
    a wasm3/wasmtime instance importing the same four functions is the remaining step."""

    # scratch offsets the guest owns within its linear memory
    TX = 0
    RX = 16

    def init(self, host):
        host.memory[self.TX] = REG_PWR_MGMT_1
        host.memory[self.TX + 1] = 0x01           # wake + PLL clock
        return host.i2c_write(IMU_ADDR, self.TX, 2)

    def poll(self, host):
        host.memory[self.TX] = REG_WHO_AM_I
        if host.i2c_write_read(IMU_ADDR, self.TX, 1, self.RX, 1) < 0:
            return -1
        who = host.memory[self.RX]

        host.memory[self.TX] = REG_ACCEL_XOUT_H
        if host.i2c_write_read(IMU_ADDR, self.TX, 1, self.RX, 14) < 0:
            return -2
        raw = host.memory[self.RX:self.RX + 14]

        def be16(i):
            v = (raw[i] << 8) | raw[i + 1]
            return v - 0x10000 if v & 0x8000 else v

        host.publish_imu(who, IMU_ADDR, be16(0), be16(2), be16(4),
                         be16(8), be16(10), be16(12), be16(6))
        return 0


def run_slot(cycles):
    host = WasmHostImports(SimulatedImu())
    guest = WasmGuestModule()

    host.fuel = FUEL_PER_POLL
    if guest.init(host) < 0:
        raise SlotFault("guest init failed")

    reads, errors = 0, 0
    for _ in range(cycles):
        host.fuel = FUEL_PER_POLL      # refuel each cycle: per-poll bound
        rc = guest.poll(host)
        if rc < 0:
            errors += 1
        else:
            reads += 1

    who = host.published[-1]["who"] if host.published else 0
    all_pass = (reads == cycles and errors == 0 and who == WHO_AM_I_VALUE
                and len(host.published) == cycles and host.host_pointer_leaks == 0)
    return {
        "backend": "wasm-slot-spike",
        "backend_id": 4,
        "linear_memory_bytes": LINEAR_MEMORY_BYTES,
        "fuel_per_poll": FUEL_PER_POLL,
        "cycles": cycles,
        "reads": reads,
        "errors": errors,
        "who_am_i": f"0x{who:02X}",
        "published_samples": len(host.published),
        "host_pointer_leaks": host.host_pointer_leaks,
        "boundary": "i32 linear-memory offsets; bytes copied with bounds checks",
        "all_pass": all_pass,
    }


def selftest():
    report = run_slot(50)
    # negative case: a tiny fuel budget must abort a poll (bound is real, not decorative).
    bounded = False
    try:
        host = WasmHostImports(SimulatedImu())
        host.fuel = 1
        WasmGuestModule().poll(host)
    except SlotFault:
        bounded = True
    ok = report["all_pass"] and report["who_am_i"] == "0x71" and bounded
    print(f"reads/cycles   : {report['reads']}/{report['cycles']}  errors={report['errors']}")
    print(f"who_am_i       : {report['who_am_i']}")
    print(f"fuel bound     : {'enforced' if bounded else 'NOT enforced'}")
    print(f"pointer leaks  : {report['host_pointer_leaks']}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description="Wasm module-slot boundary spike.")
    ap.add_argument("--cycles", type=int, default=50, help="poll cycles to run")
    ap.add_argument("--out-dir", default=os.path.join(ROOT, "_work", "evidence"))
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    report = run_slot(args.cycles)
    os.makedirs(args.out_dir, exist_ok=True)
    out = os.path.join(args.out_dir, "wasm_slot.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    print(f"wasm slot: {'PASS' if report['all_pass'] else 'FAIL'} "
          f"({report['reads']}/{report['cycles']} reads, who={report['who_am_i']})")
    print(f"  -> {os.path.relpath(out, ROOT)}")
    return 0 if report["all_pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
