#!/usr/bin/env python3
"""Gate the shared task/wire authoring contract across public surfaces."""

from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bindings" / "python"))
sys.path.insert(0, str(ROOT / "tools"))

from nobro_rtos import AppDeclarationError, HZ, NobroApp  # noqa: E402
import nobro_app  # noqa: E402


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def fails(callable_, text: str) -> None:
    try:
        callable_()
    except AppDeclarationError as error:
        require(text in str(error), f"expected {text!r}, got {error!r}")
        return
    raise AssertionError(f"expected AppDeclarationError containing {text!r}")


def main() -> int:
    contract = json.loads(
        (ROOT / "sdk" / "app-authoring-contract.json").read_text(encoding="utf-8")
    )
    require(
        contract["schema"] == "nobro-app-authoring-contract-v1",
        "authoring contract schema drift",
    )
    require(contract["document_schema"] == "nobro-app-v1", "document schema drift")
    require(contract["defaults"]["role"] == "periodic", "default role drift")
    require(contract["defaults"]["wire_capacity"] == 1, "wire default drift")
    require(contract["limits"]["wire_capacity_max"] == 64, "wire limit drift")

    app = (
        NobroApp("parity")
        .task("imu", HZ(100))
        .task("control", HZ(50), role="control")
        .wire("imu", "control")
    )
    periodic, control = app.tasks
    require(periodic.role == "periodic", "Python default role drift")
    require(periodic.budget_us == periodic.period_us // 10, "periodic budget drift")
    require(control.budget_us == control.period_us // 10, "control budget drift")
    require(periodic.flash_bytes == control.flash_bytes == 1024, "flash default drift")
    require(periodic.ram_bytes == control.ram_bytes == 256, "RAM default drift")
    require(app.wires[0].capacity == 1, "Python wire default drift")
    require(
        NobroApp("alias").task("imu", 1000, role="sensor").tasks[0].role
        == "periodic",
        "sensor compatibility alias drift",
    )

    full = NobroApp("full")
    for index in range(8):
        full.task(f"task{index}", 1000)
    fails(lambda: full.task("task0", 1000), "duplicate task")
    fails(lambda: NobroApp("period").task("task", 0), "period_us")
    fails(
        lambda: NobroApp("endpoint").task("task", 1000).wire("task", "missing"),
        "unknown task",
    )
    fails(
        lambda: (
            NobroApp("duplicate")
            .task("left", 1000)
            .task("right", 1000)
            .wire("left", "right")
            .wire("left", "right")
        ),
        "duplicate wire",
    )
    fails(
        lambda: NobroApp("capacity").task("task", 1000).wire("task", "task", 65),
        "between 1 and 64",
    )

    fixture_path = ROOT / "tutorials" / "hello-device" / "app.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    require(not nobro_app.validate(fixture), "canonical block fixture rejected")
    rust = nobro_app.generate_rust(NobroApp.from_dict(fixture))
    require("TaskDecl::periodic" in rust and ".wire(" in rust, "Rust graph vocabulary drift")

    surfaces = {
        "Rust": ROOT / "core" / "crates" / "nobro_kernel" / "src" / "graph.rs",
        "C": ROOT / "bindings" / "c" / "include" / "nobro_app.h",
        "C++": ROOT / "bindings" / "cpp" / "include" / "nobro_app.hpp",
        "Arduino": ROOT / "packages" / "arduino" / "src" / "NobroRTOS.h",
        "Python": ROOT / "bindings" / "python" / "nobro_rtos" / "app.py",
        "blocks": ROOT / "packages" / "block-editor" / "app.js",
    }
    required_tokens = {
        "Rust": ["pub const fn hz(", "pub fn wire", "pub fn channel"],
        "C": ["nobro_task(", "nobro_wire("],
        "C++": ["inline int32_t task(", "inline int32_t wire("],
        "Arduino": ["uint32_t hz(", "TaskId task(", "NobroApp &wire("],
        "Python": ["def task(", "def wire("],
        "blocks": ['schema: "nobro-app-v1"', "tasks:", "wires:"],
    }
    for name, path in surfaces.items():
        text = path.read_text(encoding="utf-8")
        for token in required_tokens[name]:
            require(token in text, f"{name} missing {token}")

    compiler = shutil.which("g++") or shutil.which("g++.exe")
    require(compiler is not None, "g++ is required for C++11 authoring parity")
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "authoring.cpp"
        output = Path(directory) / "authoring.o"
        source.write_text(
            '#include "nobro_app.hpp"\n'
            "static int32_t step() { return 0; }\n"
            "int configure() {\n"
            '  if (nobro::task("imu", HZ(100), step) != NOBRO_OK) return 1;\n'
            '  if (nobro::wire("imu", "control", 8) != NOBRO_OK) return 2;\n'
            "  return nobro::run();\n"
            "}\n",
            encoding="utf-8",
        )
        compiled = subprocess.run(
            [
                compiler,
                "-std=c++11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
                str(ROOT / "bindings" / "cpp" / "include"),
                "-I",
                str(ROOT / "bindings" / "c" / "include"),
                "-c",
                str(source),
                "-o",
                str(output),
            ],
            capture_output=True,
            text=True,
        )
        require(
            compiled.returncode == 0,
            "C++11 task/wire wrapper failed:\n" + compiled.stderr[-2000:],
        )

    manifest = json.loads((ROOT / "sdk" / "sdk-manifest.json").read_text(encoding="utf-8"))
    require(
        manifest.get("app_authoring_contract") == "sdk/app-authoring-contract.json",
        "SDK does not publish the authoring contract",
    )
    print("APP AUTHORING PARITY: PASS (task/wire nouns, defaults, aliases, ordered negatives)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
