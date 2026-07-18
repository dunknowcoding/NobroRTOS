#!/usr/bin/env python3
"""Validate that the tracked tree contains only the public product surface."""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import unicodedata


ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "sdk" / "sdk-manifest.json"
LOCAL_NEEDLES = ROOT / "tools" / "leak_needles.local.txt"

PUBLIC_TOP_LEVEL_FILES = {
    ".gitignore",
    "CHANGELOG.md",
    "LICENSE",
    "README.md",
}
PUBLIC_TOP_LEVEL_DIRECTORIES = {
    ".github",
    "bindings",
    "core",
    "docs",
    "host",
    "packages",
    "sdk",
    "tools",
    "tutorials",
}
EXPECTED_CORE_ROOTS = frozenset(
    pathlib.PurePosixPath(item)
    for item in (
        "core/.cargo",
        "core/Cargo.lock",
        "core/Cargo.toml",
        "core/README.md",
        "core/adapters",
        "core/apps",
        "core/boards",
        "core/crates",
        "core/layout.json",
        "core/memory-nosd.x",
        "core/memory-s140.x",
        "core/ports",
        "core/vendor",
    )
)
EXPECTED_RELEASE_EXCLUDES = frozenset(
    pathlib.PurePosixPath(item)
    for item in ("_work", "core/_work", "core/target")
)
EXPECTED_HOST_TOOLS = frozenset(
    pathlib.PurePosixPath(item)
    for item in (
        "tools/nobro_contract_tool.py",
        "tools/nobro_project.py",
        "tools/nobro_shrink.py",
    )
)
EXPECTED_INCLUDE_ROOTS = frozenset(
    pathlib.PurePosixPath(item)
    for item in ("bindings/c/include", "bindings/cpp/include")
)
EXPECTED_PACKAGE_SURFACES = {
    "arduino": "packages/arduino",
    "platformio": "packages/platformio",
    "block_editor": "packages/block-editor",
    "web_flasher": "packages/web-flasher",
}
EXPECTED_PACKAGE_ROOTS = frozenset(
    pathlib.PurePosixPath(item) for item in EXPECTED_PACKAGE_SURFACES.values()
)
EXPECTED_BINDING_ROOTS = frozenset(
    pathlib.PurePosixPath(item)
    for item in ("bindings/c", "bindings/cpp", "bindings/python")
)
EXPECTED_PYTHON_PACKAGE = pathlib.PurePosixPath("bindings/python")
EXPECTED_DISTRIBUTION_POLICY = {
    "source": "tracked_files",
    "allow_untracked": False,
    "allow_symlinks": False,
}
REGULAR_FILE_MODES = {"100644", "100755"}
SDK_PUBLIC_CHILDREN = {
    "README.md",
    "cli",
    "feature-catalog.json",
    "firmware",
    "include",
    "python",
    "sdk-manifest.json",
}
HOST_PUBLIC_CHILDREN = {"README.md", "nobro-host-contract.json"}
BINARY_SUFFIXES = {
    ".bin",
    ".elf",
    ".gif",
    ".ico",
    ".jpeg",
    ".jpg",
    ".pdf",
    ".png",
    ".ttf",
    ".uf2",
    ".webp",
    ".woff",
    ".woff2",
    ".zip",
}
WINDOWS_RESERVED_BASENAMES = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CLOCK$",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}

PRIVACY_PATTERNS = (
    (
        "local serial endpoint",
        re.compile(r"(?<![A-Za-z0-9])COM[0-9]+(?![A-Za-z0-9])", re.IGNORECASE),
    ),
    (
        "local board alias",
        re.compile(
            r"(?<![A-Za-z0-9])board[1-9][0-9]*(?![A-Za-z0-9])",
            re.IGNORECASE,
        ),
    ),
    (
        "planning wave tag",
        re.compile(
            r"(?<![A-Za-z0-9])Wave(?:[_ -]?[1-9][0-9]*)(?![A-Za-z0-9])",
            re.IGNORECASE,
        ),
    ),
    (
        "explicit planning milestone tag",
        re.compile(
            r"(?<![A-Za-z0-9])milestone(?:[_ -]+)M[0-9]+(?![A-Za-z0-9])",
            re.IGNORECASE,
        ),
    ),
    (
        "host-specific absolute path",
        re.compile(
            r"(?<![A-Za-z0-9])(?:"
            r"[A-Za-z]:[\\/]"
            r"|[A-Za-z]:[^\s'\"`<>|]*\\[^\s'\"`<>|]*"
            r"|\\\\(?:[?.]\\|[A-Za-z0-9][A-Za-z0-9_.-]*\\)"
            r")"
        ),
    ),
    (
        "non-public document reference",
        re.compile(
            r"(?<![A-Za-z0-9_.-])[A-Za-z0-9_.-]*_"
            r"(?:INTERNAL|PRIVATE)(?:\.[A-Za-z0-9_.-]+)?",
            re.IGNORECASE,
        ),
    ),
)
NON_PUBLIC_FILENAME = re.compile(
    r"(?:_(?:INTERNAL|PRIVATE)(?:\.|$)|\.local\.)", re.IGNORECASE
)
TOOL_REFERENCE = re.compile(
    r"(?<![A-Za-z0-9_.-])(tools[\\/][A-Za-z0-9_.\\/-]+\.py)\b",
    re.IGNORECASE,
)
GENERATED_TOOL_DECLARATION = re.compile(
    r"TemplateFile\(\s*['\"](tools[\\/][A-Za-z0-9_.\\/-]+\.py)['\"]"
)
COMPARISON_TOOL = re.compile(r"tools/measure_[A-Za-z0-9_.-]+\.py", re.IGNORECASE)
GENERATED_TOOL_TEMPLATE = "bindings/python/nobro_rtos/templates.py"


def _is_portable_repo_path(value: str) -> bool:
    if not value or value != unicodedata.normalize("NFC", value):
        return False
    if "\\" in value or ":" in value:
        return False
    if any(ord(character) < 32 or ord(character) == 127 for character in value):
        return False
    path = pathlib.PurePosixPath(value)
    if path.is_absolute() or path.as_posix() != value or not path.parts:
        return False
    if any(part in ("", ".", "..") for part in path.parts):
        return False
    for part in path.parts:
        if part.endswith((" ", ".")):
            return False
        basename = part.split(".", 1)[0].upper()
        if basename in WINDOWS_RESERVED_BASENAMES:
            return False
    return True


def _tracked_entries() -> tuple[dict[str, tuple[str, str]], list[str]]:
    result = subprocess.run(
        ["git", "ls-files", "--stage", "-z"],
        cwd=ROOT,
        capture_output=True,
        check=True,
    )
    entries: dict[str, tuple[str, str]] = {}
    errors: list[str] = []
    for raw in result.stdout.split(b"\0"):
        if not raw:
            continue
        try:
            metadata, encoded_path = raw.split(b"\t", 1)
            mode, _object_id, stage = metadata.decode("ascii").split()
            path = encoded_path.decode("utf-8")
        except (UnicodeDecodeError, ValueError):
            errors.append("tracked entry has a malformed or non-UTF-8 path")
            continue
        if path in entries:
            errors.append("tracked path has multiple index stages")
        entries[path] = (mode, stage)
    return entries, errors


def _local_needles() -> list[str]:
    if not LOCAL_NEEDLES.is_file():
        return []
    return [
        line.strip()
        for line in LOCAL_NEEDLES.read_text(encoding="utf-8-sig").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]


def _overlaps(left: pathlib.PurePosixPath, right: pathlib.PurePosixPath) -> bool:
    return left == right or left in right.parents or right in left.parents


def _is_within(path: pathlib.PurePosixPath, root: pathlib.PurePosixPath) -> bool:
    return path == root or root in path.parents


def _has_tracked_content(relative: pathlib.PurePosixPath, tracked: set[str]) -> bool:
    value = relative.as_posix()
    prefix = value.rstrip("/") + "/"
    return value in tracked or any(item.startswith(prefix) for item in tracked)


def _privacy_hits(text: str) -> set[str]:
    return {label for label, pattern in PRIVACY_PATTERNS if pattern.search(text)}


def _path_has_non_public_marker(path: pathlib.PurePosixPath) -> bool:
    return any(
        part.casefold() in {"internal", "private"} or NON_PUBLIC_FILENAME.search(part)
        for part in path.parts
    )


def _parse_manifest_paths(
    value: object, label: str, errors: list[str]
) -> set[pathlib.PurePosixPath]:
    if type(value) is not list:
        errors.append(f"manifest {label} must be a path list")
        return set()
    paths: set[pathlib.PurePosixPath] = set()
    folded: set[str] = set()
    for item in value:
        if not isinstance(item, str) or not _is_portable_repo_path(item):
            errors.append(f"manifest {label} contains a noncanonical path")
            continue
        path = pathlib.PurePosixPath(item)
        key = unicodedata.normalize("NFC", item).casefold()
        if path in paths or key in folded:
            errors.append(f"manifest {label} contains duplicate paths")
        paths.add(path)
        folded.add(key)
    for path in paths:
        if any(path != other and _is_within(path, other) for other in paths):
            errors.append(f"manifest {label} contains redundant parent paths")
            break
    return paths


def _validate_manifest(manifest: object) -> list[str]:
    errors: list[str] = []
    if not isinstance(manifest, dict):
        return ["SDK manifest must be an object"]

    core_roots = _parse_manifest_paths(
        manifest.get("core_distribution_roots"), "core roots", errors
    )
    release_excludes = _parse_manifest_paths(
        manifest.get("release_excludes"), "release excludes", errors
    )
    host_tools = _parse_manifest_paths(manifest.get("host_tools"), "host tools", errors)
    include_roots = _parse_manifest_paths(
        manifest.get("include_roots"), "include roots", errors
    )
    if core_roots != EXPECTED_CORE_ROOTS:
        errors.append("manifest core roots differ from the public distribution contract")
    if release_excludes != EXPECTED_RELEASE_EXCLUDES:
        errors.append("manifest release excludes must contain only generated outputs")
    if host_tools != EXPECTED_HOST_TOOLS:
        errors.append("manifest host tools differ from the public command contract")
    if include_roots != EXPECTED_INCLUDE_ROOTS:
        errors.append("manifest include roots differ from the public binding contract")

    package_surfaces = manifest.get("package_surfaces")
    if package_surfaces != EXPECTED_PACKAGE_SURFACES:
        errors.append("manifest package surfaces differ from the public package contract")
    elif any(
        not _is_portable_repo_path(value) for value in package_surfaces.values()
    ):
        errors.append("manifest package surfaces contain a noncanonical path")

    python_package = manifest.get("python_package")
    if python_package != EXPECTED_PYTHON_PACKAGE.as_posix():
        errors.append("manifest Python package differs from the public binding contract")
    elif not _is_portable_repo_path(python_package):
        errors.append("manifest Python package path is noncanonical")

    fixed_paths = {
        "canonical contract": (
            manifest.get("canonical_contract"),
            "host/nobro-host-contract.json",
        ),
        "feature catalog": (
            manifest.get("feature_catalog"),
            "sdk/feature-catalog.json",
        ),
        "core workspace": (manifest.get("core_workspace"), "core"),
    }
    for label, (actual, expected) in fixed_paths.items():
        if (
            actual != expected
            or not isinstance(actual, str)
            or not _is_portable_repo_path(actual)
        ):
            errors.append(f"manifest {label} differs from the public contract")
    generated = manifest.get("generated_output_policy")
    if not isinstance(generated, dict) or generated.get("repo_relative_work_dir") != "_work":
        errors.append("manifest generated-output workspace differs from the public contract")
    if manifest.get("distribution_policy") != EXPECTED_DISTRIBUTION_POLICY:
        errors.append("manifest distribution policy must use regular tracked files only")
    return errors


def _workflow_uses_unsupported_toolchain(workflow: str) -> bool:
    jobs: list[str] = []
    current: list[str] | None = None
    in_jobs = False
    for line in workflow.splitlines():
        if line.strip() == "jobs:" and not line.startswith(" "):
            in_jobs = True
            continue
        if not in_jobs:
            continue
        if re.fullmatch(r"  [A-Za-z0-9_-]+:\s*(?:#.*)?", line):
            if current is not None:
                jobs.append("\n".join(current))
            current = [line]
        elif current is not None:
            current.append(line)
    if current is not None:
        jobs.append("\n".join(current))

    command = "arduino-cli core install arduinonrf:nrf52"
    for job in jobs:
        if command not in job:
            continue
        if not re.search(
            r"(?mi)^\s{4}runs-on:\s*['\"]?windows(?:-[A-Za-z0-9_.-]+)?['\"]?\s*$",
            job,
        ):
            return True
    return False


def _safe_display_path(relative: str, folded_needles: list[str]) -> str:
    portable = _is_portable_repo_path(relative)
    sensitive = not portable or any(
        needle in relative.casefold() for needle in folded_needles
    )
    if portable:
        path = pathlib.PurePosixPath(relative)
        sensitive = sensitive or _path_has_non_public_marker(path) or bool(
            _privacy_hits(relative)
        )
    return "<redacted tracked path>" if sensitive else relative


def _policy_selftest() -> list[str]:
    errors: list[str] = []
    cases = {
        "local serial endpoint": "COM" + "17",
        "local board alias": "_" + "board" + "7",
        "planning wave tag": "Wave" + " 17",
        "explicit planning milestone tag": "milestone" + " M17",
        "host-specific absolute path": "C" + ":\\workspace",
        "non-public document reference": "see sample_" + "INTERNAL.md",
    }
    for expected, sample in cases.items():
        if expected not in _privacy_hits(sample):
            errors.append(f"privacy policy self-test missed {expected}")
    if _privacy_hits("latency fault baseline selftest Cortex-M4 ARMv7E-M (M4)"):
        errors.append("privacy policy self-test rejected ordinary public terminology")

    private_path = pathlib.PurePosixPath("docs") / ("sample_" + "PRIVATE.md")
    if not _path_has_non_public_marker(private_path):
        errors.append("public-layout self-test missed a non-public filename")
    for public in ("core/crates/private_key.rs", "core/crates/src/local.rs", "docs/API.md"):
        if _path_has_non_public_marker(pathlib.PurePosixPath(public)):
            errors.append("public-layout self-test rejected an ordinary public path")

    bad_paths = (
        "docs\\guide.md",
        "C" + ":relative",
        "docs/control" + chr(7) + ".md",
        "docs/trailing.",
        "docs/COM" + "1.txt",
        "docs/e" + "\u0301.md",
        "docs/../README.md",
    )
    if any(_is_portable_repo_path(item) for item in bad_paths):
        errors.append("portable-path self-test accepted a noncanonical path")
    if not _is_portable_repo_path("docs/é.md"):
        errors.append("portable-path self-test rejected an NFC public path")
    if _safe_display_path("docs/owner-marker.md", ["owner-marker"]) != (
        "<redacted tracked path>"
    ):
        errors.append("privacy-path self-test failed to redact a local marker")

    png = b"\x89PNG\r\n\x1a\n" + b"public"
    if not _is_allowed_binary(pathlib.PurePosixPath("docs/images/test.png"), png):
        errors.append("binary-surface self-test rejected a public PNG")
    if _is_allowed_binary(pathlib.PurePosixPath("tools/test.png"), png):
        errors.append("binary-surface self-test accepted a PNG outside its public zone")
    uf2 = b"UF2\x0aWQ]\x9e" + bytes(504)
    if not _is_allowed_binary(pathlib.PurePosixPath("sdk/firmware/test.uf2"), uf2):
        errors.append("binary-surface self-test rejected a structurally valid UF2")

    mode_errors = _validate_layout({"README.md"}, {"README.md": ("120000", "0")})
    if "tracked entry is not a regular public file" not in mode_errors:
        errors.append("index-mode self-test missed a symbolic entry")
    collision_errors = _validate_layout(
        {"README.md", "readme.md"},
        {"README.md": ("100644", "0"), "readme.md": ("100644", "0")},
    )
    if "tracked paths collide on normalized case-insensitive filesystems" not in collision_errors:
        errors.append("portable-path self-test missed a case collision")

    valid_workflow = (
        "jobs:\n  build:\n    runs-on: \"windows-latest\"\n"
        "    run: arduino-cli core install arduinonrf:nrf52\n"
    )
    invalid_workflow = valid_workflow.replace("windows-latest", "ubuntu-latest")
    if _workflow_uses_unsupported_toolchain(valid_workflow):
        errors.append("workflow self-test rejected the supported runner")
    if not _workflow_uses_unsupported_toolchain(invalid_workflow):
        errors.append("workflow self-test missed the unsupported runner")
    return errors


def _validate_layout(
    tracked: set[str], entries: dict[str, tuple[str, str]]
) -> list[str]:
    errors: list[str] = []
    folded_paths: dict[str, str] = {}
    for relative, (mode, stage) in entries.items():
        portable = _is_portable_repo_path(relative)
        if not portable:
            errors.append("tracked entry has a noncanonical cross-platform path")
            continue
        folded = unicodedata.normalize("NFC", relative).casefold()
        if folded in folded_paths and folded_paths[folded] != relative:
            errors.append("tracked paths collide on normalized case-insensitive filesystems")
        folded_paths[folded] = relative
        if mode not in REGULAR_FILE_MODES or stage != "0":
            errors.append("tracked entry is not a regular public file")

    for relative in sorted(tracked):
        if not _is_portable_repo_path(relative):
            continue
        path = pathlib.PurePosixPath(relative)
        if _path_has_non_public_marker(path) or _privacy_hits(relative):
            errors.append("tracked path violates the public privacy convention")
        if len(path.parts) == 1:
            if relative not in PUBLIC_TOP_LEVEL_FILES:
                errors.append("tracked file is outside the declared public layout")
            continue
        top = path.parts[0]
        if top not in PUBLIC_TOP_LEVEL_DIRECTORIES:
            errors.append("tracked directory is outside the declared public layout")
            continue
        if top == ".github" and not (
            len(path.parts) == 3
            and path.parts[1] == "workflows"
            and path.suffix.lower() in {".yml", ".yaml"}
        ):
            errors.append("tracked automation is outside the public workflow surface")
        if top == "core" and not any(
            _is_within(path, root) for root in EXPECTED_CORE_ROOTS
        ):
            errors.append("tracked core path is outside the SDK distribution roots")
        if top == "packages" and not (
            relative == "packages/README.md"
            or any(_is_within(path, root) for root in EXPECTED_PACKAGE_ROOTS)
        ):
            errors.append("tracked package is outside the declared package surfaces")
        if top == "bindings" and not (
            relative == "bindings/README.md"
            or any(_is_within(path, root) for root in EXPECTED_BINDING_ROOTS)
        ):
            errors.append("tracked binding is outside the declared SDK surfaces")
        if top == "sdk" and path.parts[1] not in SDK_PUBLIC_CHILDREN:
            errors.append("tracked SDK path is outside the declared public surface")
        if top == "host" and path.parts[1] not in HOST_PUBLIC_CHILDREN:
            errors.append("tracked host path is outside the declared public surface")
        if top == "tools" and len(path.parts) != 2:
            errors.append("public tools must be single-file commands in the tools root")
    return errors


def _is_allowed_binary(path: pathlib.PurePosixPath, data: bytes) -> bool:
    if (
        len(path.parts) == 3
        and path.parts[:2] == ("docs", "images")
        and path.suffix.lower() == ".png"
    ):
        return data.startswith(b"\x89PNG\r\n\x1a\n")
    if (
        len(path.parts) == 3
        and path.parts[:2] == ("sdk", "firmware")
        and path.suffix.lower() == ".uf2"
    ):
        return (
            len(data) >= 512
            and len(data) % 512 == 0
            and data.startswith(b"UF2\x0aWQ]\x9e")
        )
    return False


def _scan_tracked_content(
    tracked: set[str], entries: dict[str, tuple[str, str]], needles: list[str]
) -> list[str]:
    errors: list[str] = []
    folded_needles = [(needle, needle.casefold()) for needle in needles]
    boundary_script = pathlib.PurePosixPath(
        pathlib.Path(__file__).resolve().relative_to(ROOT).as_posix()
    )

    for relative in sorted(tracked):
        portable = _is_portable_repo_path(relative)
        folded_relative = relative.casefold()
        local_path_marker = any(
            folded in folded_relative for _, folded in folded_needles
        )
        display = _safe_display_path(
            relative, [folded for _, folded in folded_needles]
        )
        if local_path_marker:
            errors.append("tracked path contains a local privacy marker")
        if not portable or entries.get(relative, ("", ""))[0] not in REGULAR_FILE_MODES:
            continue

        posix_path = pathlib.PurePosixPath(relative)
        path = ROOT / pathlib.Path(*posix_path.parts)
        if path.is_symlink() or not path.exists() or not path.is_file():
            errors.append(f"{display}: tracked public file is missing or non-regular")
            continue
        try:
            if not path.resolve().is_relative_to(ROOT.resolve()):
                errors.append(f"{display}: tracked public file resolves outside the repository")
                continue
        except OSError:
            errors.append(f"{display}: tracked public file cannot be resolved")
            continue

        data = path.read_bytes()
        lowered_data = data.lower()
        if any(
            needle.encode("utf-8").lower() in lowered_data
            for needle, _ in folded_needles
        ):
            errors.append(f"{display}: local privacy marker present")
        if _is_allowed_binary(posix_path, data):
            continue
        if posix_path.suffix.lower() in BINARY_SUFFIXES:
            errors.append(f"{display}: binary file is outside an approved public binary surface")
            continue
        if b"\0" in data or any(
            byte < 9 or byte in {11, 12, 127} for byte in data
        ):
            errors.append(f"{display}: public text file contains binary control data")
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            errors.append(f"{display}: public text file is not UTF-8")
            continue
        if posix_path == boundary_script:
            continue

        generated_tools = set()
        if relative == GENERATED_TOOL_TEMPLATE:
            generated_tools = {
                match.group(1).replace("\\", "/")
                for match in GENERATED_TOOL_DECLARATION.finditer(text)
            }
        for label, pattern in PRIVACY_PATTERNS:
            for match in pattern.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{display}:{line}: violates {label} policy")
        for match in TOOL_REFERENCE.finditer(text):
            target = match.group(1).replace("\\", "/")
            if target in generated_tools:
                continue
            if target not in tracked or not (ROOT / target).is_file():
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{display}:{line}: references a missing public tool")
    return errors


def _validate_host_contract() -> list[str]:
    package_root = ROOT / "bindings" / "python"
    sys.path.insert(0, str(package_root))
    try:
        from nobro_rtos.host_contract import HostContract

        HostContract.from_path(ROOT / "host" / "nobro-host-contract.json")
    except Exception:  # noqa: BLE001 - convert all schema failures to one redacted gate error.
        return ["canonical host contract violates the public schema"]
    finally:
        try:
            sys.path.remove(str(package_root))
        except ValueError:
            pass
    return []


def validate() -> list[str]:
    errors = _policy_selftest()
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        manifest = None
        errors.append("SDK manifest is missing or invalid JSON")
    errors.extend(_validate_manifest(manifest))

    entries, entry_errors = _tracked_entries()
    errors.extend(entry_errors)
    tracked = set(entries)
    needles = _local_needles()
    errors.extend(_validate_layout(tracked, entries))

    for public in EXPECTED_HOST_TOOLS:
        if any(_overlaps(public, excluded) for excluded in EXPECTED_RELEASE_EXCLUDES):
            errors.append("public tool overlaps a generated-output root")
        if public.as_posix() not in tracked or not (ROOT / public).is_file():
            errors.append("public host tool is missing or untracked")
    for included in EXPECTED_CORE_ROOTS:
        if any(_overlaps(included, excluded) for excluded in EXPECTED_RELEASE_EXCLUDES):
            errors.append("core distribution root overlaps generated output")
        if not _has_tracked_content(included, tracked):
            errors.append("core distribution root is missing or untracked")
    for package in EXPECTED_PACKAGE_ROOTS:
        if not _has_tracked_content(package, tracked):
            errors.append("package surface is missing or untracked")
    for binding in EXPECTED_BINDING_ROOTS:
        if not _has_tracked_content(binding, tracked):
            errors.append("binding surface is missing or untracked")

    if any(COMPARISON_TOOL.fullmatch(path) for path in tracked):
        errors.append("comparison commands must not be part of the public tools surface")
    workflow_path = ROOT / ".github" / "workflows" / "gates.yml"
    try:
        workflow = workflow_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        workflow = ""
        errors.append("public hosted workflow is missing or invalid")
    if re.search(r"tools[\\/]measure_[A-Za-z0-9_.-]+\.py", workflow, re.IGNORECASE):
        errors.append("hosted workflow invokes a non-product comparison command")
    if _workflow_uses_unsupported_toolchain(workflow):
        errors.append("hosted workflow installs a platform toolchain on an unsupported runner")

    errors.extend(_validate_host_contract())
    errors.extend(_scan_tracked_content(tracked, entries, needles))
    return errors


def main() -> int:
    errors = list(dict.fromkeys(validate()))
    for error in errors:
        print(f"RELEASE BOUNDARY: {error}")
    print(f"RELEASE BOUNDARY: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
