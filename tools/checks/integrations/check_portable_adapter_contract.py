#!/usr/bin/env python3
"""Guard the portable adapter lifecycle and MPU9250 composition boundary."""

from __future__ import annotations

import json
import pathlib


ROOT = pathlib.Path(__file__).resolve().parents[3]


def require_text(path: pathlib.Path, needles: tuple[str, ...], errors: list[str]) -> str:
    if not path.is_file():
        errors.append(f"missing {path.relative_to(ROOT)}")
        return ""
    text = path.read_text(encoding="utf-8")
    for needle in needles:
        if needle not in text:
            errors.append(f"{path.relative_to(ROOT)}: missing {needle!r}")
    return text


def validate() -> list[str]:
    errors: list[str] = []
    lifecycle = require_text(
        ROOT / "core/crates/nobro_device/src/portable_adapter.rs",
        (
            "pub enum AdapterBackendKind",
            "Native = 1",
            "EmbeddedHal = 2",
            "Arduino = 3",
            "C = 4",
            "pub trait PortableAdapterBackend",
            "pub struct MountedAdapter<B: PortableAdapterBackend>",
            "ProviderLifecycle",
            "AdapterCapability::Deadline",
            "AdapterCapability::PartialProgress",
            "AdapterCapability::Cancellation",
            "pub fn recover(",
            "self.lifecycle.release(self.session, cleaned)?",
        ),
        errors,
    )
    if lifecycle and "Vec<" in lifecycle:
        errors.append("portable adapter lifecycle must remain heap-free")

    manifest = require_text(
        ROOT / "core/adapters/imu/mpu9250-imu/Cargo.toml",
        (
            'embedded-hal = "1.0"',
            "nobro-hal = {",
            "optional = true",
            'board-promicro-nosd = [',
            'board-promicro-s140 = [',
        ),
        errors,
    )
    for dependency in ("nobro-hal", "nobro-eh-i2c", "cortex-m"):
        line = next(
            (line for line in manifest.splitlines() if line.startswith(f"{dependency} = ")),
            "",
        )
        if line and "optional = true" not in line:
            errors.append(f"MPU9250 dependency {dependency} must remain optional")

    adapter = require_text(
        ROOT / "core/adapters/imu/mpu9250-imu/src/lib.rs",
        (
            "pub struct PortableMpu9250Imu<B, C, D, I, L>",
            "B: I2c",
            "C: MonotonicClock",
            "D: DelayNs",
            "I: InterruptSource",
            "L: BusLease<B>",
            '#[cfg(feature = "nrf52840")]',
            "pub type Mpu9250Imu =",
        ),
        errors,
    )
    prefix = adapter.split('#[cfg(feature = "nrf52840")]', 1)[0]
    for nrf_symbol in ("nobro_hal::", "NobroI2c", "ActivePlatform", "cortex_m::"):
        if nrf_symbol in prefix:
            errors.append(
                f"portable MPU9250 core references nRF-only symbol {nrf_symbol!r}"
            )

    require_text(
        ROOT / "core/apps/interop/portable_adapter_conformance/src/lib.rs",
        (
            "pub fn exercise<B: PortableAdapterBackend>",
            "begin_operation",
            "advance_operation",
            "cancel_operation",
            "recover()",
            "check::<1>()",
            "check::<2>()",
            "check::<3>()",
            "check::<4>()",
        ),
        errors,
    )
    require_text(
        ROOT / "core/Cargo.toml",
        ('"apps/interop/portable_adapter_conformance"',),
        errors,
    )
    require_text(
        ROOT / "tools/checks/platforms/check_portability.sh",
        ("nobro-adapter-mpu9250-imu", "portable-adapter-conformance"),
        errors,
    )
    require_text(
        ROOT / "tools/checks/ci_matrix.sh",
        ("portable adapter conformance", "portable-adapter-conformance"),
        errors,
    )
    require_text(
        ROOT / "tools/checks/core/check_host_workspace.py",
        ('"portable-adapter-conformance",',),
        errors,
    )

    catalog_path = ROOT / "core/adapters/catalog.json"
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    component = next(
        (
            item
            for item in catalog.get("components", [])
            if item.get("id") == "adapter-imu-mpu9250"
        ),
        None,
    )
    if component is None:
        errors.append("catalog is missing adapter-imu-mpu9250")
    else:
        if component.get("maturity") != "implemented":
            errors.append("MPU9250 catalog maturity must remain implemented")
        if component.get("evidence") != ["host-test", "target-build"]:
            errors.append("MPU9250 catalog evidence must match the gated tests")
        if component.get("supported_targets") != ["embedded-hal-1", "nrf52840"]:
            errors.append("MPU9250 catalog targets must include portable and nRF scopes")
        if "physical" in component.get("evidence", []):
            errors.append("MPU9250 must not claim public physical promotion")

    return errors


def main() -> int:
    errors = validate()
    if errors:
        print("PORTABLE ADAPTER CONTRACT: FAIL")
        for error in errors:
            print(f"- {error}")
        return 1
    print(
        "PORTABLE ADAPTER CONTRACT: PASS "
        "(one lifecycle; native/embedded-hal/Arduino/C; injected MPU9250 transport)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
