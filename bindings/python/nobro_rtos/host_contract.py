"""Host-contract loader and validation helpers for NobroRTOS tooling."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import json
from typing import Any

from .contracts import Capability


DEFAULT_CONTRACT_RELATIVE_PATH = Path("host") / "nobro-host-contract.json"
EXPECTED_BOOT_STAGES = (
    "board_profile",
    "board_package",
    "manifest",
    "adapter_compatibility",
    "admission",
    "runtime",
)
EXPECTED_STATUS_LABELS = ("missing", "in_progress", "pass", "fail", "corrupt")


@dataclass(frozen=True)
class HostContract:
    """Parsed host contract mirrored from the repository JSON file."""

    payload: dict[str, Any]

    @classmethod
    def from_path(cls, path: str | Path) -> "HostContract":
        with Path(path).open("r", encoding="utf-8") as handle:
            payload = json.load(handle)
        contract = cls(payload)
        contract.validate()
        return contract

    def validate(self) -> None:
        self._require_object("module_tags")
        self._require_object("capability_bits")
        boot = self._require_object("boot_diagnostics")

        stages = tuple(boot.get("stage_order", ()))
        if stages != EXPECTED_BOOT_STAGES:
            raise ValueError(f"unexpected boot stage order: {stages}")

        status_labels = tuple(boot.get("status_labels", ()))
        if status_labels != EXPECTED_STATUS_LABELS:
            raise ValueError(f"unexpected status labels: {status_labels}")

        self._validate_capability_bits()

    def capability_label(self, capability: Capability) -> str:
        return self.payload["capability_bits"][str(int(capability))]

    def module_label(self, code: int | str) -> str:
        return self.payload["module_tags"][str(code)]

    def boot_stage_order(self) -> tuple[str, ...]:
        return tuple(self.payload["boot_diagnostics"]["stage_order"])

    def status_labels(self) -> tuple[str, ...]:
        return tuple(self.payload["boot_diagnostics"]["status_labels"])

    def _require_object(self, key: str) -> dict[str, Any]:
        value = self.payload.get(key)
        if not isinstance(value, dict):
            raise ValueError(f"missing object: {key}")
        return value

    def _validate_capability_bits(self) -> None:
        capability_bits = self.payload["capability_bits"]
        for capability in Capability:
            label = capability_bits.get(str(int(capability)))
            expected = capability.name.lower()
            if label != expected:
                raise ValueError(
                    f"capability {capability.name} expected {expected}, got {label}"
                )


def find_repo_root(start: str | Path | None = None) -> Path:
    """Find a repository root containing the canonical host contract JSON."""

    current = Path.cwd() if start is None else Path(start).resolve()
    for candidate in (current, *current.parents):
        contract_path = candidate / DEFAULT_CONTRACT_RELATIVE_PATH
        if contract_path.exists():
            return candidate
    raise FileNotFoundError("could not find host/nobro-host-contract.json")


def load_repo_host_contract(start: str | Path | None = None) -> HostContract:
    root = find_repo_root(start)
    return HostContract.from_path(root / DEFAULT_CONTRACT_RELATIVE_PATH)
