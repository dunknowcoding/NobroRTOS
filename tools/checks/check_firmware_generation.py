#!/usr/bin/env python3
"""Build board-owned standalone firmware contracts for every promoted Cortex-M layout."""

import importlib.util
import pathlib
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "cli" / "nobro_firmware_project.py"


def load_generator():
    spec = importlib.util.spec_from_file_location("nobro_firmware_project", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load firmware generator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    generator = load_generator()
    boards = ("nrf52840-nosd", "nrf52840-s140", "uno-r4-wifi", "samd21-uf2")
    with tempfile.TemporaryDirectory(prefix="nobro-firmware-") as raw:
        temporary = pathlib.Path(raw)
        for board in boards:
            source = temporary / f"{board}.nobro"
            source.write_text(
                "\n".join(
                    (
                        f"app gate_{board.replace('-', '_')}",
                        f"board {board}",
                        "control control every 5ms budget 200us",
                        "periodic sensor every 10ms -> control budget 300us",
                        "",
                    )
                ),
                encoding="utf-8",
            )
            result = generator.generate(source, temporary / "projects")
            completed = generator.build(result["project"])
            if completed.returncode:
                tail = (completed.stdout + completed.stderr).splitlines()[-20:]
                print(f"[FAIL] {board}")
                print("\n".join(tail))
                return 1
            print(
                f"[ OK ] {board}: {result['cargo_target']} "
                f"({result['generation_contract']['entry']})"
            )
    print(f"PORTABLE FIRMWARE GENERATION: PASS ({len(boards)}/{len(boards)} target builds)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
