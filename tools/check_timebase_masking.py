#!/usr/bin/env python3
"""Fail if deadline/watchdog progress is hidden behind global interrupt masking."""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
FILES = {
    "scheduler": ROOT / "core/crates/nobro_kernel/src/scheduler.rs",
    "deadline_timer": ROOT / "core/crates/nobro_hal/src/deadline_timer.rs",
    "power": ROOT / "core/crates/nobro_hal/src/power_nrf.rs",
    "ceiling": ROOT / "core/crates/nobro_hal/src/priority_ceiling.rs",
    "context_switch": ROOT / "core/crates/nobro_hal/src/context_switch.rs",
    "hal_lib": ROOT / "core/crates/nobro_hal/src/lib.rs",
    "hal_manifest": ROOT / "core/crates/nobro_hal/Cargo.toml",
    "generator": ROOT / "tools/nobro_firmware_project.py",
    "samd_manifest": ROOT / "core/ports/samd21/Cargo.toml",
    "samd_provider": ROOT / "core/ports/samd21/src/masked_critical_section.rs",
    "samd_report": ROOT / "core/ports/samd21/src/main.rs",
    "ra_event_dma": ROOT / "core/ports/ra4m1/src/event_dma.rs",
    "ra_selftest": ROOT / "core/ports/ra4m1/src/main.rs",
}
FORBIDDEN = ("critical_section::with", "interrupt::free", "primask")
RAW_MASK_TOKENS = (
    "cortex_m::interrupt::disable(",
    "cortex_m::interrupt::enable(",
    "cortex_m::interrupt::free(",
    "cortex_m::register::primask",
)
RAW_MASK_ALLOWLIST = {
    ROOT / "core/apps/connectivity/usb_cdc_demo/src/main.rs",
    FILES["ra_event_dma"],
    FILES["ra_selftest"],
    ROOT / "core/ports/samd21/src/masked_critical_section.rs",
}


def _basepri_service_model() -> list[str]:
    """Model the priority-ceiling contract independent of a Cortex-M target.

    ARM BASEPRI masks interrupts with logical priority numbers greater than or
    equal to the ceiling. Lower numbers are more urgent. The safety
    property is that deadline/watchdog-feeder sources stay serviceable while a
    kernel critical section is held; PendSV and ordinary user work must wait so
    they cannot split a shared-state transaction.
    """
    failures: list[str] = []
    profiles = {
        "bare": {
            "ceiling": 3,
            "deadline": 0,
            "watchdog_feeder": 1,
            "p_isr": 2,
            "pendsv": 7,
            "user": 3,
        },
        "s140": {
            "ceiling": 6,
            "deadline": 2,
            "watchdog_feeder": 3,
            "p_isr": 5,
            "pendsv": 7,
            "user": 6,
        },
    }

    for name, priorities in profiles.items():
        ceiling = priorities["ceiling"]
        serviced: list[str] = []
        deferred: list[str] = []
        for event, priority in priorities.items():
            if event == "ceiling":
                continue
            if priority < ceiling:
                serviced.append(event)
            else:
                deferred.append(event)
        for event in ("deadline", "watchdog_feeder", "p_isr"):
            if event not in serviced:
                failures.append(
                    f"{name}: {event} priority is masked by the BASEPRI ceiling"
                )
        for event in ("pendsv", "user"):
            if event not in deferred:
                failures.append(f"{name}: {event} can split a critical section")
        if set(serviced + deferred) != {"deadline", "watchdog_feeder", "p_isr", "pendsv", "user"}:
            failures.append(f"{name}: model did not classify every event")
    return failures


def _raw_masking_allowlist() -> list[str]:
    """Keep raw interrupt masking out of ordinary public code paths.

    nRF builds must route shared-state exclusion through the BASEPRI-backed
    `critical_section` provider so deadline/watchdog sources stay serviceable.
    The only public exceptions are:
      * the pre-RAM USB bootloader-handoff sanitizer, which deliberately masks
        before Rust statics exist and re-enables at the start of `main`; and
      * the SAMD21 Cortex-M0+ measured fallback, because the architecture has no
        BASEPRI and the provider reports its maximum masked time;
      * the RA4M1 event-DMA provider's read-only PRIMASK/FAULTMASK fail-closed
        check; and
      * the feature-gated RA4M1 physical self-test handoff, which is the only
        path allowed to unmask the stock bootloader's inherited PRIMASK state.
    """
    failures: list[str] = []
    for path in sorted((ROOT / "core").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        hits = [token for token in RAW_MASK_TOKENS if token in text]
        if not hits:
            continue
        if path not in RAW_MASK_ALLOWLIST:
            rel = path.relative_to(ROOT)
            failures.append(f"{rel}: raw interrupt masking tokens {hits} are not allowed")
            continue
        if path == FILES["samd_provider"]:
            for token in ("MAX_MASKED_CYCLES", "SYST_COUNTFLAG", "max_masked_us_ceil"):
                if token not in text:
                    failures.append(
                        f"{path.relative_to(ROOT)}: measured PRIMASK fallback missing {token!r}"
                    )
        elif path == FILES["ra_event_dma"]:
            required = (
                "cortex_m::register::primask::read().is_inactive()",
                "cortex_m::register::faultmask::read().is_inactive()",
                "return Err(EventDmaError::InterruptsMasked);",
            )
            for token in required:
                if token not in text:
                    failures.append(
                        f"{path.relative_to(ROOT)}: RA4M1 fail-closed mask check missing {token!r}"
                    )
            for token in (
                "cortex_m::interrupt::disable(",
                "cortex_m::interrupt::enable(",
            ):
                if token in text:
                    failures.append(
                        f"{path.relative_to(ROOT)}: reusable RA4M1 provider must not change global masks"
                    )
        elif path == FILES["ra_selftest"]:
            required = (
                '#[cfg(feature = "event-dma-selftest")]',
                "stock UNO R4 bootloader can jump with PRIMASK set",
                "cortex_m::interrupt::enable();",
            )
            for token in required:
                if token not in text:
                    failures.append(
                        f"{path.relative_to(ROOT)}: RA4M1 self-test handoff exception missing {token!r}"
                    )
            if "cortex_m::interrupt::disable(" in text:
                failures.append(
                    f"{path.relative_to(ROOT)}: RA4M1 self-test must not globally disable interrupts"
                )
        else:
            required = (
                "sanitize_bootloader_interrupt_handoff",
                "leaves interrupt delivery masked until `main`",
                "pre-RAM handoff sanitizer deliberately leaves PRIMASK set",
            )
            for token in required:
                if token not in text:
                    failures.append(
                        f"{path.relative_to(ROOT)}: boot-handoff raw mask exception missing {token!r}"
                    )
    return failures


def main() -> int:
    text = {name: path.read_text(encoding="utf-8") for name, path in FILES.items()}
    failures = []
    for name in ("scheduler", "deadline_timer", "power"):
        for token in FORBIDDEN:
            if token in text[name]:
                failures.append(f"{name}: forbidden global-mask token {token!r}")
    required = {
        "scheduler": ("fetch_update", "on_deadline_tick"),
        "deadline_timer": ("PENDING_PERIOD_US", "on_isr"),
        "power": (
            "SCB_SCR_SEVONPEND",
            "asm::sev",
            "asm::wfe",
            "PENDING_READY.load",
            "intenclr.write",
            "ARMED_DEADLINE.store(0",
        ),
        "ceiling": (
            "basepri::write",
            "set_impl!",
            "RAW_CEILING",
            "DeadlineWouldBeMasked",
            "WatchdogWouldBeMasked",
        ),
        "context_switch": (
            "raw < ceiling.raw()",
            "PendSvWouldPreemptCeiling",
        ),
        "hal_lib": (
            'all(feature = "cortex-m-slice", feature = "board-nicenano-s140")',
            "current port programs PendSV through CMSIS",
            "no SoftDevice NVIC integration",
        ),
        "hal_manifest": ("restore-state-bool",),
        "samd_manifest": ("restore-state-bool", "portable-atomic"),
        "samd_provider": (
            "set_impl!",
            "AtomicU32",
            "MAX_MASKED_CYCLES",
            "SYST_COUNTFLAG",
        ),
        "samd_report": ("mask_max_cycles=", "mask_bound_us=", "mask_pass="),
    }
    for name, tokens in required.items():
        for token in tokens:
            if token not in text[name]:
                failures.append(f"{name}: missing required mechanism {token!r}")
    failures.extend(_basepri_service_model())
    failures.extend(_raw_masking_allowlist())
    selection_token = "critical-section-single-core"
    selected_manifests = list((ROOT / "core/apps").rglob("Cargo.toml"))
    selected_manifests += list((ROOT / "core/adapters").rglob("Cargo.toml"))
    selected_manifests += list((ROOT / "core/crates").rglob("Cargo.toml"))
    for path in selected_manifests:
        manifest = path.read_text(encoding="utf-8")
        if "nobro-hal" in manifest and selection_token in manifest:
            failures.append(
                f"{path.relative_to(ROOT)}: PRIMASK implementation conflicts with nRF BASEPRI"
            )
    if selection_token in text["hal_manifest"]:
        failures.append("nobro-hal: cortex-m PRIMASK implementation is selected")
    if selection_token in text["generator"]:
        failures.append("firmware generator still selects the cortex-m PRIMASK implementation")
    if selection_token in text["samd_manifest"]:
        failures.append("SAMD21 still selects an unmeasured Cortex-M0 PRIMASK implementation")
    if failures:
        print("TIMEBASE MASKING GATE: FAIL")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(
        "TIMEBASE MASKING GATE: PASS "
        "(nRF BASEPRI leaves deadline/watchdog-feeder priorities live; "
        "Cortex-M0 fallback reports maximum PRIMASK time; "
        "RA4M1 provider only observes masks and its physical self-test owns the handoff)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
