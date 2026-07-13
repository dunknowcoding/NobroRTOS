#!/usr/bin/env python3
"""Best-effort DeviceTree (.dts) -> NobroRTOS board.json importer (the Zephyr/DTS on-ramp).

Zephyr/Linux describe a board in DeviceTree; NobroRTOS boards are data-first board.json.
This importer parses the DTS subset that maps cleanly - compatible, model, the code/app
flash partition, the SRAM region, and the status LED gpio - and emits a board.json that
passes tools/check_board_profiles.py. Everything DeviceTree does NOT carry (NobroRTOS
software budgets, the cargo feature name, servo/trigger pins) is filled with reviewable
defaults and listed under "_review" so nothing is silently invented.

  python tools/import_dts.py board.dts --out core/boards/vendor/mine/board.json
  python tools/import_dts.py --selftest

This is a best-effort on-ramp, not a full DTS compiler: no phandle resolution beyond
labels, no overlays/includes. Review the emitted board.json before trusting it.
"""
import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_board_profiles as cbp  # reuse the real validator  # noqa: E402

PLATFORMS = ["nrf52840", "esp32c3", "rp2350", "samd21", "stm32f4", "imxrt1062"]
# app_flash_start -> boot layout (inverse of the validator's table).
START_TO_LAYOUT = {v: k for k, v in cbp.LAYOUT_FLASH_START.items()}
APP_PARTITION_LABELS = {"code", "app", "mcuboot", "image-0", "slot0", "factory",
                        "code-partition", "code_partition"}


class Node:
    __slots__ = ("name", "props", "children")

    def __init__(self, name):
        self.name = name
        self.props = {}
        self.children = []

    def walk(self):
        yield self
        for c in self.children:
            yield from c.walk()


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    text = re.sub(r"//[^\n]*", " ", text)
    return "\n".join(ln for ln in text.splitlines() if not ln.lstrip().startswith("#"))


def parse_dts(text):
    """Parse the DTS into a tree of Nodes. Handles nested nodes, `name = value;`
    properties, and boolean properties; good enough for the mappable subset."""
    root = Node("/")
    stack, buf = [root], ""
    for ch in strip_comments(text):
        if ch == "{":
            header = buf.strip().split(":")[-1].strip()  # drop optional label
            node = Node(header)
            stack[-1].children.append(node)
            stack.append(node)
            buf = ""
        elif ch == "}":
            buf = ""
        elif ch == ";":
            stmt = buf.strip()
            buf = ""
            if not stmt or stmt in ("/dts-v1/",):
                continue
            if len(stack) > 1 and stmt == "":
                continue
            if "=" in stmt:
                key, val = stmt.split("=", 1)
                stack[-1].props[key.strip()] = val.strip()
            else:
                stack[-1].props[stmt] = True   # boolean property
        else:
            buf += ch
    return root


def _cells(value):
    """Ints from a `<a b c>` cell list (handles hex/dec)."""
    m = re.search(r"<(.*?)>", value, re.S)
    if not m:
        return []
    out = []
    for tok in m.group(1).replace(",", " ").split():
        tok = tok.strip()
        if re.fullmatch(r"0x[0-9A-Fa-f]+|\d+", tok):
            out.append(int(tok, 0))
    return out


def _strings(value):
    return re.findall(r'"([^"]*)"', value)


def import_board(text):
    root = parse_dts(text)
    review = []
    platform, model, compat_all = None, None, []
    flash_start = flash_len = ram_start = ram_len = None
    led = None

    for node in root.walk():
        props = node.props
        if "compatible" in props:
            compat_all += _strings(props["compatible"])
        if "model" in props and model is None:
            strs = _strings(props["model"])
            model = strs[0] if strs else None
        # SRAM region: memory@2.. or a node named sram*
        if (node.name.startswith("memory@") or "sram" in node.name.lower()) and "reg" in props:
            cells = _cells(props["reg"])
            if len(cells) >= 2 and ram_start is None:
                ram_start, ram_len = cells[0], cells[1]
        # Flash app/code partition
        labels = set(_strings(props.get("label", ""))) | {node.name.split("@")[0]}
        if "reg" in props and (labels & APP_PARTITION_LABELS):
            cells = _cells(props["reg"])
            if len(cells) >= 2:
                flash_start, flash_len = cells[0], cells[1]
        # Status LED gpio: leds { ... gpios = <&gpioX N flags>; }
        if led is None and "gpios" in props and "led" in node.name.lower():
            cells = _cells(props["gpios"])
            if cells:
                led = cells[0]

    for c in compat_all:
        chip = c.split(",")[-1]
        for p in PLATFORMS:
            if p in chip:
                platform = p
                break
        if platform:
            break

    if platform is None:
        review.append("platform_id: no known compatible matched - set manually")
        platform = "cortex_m"
    if flash_start is None:
        review.append("boot.app_flash_start/len: no app/code partition reg found - "
                      "defaulted, set from your flash map")
        flash_start, flash_len = cbp.LAYOUT_FLASH_START.get("NoSoftDevice", 0x1000), 0x40000
    if ram_start is None:
        review.append("boot.ram_start/len: no memory/sram reg found - defaulted")
        lo, hi = cbp.PLATFORM_RAM.get(platform, (0x2000_0000, 0x2000_8000))
        ram_start, ram_len = lo, hi - lo
    if led is None:
        review.append("pins.led: no led gpio found - defaulted to 0")
        led = 0

    layout = START_TO_LAYOUT.get(flash_start, "Custom")
    if layout == "Custom":
        review.append(f"boot.layout: app_flash_start {flash_start:#x} matches no known "
                      "layout - set layout manually")
    review.append("feature: cargo feature name is not in DTS - set the board-* feature")
    review.append("capacity.*: NobroRTOS software budgets are not in DTS - tune them")
    review.append("pins.servo_pwm / pins.mvk_trigger: not derived from DTS - set if used")

    board = {
        "board_id": (model or f"{platform}_imported").lower().replace(" ", "_"),
        "platform_id": platform,
        "feature": f"board-{platform}-imported",
        "description": (model or "imported from DeviceTree") + " (DTS import - review)",
        "boot": {
            "layout": layout,
            "app_flash_start": hex(flash_start),
            "app_flash_len_bytes": flash_len,
            "ram_start": hex(ram_start),
            "ram_len_bytes": ram_len,
        },
        "capacity": {
            "flash_budget_bytes": min(81920, flash_len),
            "ram_budget_bytes": min(32768, ram_len),
            "sample_pool_slots": 8,
            "max_modules": 16,
        },
        "pins": {"led": led, "servo_pwm": 0, "mvk_trigger": 0},
        "_review": review,
    }
    return board


SAMPLE_DTS = """\
/dts-v1/;
/ {
    model = "Generic nRF52840 DevBoard";
    compatible = "acme,nrf52840-dev", "nordic,nrf52840";
    chosen { zephyr,code-partition = &code_partition; };
    sram0: memory@20000000 { reg = <0x20000000 0x40000>; };
    leds {
        led_0: led_0 { gpios = <&gpio0 15 0>; label = "green"; };
    };
    flash0: flash@0 {
        partitions {
            code_partition: partition@26000 {
                label = "code";
                reg = <0x26000 0xC2000>;
            };
        };
    };
};
"""


def _validate(board):
    import tempfile
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as f:
        json.dump(board, f)
        path = f.name
    try:
        return cbp.check(path)
    finally:
        os.unlink(path)


def selftest():
    board = import_board(SAMPLE_DTS)
    errs = _validate(board)
    ok = (board["platform_id"] == "nrf52840"
          and board["boot"]["app_flash_start"] == "0x26000"
          and board["boot"]["layout"] == "SoftDeviceS140V6"
          and board["boot"]["ram_len_bytes"] == 0x40000
          and board["pins"]["led"] == 15
          and not errs)
    print(f"platform_id : {board['platform_id']}")
    print(f"boot        : {board['boot']['layout']} app@{board['boot']['app_flash_start']} "
          f"ram {board['boot']['ram_start']}+{board['boot']['ram_len_bytes']:#x}")
    print(f"led pin     : {board['pins']['led']}")
    print(f"validator   : {'clean' if not errs else errs}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description="DeviceTree .dts -> board.json (best-effort).")
    ap.add_argument("dts", nargs="?", help="path to a .dts file")
    ap.add_argument("--out", help="write board.json here (default: stdout)")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest or not args.dts:
        return selftest()

    board = import_board(open(args.dts, encoding="utf-8").read())
    errs = _validate(board)
    text = json.dumps(board, indent=2)
    if args.out:
        os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
        with open(args.out, "w", newline="\n", encoding="utf-8") as f:
            f.write(text + "\n")
        print(f"wrote {args.out}")
    else:
        print(text)
    for item in board["_review"]:
        print(f"  REVIEW: {item}", file=sys.stderr)
    if errs:
        print(f"  validator errors: {errs}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
