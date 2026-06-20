"""Typed host-side contract builders for NobroRTOS.

These helpers intentionally mirror the public firmware contracts without trying
to run realtime policy in Python. They are for VS Code tasks, simulations,
metadata generation, report tooling, AI model descriptors, and ROS bridge
descriptors.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import IntEnum
import json
from typing import Any

CONTRACT_SCHEMA_VERSION = 1


class Capability(IntEnum):
    TIMEBASE = 0
    DEADLINE_TIMER = 1
    EVENT_CAPTURE = 2
    BUS0 = 3
    BUS1 = 4
    RADIO = 5
    SERVO_PWM = 6
    STREAM = 7
    CRYPTO = 8
    SAMPLE_POOL = 9
    HOST_REPORT = 10
    AI_INFERENCE = 11
    AI_ENDPOINT = 12

    @property
    def bit(self) -> int:
        return 1 << int(self)


class Criticality(IntEnum):
    BEST_EFFORT = 0
    USER = 1
    DRIVER = 2
    SYSTEM = 3
    HARD_REALTIME = 4


class AiBackendKind(IntEnum):
    ON_DEVICE = 1
    REMOTE_API = 2
    EDGE_SIDECAR = 3
    HYBRID = 4


def capability_mask(*capabilities: Capability) -> int:
    mask = 0
    for capability in capabilities:
        mask |= capability.bit
    return mask


@dataclass(frozen=True)
class MemoryBudget:
    flash_bytes: int
    ram_bytes: int
    pool_slots: int = 0

    def validate(self) -> None:
        if self.flash_bytes <= 0 or self.ram_bytes <= 0:
            raise ValueError("flash and RAM budgets must be positive")
        if self.pool_slots < 0:
            raise ValueError("pool slots cannot be negative")

    def to_dict(self) -> dict[str, int]:
        self.validate()
        return {
            "flash_bytes": self.flash_bytes,
            "ram_bytes": self.ram_bytes,
            "pool_slots": self.pool_slots,
        }


@dataclass(frozen=True)
class ModuleSpec:
    module: str
    criticality: Criticality
    memory: MemoryBudget
    requires: tuple[Capability, ...] = ()
    owns: tuple[Capability, ...] = ()
    period_us: int | None = None
    max_jitter_us: int | None = None

    def validate(self) -> None:
        self.memory.validate()
        if self.criticality == Criticality.HARD_REALTIME:
            if not self.period_us or not self.max_jitter_us:
                raise ValueError("hard realtime modules require a deadline")
        if self.period_us is not None and self.period_us <= 0:
            raise ValueError("period_us must be positive when present")
        if self.max_jitter_us is not None and self.max_jitter_us <= 0:
            raise ValueError("max_jitter_us must be positive when present")

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "module": self.module,
            "criticality": self.criticality.name.lower(),
            "requires_bits": capability_mask(*self.requires),
            "owns_bits": capability_mask(*self.owns),
            "memory": self.memory.to_dict(),
            "deadline": (
                None
                if self.period_us is None
                else {
                    "period_us": self.period_us,
                    "max_jitter_us": self.max_jitter_us,
                }
            ),
        }


@dataclass(frozen=True)
class AiModelContract:
    model_id: int
    backend: AiBackendKind
    input_bytes_max: int
    output_bytes_max: int
    arena_bytes: int
    timeout_us: int
    stale_after_us: int

    def validate(self) -> None:
        for field_name in (
            "model_id",
            "input_bytes_max",
            "output_bytes_max",
            "timeout_us",
            "stale_after_us",
        ):
            if getattr(self, field_name) <= 0:
                raise ValueError(f"{field_name} must be positive")
        if self.arena_bytes < 0:
            raise ValueError("arena_bytes cannot be negative")

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "model_id": self.model_id,
            "backend": self.backend.name.lower(),
            "input_bytes_max": self.input_bytes_max,
            "output_bytes_max": self.output_bytes_max,
            "arena_bytes": self.arena_bytes,
            "timeout_us": self.timeout_us,
            "stale_after_us": self.stale_after_us,
        }


@dataclass(frozen=True)
class RosTopic:
    name: str
    message_type: str
    depth: int
    max_message_bytes: int

    def validate(self) -> None:
        _validate_name(self.name)
        _validate_name(self.message_type)
        _validate_positive("depth", self.depth)
        _validate_positive("max_message_bytes", self.max_message_bytes)

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "name": self.name,
            "message_type": self.message_type,
            "depth": self.depth,
            "max_message_bytes": self.max_message_bytes,
        }


@dataclass(frozen=True)
class RosService:
    name: str
    request_bytes_max: int
    response_bytes_max: int
    timeout_us: int

    def validate(self) -> None:
        _validate_name(self.name)
        _validate_positive("request_bytes_max", self.request_bytes_max)
        _validate_positive("response_bytes_max", self.response_bytes_max)
        _validate_positive("timeout_us", self.timeout_us)

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "name": self.name,
            "request_bytes_max": self.request_bytes_max,
            "response_bytes_max": self.response_bytes_max,
            "timeout_us": self.timeout_us,
        }


@dataclass(frozen=True)
class RosAction:
    name: str
    goal_bytes_max: int
    feedback_bytes_max: int
    result_bytes_max: int
    timeout_us: int

    def validate(self) -> None:
        _validate_name(self.name)
        _validate_positive("goal_bytes_max", self.goal_bytes_max)
        _validate_positive("feedback_bytes_max", self.feedback_bytes_max)
        _validate_positive("result_bytes_max", self.result_bytes_max)
        _validate_positive("timeout_us", self.timeout_us)

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "name": self.name,
            "goal_bytes_max": self.goal_bytes_max,
            "feedback_bytes_max": self.feedback_bytes_max,
            "result_bytes_max": self.result_bytes_max,
            "timeout_us": self.timeout_us,
        }


@dataclass(frozen=True)
class RosParameter:
    name: str
    value_bytes_max: int

    def validate(self) -> None:
        _validate_name(self.name)
        _validate_positive("value_bytes_max", self.value_bytes_max)

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "name": self.name,
            "value_bytes_max": self.value_bytes_max,
        }


@dataclass(frozen=True)
class RosBridgeDescriptor:
    bridge_id: str
    transport: str
    topics: tuple[RosTopic, ...] = ()
    services: tuple[RosService, ...] = ()
    actions: tuple[RosAction, ...] = ()
    parameters: tuple[RosParameter, ...] = ()

    def validate(self) -> None:
        _validate_name(self.bridge_id)
        _validate_name(self.transport)
        _validate_unique("ROS topic", [topic.name for topic in self.topics])
        _validate_unique("ROS service", [service.name for service in self.services])
        _validate_unique("ROS action", [action.name for action in self.actions])
        _validate_unique("ROS parameter", [param.name for param in self.parameters])
        for topic in self.topics:
            topic.validate()
        for service in self.services:
            service.validate()
        for action in self.actions:
            action.validate()
        for param in self.parameters:
            param.validate()

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "bridge_id": self.bridge_id,
            "transport": self.transport,
            "topics": [topic.to_dict() for topic in self.topics],
            "services": [service.to_dict() for service in self.services],
            "actions": [action.to_dict() for action in self.actions],
            "parameters": [param.to_dict() for param in self.parameters],
        }


@dataclass(frozen=True)
class NobroContractBundle:
    modules: tuple[ModuleSpec, ...] = ()
    ai_models: tuple[AiModelContract, ...] = ()
    ros_bridges: tuple[RosBridgeDescriptor, ...] = ()
    metadata: dict[str, str] = field(default_factory=dict)

    def validate(self) -> None:
        _validate_unique("module", [module.module for module in self.modules])
        _validate_unique("AI model", [str(model.model_id) for model in self.ai_models])
        _validate_unique("ROS bridge", [bridge.bridge_id for bridge in self.ros_bridges])
        for module in self.modules:
            module.validate()
        for model in self.ai_models:
            model.validate()
        for bridge in self.ros_bridges:
            bridge.validate()

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "schema_version": CONTRACT_SCHEMA_VERSION,
            "metadata": dict(sorted(self.metadata.items())),
            "modules": [module.to_dict() for module in self.modules],
            "ai_models": [model.to_dict() for model in self.ai_models],
            "ros_bridges": [bridge.to_dict() for bridge in self.ros_bridges],
        }

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), indent=2, sort_keys=True)


def _validate_name(value: str) -> None:
    if not value or value.strip() != value:
        raise ValueError("names must be non-empty and trimmed")


def _validate_positive(name: str, value: int) -> None:
    if value <= 0:
        raise ValueError(f"{name} must be positive")


def _validate_unique(kind: str, values: list[str]) -> None:
    seen: set[str] = set()
    for value in values:
        if value in seen:
            raise ValueError(f"duplicate {kind}: {value}")
        seen.add(value)
