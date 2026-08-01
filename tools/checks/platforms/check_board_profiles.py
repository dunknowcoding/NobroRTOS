#!/usr/bin/env python3
"""Validate v2 board profiles and their generated Rust catalog."""

from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess
import sys


LAYOUT_FLASH_START = {
    "NoSoftDevice": 0x1000,
    "SoftDeviceS140V6": 0x26000,
    "Esp32IdfApp": 0x10000,
    "Rp2350ImageDef": 0x10000000,
    "Rp2040Boot2": 0x10000000,
    "AvrOptiboot2K": 0,
    "Esp8266Arduino4M2M": 0,
}
PLATFORM_RAM = {
    "nrf52840": (0x2000_0000, 0x2004_0000),
    "esp32c3": (0x3FC8_0000, 0x3FCE_0000),
    "esp32": (0x3FFB_0000, 0x3FFD_C000),
    "esp32p4": (0x4FF4_0000, 0x4FFA_E000),
    "rp2350": (0x2000_0000, 0x2008_2000),
    "rp2040": (0x2000_0000, 0x2004_2000),
    "atmega328p": (0x100, 0x900),
    "esp8266": (0x3FFE_8000, 0x3FFF_C000),
    "ra4m1": (0x2000_0000, 0x2000_8000),
    "samd21": (0x2000_0000, 0x2000_8000),
    "stm32f4": (0x2000_0000, 0x2002_0000),
    "imxrt1062": (0x2020_0000, 0x2028_0000),
    "cortex_m": (0x2000_0000, 0x2000_8000),
}
COMPOSITION_FIELDS = {
    "silicon",
    "physical_board",
    "boot_layout",
    "framework_core",
    "hal_stack",
    "usb_stack",
}
CONNECTED_BOARD_IDS = {
    "promicro_nrf52840_nosd",
    "promicro_nrf52840_s140",
    "rp2350_pico2w",
    "raspberry_pi_pico_rp2040",
    "uno_r4_wifi",
    "samd21_m0_mini",
    "arduino_nano_v3_atmega328p",
    "esp32c3_supermini",
    "esp32s3_uno",
    "xiao_esp32s3_sense",
    "esp32_cam_ov2640",
    "esp32s3_cam_ov5640",
    "esp_wroom32_30pin",
    "esp32p4_pico",
    "wemos_d1_mini_esp8266",
}
ID = re.compile(r"^[a-z0-9][a-z0-9_.:-]*$")
ROOT = Path(__file__).resolve().parents[3] / "core" / "boards"
REPO = ROOT.parents[1]


def as_int(value: object) -> int:
    return int(value, 0) if isinstance(value, str) else int(value)


def check_profile(path: Path) -> tuple[dict, list[str]]:
    with path.open(encoding="utf-8") as handle:
        data = json.load(handle)
    errors: list[str] = []
    required = {
        "schema",
        "board_id",
        "platform_id",
        "feature",
        "feature_aliases",
        "composition",
        "boot",
        "capacity",
        "pins",
        "firmware_generation",
    }
    missing = sorted(required - data.keys())
    if missing:
        return data, ["missing " + ", ".join(missing)]
    if data["schema"] != "nobro-board-profile-v2":
        errors.append("schema must be nobro-board-profile-v2")
    for field in ("board_id", "platform_id", "feature"):
        if not isinstance(data[field], str) or not ID.fullmatch(data[field]):
            errors.append(f"{field} is not a stable identifier")
    aliases = data["feature_aliases"]
    if not isinstance(aliases, list) or not all(
        isinstance(alias, str) and ID.fullmatch(alias) for alias in aliases
    ):
        errors.append("feature_aliases must be a stable-identifier list")

    composition = data["composition"]
    if not isinstance(composition, dict):
        errors.append("composition must be an object")
    else:
        missing_composition = sorted(COMPOSITION_FIELDS - composition.keys())
        extra_composition = sorted(composition.keys() - COMPOSITION_FIELDS)
        if missing_composition:
            errors.append("composition missing " + ", ".join(missing_composition))
        if extra_composition:
            errors.append("composition has unknown " + ", ".join(extra_composition))
        for field in COMPOSITION_FIELDS:
            value = composition.get(field)
            if value is not None and (
                not isinstance(value, str) or not ID.fullmatch(value)
            ):
                errors.append(f"composition.{field} is not null or a stable identifier")
        if not composition.get("physical_board") or not composition.get("boot_layout"):
            errors.append("physical_board and boot_layout must be selected")

    boot = data["boot"]
    boot_fields = (
        "app_flash_start",
        "app_flash_len_bytes",
        "ram_start",
        "ram_len_bytes",
    )
    if "layout" not in boot or any(field not in boot for field in boot_fields):
        errors.append("boot requires layout and the four memory fields")
        numeric_boot = False
    else:
        null_count = sum(boot[field] is None for field in boot_fields)
        numeric_boot = null_count == 0
        if null_count not in (0, len(boot_fields)):
            errors.append("boot memory fields must be all concrete or all null")
        if numeric_boot:
            try:
                values = {field: as_int(boot[field]) for field in boot_fields}
            except (TypeError, ValueError):
                errors.append("boot memory fields must be integers or integer strings")
                numeric_boot = False
            else:
                expected = LAYOUT_FLASH_START.get(boot["layout"])
                if expected is not None and values["app_flash_start"] != expected:
                    errors.append(
                        f"{boot['layout']} app_flash_start should be {expected:#x}"
                    )
                if values["app_flash_len_bytes"] <= 0 or values["ram_len_bytes"] <= 0:
                    errors.append("concrete boot regions must be non-empty")
                platform = data["platform_id"]
                if platform not in PLATFORM_RAM:
                    errors.append(f"no RAM sanity window for concrete platform {platform}")
                else:
                    low, high = PLATFORM_RAM[platform]
                    start = values["ram_start"]
                    if not (
                        low <= start
                        and start + values["ram_len_bytes"] <= high
                    ):
                        errors.append(f"RAM window outside {platform} range")

    capacity = data["capacity"]
    for field in (
        "flash_budget_bytes",
        "ram_budget_bytes",
        "sample_pool_slots",
        "max_modules",
    ):
        if not isinstance(capacity.get(field), int) or capacity[field] <= 0:
            errors.append(f"capacity.{field} must be a positive integer")
    if numeric_boot:
        if capacity["flash_budget_bytes"] > values["app_flash_len_bytes"]:
            errors.append("flash budget exceeds app flash region")
        if capacity["ram_budget_bytes"] > values["ram_len_bytes"]:
            errors.append("RAM budget exceeds RAM region")

    pins = data["pins"]
    if set(pins) != {"led", "servo_pwm", "mvk_trigger"}:
        errors.append("pins must contain exactly led, servo_pwm, and mvk_trigger")
    selected_pins = []
    for field in ("led", "servo_pwm", "mvk_trigger"):
        value = pins.get(field)
        if value is not None and (
            not isinstance(value, int)
            or isinstance(value, bool)
            or not 0 <= value <= 255
        ):
            errors.append(f"pins.{field} must be null or u8")
        elif value is not None:
            selected_pins.append(value)
    if len(selected_pins) != len(set(selected_pins)):
        errors.append("selected critical pins must be distinct")

    generation = data["firmware_generation"]
    support = generation.get("support")
    if support == "unavailable":
        if not isinstance(generation.get("reason"), str) or not generation["reason"]:
            errors.append("unavailable firmware_generation needs a reason")
    elif support in {"application-image", "maintained-port", "arduino-composition"}:
        for field in ("cargo_target", "entry", "interrupts", "dma", "clock", "boot"):
            if support == "arduino-composition" and field == "cargo_target":
                continue
            if not isinstance(generation.get(field), str) or not generation[field]:
                errors.append(f"{support} generation missing {field}")
        if not numeric_boot:
            errors.append(f"{support} generation requires concrete boot memory")
        if support == "application-image":
            for field in ("linker_script", "memory_profile", "rustflags", "hal_feature"):
                if field not in generation:
                    errors.append(f"application-image generation missing {field}")
            linker = generation.get("linker_script")
            if isinstance(linker, str) and not (REPO / linker).is_file():
                errors.append(f"generation linker_script does not exist: {linker}")
            flags = generation.get("rustflags")
            if not isinstance(flags, list) or not all(
                isinstance(flag, str) for flag in flags
            ):
                errors.append("generation rustflags must be a string list")
        elif support == "maintained-port":
            manifest = generation.get("runtime_manifest")
            if not isinstance(manifest, str) or not (REPO / manifest).is_file():
                errors.append("maintained-port runtime_manifest does not exist")
        else:
            fqbn = generation.get("fqbn")
            header = generation.get("header")
            if not isinstance(fqbn, str) or not fqbn:
                errors.append("arduino-composition generation missing fqbn")
            if not isinstance(header, str) or not (REPO / header).is_file():
                errors.append("arduino-composition generation header does not exist")
    else:
        errors.append(f"unknown firmware_generation support {support!r}")
    return data, errors


def check(path: str | Path) -> list[str]:
    """Compatibility entry point used by the DTS importer."""
    return check_profile(Path(path))[1]


def main() -> int:
    paths = sorted(ROOT.glob("*/*/board.json"))
    if not paths:
        print("no board profiles found")
        return 1
    failures = 0
    board_ids: dict[str, Path] = {}
    selectors: dict[str, Path] = {}
    compositions: dict[tuple[object, ...], Path] = {}
    seen_connected: set[str] = set()
    for path in paths:
        try:
            data, errors = check_profile(path)
        except (OSError, json.JSONDecodeError, TypeError, KeyError) as error:
            data, errors = {}, [str(error)]
        relative = path.relative_to(ROOT).as_posix().removesuffix("/board.json")
        if data:
            board_id = data.get("board_id")
            if board_id in board_ids:
                errors.append(f"duplicate board_id also in {board_ids[board_id]}")
            else:
                board_ids[board_id] = path
            for selector in [data.get("feature"), *data.get("feature_aliases", [])]:
                if selector in selectors:
                    errors.append(f"feature/alias {selector} also selected by {selectors[selector]}")
                else:
                    selectors[selector] = path
            composition = data.get("composition", {})
            key = tuple(composition.get(field) for field in sorted(COMPOSITION_FIELDS))
            if key in compositions:
                errors.append(f"duplicate exact composition also in {compositions[key]}")
            else:
                compositions[key] = path
            if board_id in CONNECTED_BOARD_IDS:
                seen_connected.add(board_id)
        if errors:
            failures += 1
            print(f"[FAIL] {relative}: " + "; ".join(errors))
        else:
            print(
                f"[ OK ] {relative}: {data['board_id']} "
                f"({data['composition']['physical_board']}, {data['boot']['layout']})"
            )
    missing_connected = sorted(CONNECTED_BOARD_IDS - seen_connected)
    if missing_connected:
        failures += 1
        print("[FAIL] connected board profiles missing: " + ", ".join(missing_connected))

    generated = subprocess.run(
        [sys.executable, str(REPO / "tools/build/generate_board_catalog.py"), "--check"],
        cwd=REPO,
        check=False,
        text=True,
        capture_output=True,
    )
    if generated.returncode:
        failures += 1
        print("[FAIL] generated Rust catalog: " + (generated.stderr or generated.stdout).strip())
    else:
        print("[ OK ] " + generated.stdout.strip())
    passed = len(paths) - min(failures, len(paths))
    print(f"RESULT: {'PASS' if failures == 0 else 'FAIL'} ({passed}/{len(paths)} profiles valid)")
    return int(failures != 0)


if __name__ == "__main__":
    raise SystemExit(main())
