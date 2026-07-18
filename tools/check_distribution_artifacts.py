#!/usr/bin/env python3
"""Build and clean-install smoke the three public package surfaces.

This is a local/CI readiness gate, not a registry publisher. It creates
temporary archives from tracked package files, rejects private or generated
leakage, compiles the Arduino and PlatformIO headers outside the repository,
and installs the Python package into an isolated target directory.
"""

from __future__ import annotations

import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
ARDUINO = ROOT / "packages" / "arduino"
PLATFORMIO = ROOT / "packages" / "platformio"
PYTHON = ROOT / "bindings" / "python"
FORBIDDEN_PARTS = {
    "_maintainer",
    "_work",
    "__pycache__",
    ".pytest_cache",
    ".git",
    "target",
    "dist",
    "build",
}
FORBIDDEN_SUFFIXES = {
    ".pyc",
    ".pyo",
    ".elf",
    ".hex",
    ".map",
    ".obj",
    ".o",
    ".pdb",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def tracked_under(root: Path) -> tuple[Path, ...]:
    relative = root.relative_to(ROOT).as_posix()
    result = subprocess.run(
        ["git", "ls-files", "-z", "--", relative],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    paths = []
    for raw in result.stdout.split(b"\0"):
        if raw:
            paths.append(ROOT / raw.decode("utf-8"))
    require(paths, f"no tracked package files under {relative}")
    return tuple(paths)


def safe_member(path: PurePosixPath) -> None:
    require(not path.is_absolute(), f"absolute archive member: {path}")
    require(".." not in path.parts, f"escaping archive member: {path}")
    require(not (set(path.parts) & FORBIDDEN_PARTS), f"private/cache member: {path}")
    require(path.suffix.lower() not in FORBIDDEN_SUFFIXES, f"build member: {path}")


def package_files(root: Path) -> tuple[Path, ...]:
    files = tracked_under(root)
    for path in files:
        relative = PurePosixPath(path.relative_to(root).as_posix())
        safe_member(relative)
        require(path.is_file() and not path.is_symlink(), f"not a regular file: {path}")
    return files


def build_arduino(archive: Path) -> None:
    files = package_files(ARDUINO)
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as handle:
        for path in files:
            member = PurePosixPath("NobroRTOS") / path.relative_to(ARDUINO).as_posix()
            safe_member(member)
            handle.write(path, member.as_posix())
    with zipfile.ZipFile(archive) as handle:
        members = {PurePosixPath(name) for name in handle.namelist()}
    for required in (
        PurePosixPath("NobroRTOS/library.properties"),
        PurePosixPath("NobroRTOS/LICENSE"),
        PurePosixPath("NobroRTOS/src/NobroRTOS.h"),
        PurePosixPath("NobroRTOS/src/nobro_rtos.h"),
    ):
        require(required in members, f"Arduino archive missing {required}")


def build_platformio(archive: Path) -> None:
    files = package_files(PLATFORMIO)
    with tarfile.open(archive, "w:gz") as handle:
        for path in files:
            member = PurePosixPath("NobroRTOS") / path.relative_to(PLATFORMIO).as_posix()
            safe_member(member)
            handle.add(path, arcname=member.as_posix(), recursive=False)
    with tarfile.open(archive) as handle:
        members = {PurePosixPath(item.name) for item in handle.getmembers()}
    for required in (
        PurePosixPath("NobroRTOS/library.json"),
        PurePosixPath("NobroRTOS/LICENSE"),
        PurePosixPath("NobroRTOS/include/NobroRTOS.h"),
        PurePosixPath("NobroRTOS/include/nobro_rtos.h"),
    ):
        require(required in members, f"PlatformIO archive missing {required}")


def compiler() -> str:
    found = shutil.which("g++") or shutil.which("g++.exe")
    require(found is not None, "g++ is required for distribution header smoke")
    return found


def compile_headers(work: Path, arduino_zip: Path, platformio_tar: Path) -> None:
    arduino_root = work / "arduino"
    platformio_root = work / "platformio"
    with zipfile.ZipFile(arduino_zip) as handle:
        handle.extractall(arduino_root)
    with tarfile.open(platformio_tar) as handle:
        for member in handle.getmembers():
            safe_member(PurePosixPath(member.name))
            require(member.isfile(), f"unexpected PlatformIO archive member: {member.name}")
            handle.extract(member, platformio_root)

    source = work / "smoke.cpp"
    executable_suffix = ".exe" if os.name == "nt" else ""
    arduino_executable = work / f"arduino-smoke{executable_suffix}"
    platformio_executable = work / f"platformio-smoke{executable_suffix}"
    source.write_text(
        "#include <NobroRTOS.h>\n"
        "int main() {\n"
        "  nobro::NobroApp<3, 1> app;\n"
        '  auto motor = app.task("motor", nobro::hz(200), nobro::CONTROL);\n'
        '  auto imu = app.task("imu", nobro::hz(100));\n'
        "  app.wire(imu, motor);\n"
        "  return app.admit() ? 0 : 1;\n"
        "}\n",
        encoding="utf-8",
    )
    subprocess.run(
        [
            compiler(),
            "-std=c++11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            str(arduino_root / "NobroRTOS" / "src"),
            str(source),
            "-o",
            str(arduino_executable),
        ],
        check=True,
    )
    subprocess.run([str(arduino_executable)], check=True)

    source.write_text(
        "#include <NobroRTOS.h>\n"
        "int main() { return NOBRO_REPORT_STATUS_PASS == 3 ? 0 : 1; }\n",
        encoding="utf-8",
    )
    subprocess.run(
        [
            compiler(),
            "-std=c++11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            str(platformio_root / "NobroRTOS" / "include"),
            str(source),
            "-o",
            str(platformio_executable),
        ],
        check=True,
    )
    subprocess.run([str(platformio_executable)], check=True)


def install_python(work: Path) -> None:
    target = work / "python-source"
    env = dict(os.environ)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    target.mkdir()
    for name in ("pyproject.toml", "README.md", "LICENSE"):
        shutil.copy2(PYTHON / name, target / name)
    package_root = PYTHON / "nobro_rtos"
    for source in tracked_under(package_root):
        relative = source.relative_to(PYTHON)
        destination = target / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    for path in target.rglob("*"):
        if path.is_file():
            safe_member(PurePosixPath(path.relative_to(target).as_posix()))
    smoke = (
        "import json, nobro_rtos; "
        "from nobro_rtos import HZ, NobroApp; "
        "a=NobroApp('smoke').task('motor', HZ(200), role='control'); "
        "print(json.dumps({'tasks': len(a.tasks)}))"
    )
    result = subprocess.run(
        [sys.executable, "-c", smoke],
        check=True,
        env={**env, "PYTHONPATH": str(target)},
        cwd=work,
        capture_output=True,
        text=True,
    )
    require(json.loads(result.stdout)["tasks"] == 1, "Python clean-install smoke drift")
    subprocess.run(
        [sys.executable, "-m", "nobro_rtos", "--help"],
        check=True,
        env={**env, "PYTHONPATH": str(target)},
        cwd=work,
        capture_output=True,
        text=True,
    )
    doctor = subprocess.run(
        [sys.executable, "-m", "nobro_rtos", "doctor"],
        check=True,
        env={**env, "PYTHONPATH": str(target)},
        cwd=work,
        capture_output=True,
        text=True,
    )
    doctor_report = json.loads(doctor.stdout)
    require(doctor_report["status"] == "ok", "installed Python doctor failed")
    require(doctor_report["mode"] == "installed", "installed Python doctor mode drift")


def main() -> int:
    license_text = (ROOT / "LICENSE").read_text(encoding="utf-8")
    for package in (ARDUINO, PLATFORMIO, PYTHON):
        require(
            (package / "LICENSE").read_text(encoding="utf-8") == license_text,
            f"{package.relative_to(ROOT)} license drift",
        )
    platformio_header = (PLATFORMIO / "include" / "NobroRTOS.h").read_text(
        encoding="utf-8"
    )
    require("../" not in platformio_header, "PlatformIO header escapes package root")

    with tempfile.TemporaryDirectory(prefix="nobro-distribution-") as directory:
        work = Path(directory)
        arduino_zip = work / "NobroRTOS-arduino.zip"
        platformio_tar = work / "NobroRTOS-platformio.tar.gz"
        build_arduino(arduino_zip)
        build_platformio(platformio_tar)
        compile_headers(work, arduino_zip, platformio_tar)
        install_python(work)

    print("DISTRIBUTION ARTIFACTS: PASS (tracked, licensed, self-contained, clean-install)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
