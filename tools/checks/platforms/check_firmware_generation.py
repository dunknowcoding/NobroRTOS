#!/usr/bin/env python3
"""Gate every public one-declaration firmware route and its fail-closed edges."""

import importlib.util
import argparse
import json
import pathlib
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
MODULE_PATH = (
    ROOT / "tools" / "cli" / "project" / "nobro_firmware_project.py"
)


def load_generator():
    spec = importlib.util.spec_from_file_location("nobro_firmware_project", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load firmware generator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--arduino-builds", action="store_true",
        help="also compile every exact Arduino route (requires pinned board cores)",
    )
    args = parser.parse_args()
    generator = load_generator()
    profiles = generator.load_board_profiles()
    image_boards = sorted(
        name for name, profile in profiles.items()
        if profile["firmware_generation"]["support"] == "application-image"
    )
    arduino_boards = sorted(
        name for name, profile in profiles.items()
        if profile["firmware_generation"]["support"] == "arduino-composition"
    )
    port_boards = sorted(
        name for name, profile in profiles.items()
        if profile["firmware_generation"]["support"] == "maintained-port"
    )
    unavailable_boards = sorted(
        name for name, profile in profiles.items()
        if profile["firmware_generation"]["support"] == "unavailable"
    )
    with tempfile.TemporaryDirectory(prefix="nobro-firmware-") as raw:
        temporary = pathlib.Path(raw)
        for board in image_boards:
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
        for board in arduino_boards:
            source = temporary / f"{board}.nobro"
            backend = (
                "backend wifi_link backend-wifi-arduino-esp8266\n"
                if board == "wemos-d1-mini" else ""
            )
            source.write_text(
                "\n".join((
                    f"app gate_{board.replace('-', '_')}",
                    f"board {board}",
                    backend.rstrip(),
                    "control control every 5ms budget 200us memory 1024/256",
                    "periodic sensor every 10ms -> control budget 300us memory 1024/256",
                    "",
                )).replace(f"board {board}\n\n", f"board {board}\n"),
                encoding="utf-8",
            )
            result = generator.generate(source, temporary / "projects")
            metadata = json.loads(
                (result["project"] / "generation.json").read_text(encoding="utf-8")
            )
            if metadata["route"] != "arduino-composition" or not metadata["fqbn"]:
                print(f"[FAIL] {board}: incomplete Arduino route")
                return 1
            if args.arduino_builds:
                completed = generator.build(result["project"])
                if completed.returncode:
                    print(f"[FAIL] {board}")
                    print("\n".join((completed.stdout + completed.stderr).splitlines()[-20:]))
                    return 1
            state = "target built" if args.arduino_builds else "route generated"
            print(f"[ OK ] {board}: {metadata['fqbn']} (Arduino {state})")

        for board in port_boards:
            source = temporary / f"plan-{board}.nobro"
            source.write_text(
                "\n".join((
                    f"app plan_{board.replace('-', '_')}",
                    f"board {board}",
                    "control control every 5ms budget 200us",
                    "",
                )),
                encoding="utf-8",
            )
            result = generator.generate(source, temporary / "projects")
            if result["route"] != "maintained-port" or result["build_available"]:
                print(f"[FAIL] {board}: maintained-port route is not fail closed")
                return 1
            if generator.build(result["project"]).returncode == 0:
                print(f"[FAIL] {board}: plan-only route reported an application build")
                return 1
            print(f"[ OK ] {board}: maintained startup plan (application build unavailable)")

        for board in unavailable_boards:
            source = temporary / f"unavailable-{board}.nobro"
            source.write_text(
                f"app unavailable_{board.replace('-', '_')}\nboard {board}\n"
                "control control every 5ms budget 200us\n",
                encoding="utf-8",
            )
            try:
                generator.generate(source, temporary / "projects")
            except ValueError as error:
                if "no generated firmware route" not in str(error):
                    print(f"[FAIL] {board}: unclear unavailable diagnostic: {error}")
                    return 1
            else:
                print(f"[FAIL] {board}: unavailable route generated an image")
                return 1

    routed = len(image_boards) + len(arduino_boards) + len(port_boards)
    print(
        "PORTABLE FIRMWARE GENERATION: PASS "
        f"({len(image_boards)} images, {len(arduino_boards)} Arduino routes, "
        f"{len(port_boards)} maintained plans; {routed} maintained profiles)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
