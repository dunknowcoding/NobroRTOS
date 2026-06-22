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
from pathlib import Path
from typing import Any

CONTRACT_SCHEMA_VERSION = 1
FNV1A32_OFFSET = 0x811C9DC5
FNV1A32_PRIME = 0x01000193


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


class AiRoutePreference(IntEnum):
    LOCAL_ONLY = 1
    PREFER_LOCAL = 2
    PREFER_REMOTE = 3
    HYBRID_FALLBACK = 4


class AiRouteTarget(IntEnum):
    ON_DEVICE = 1
    REMOTE_API = 2
    EDGE_SIDECAR = 3
    STALE_SNAPSHOT = 4
    DEGRADED_FALLBACK = 5
    UNAVAILABLE = 6


def capability_mask(*capabilities: Capability) -> int:
    mask = 0
    for capability in capabilities:
        mask |= capability.bit
    return mask


def capabilities_from_mask(mask: int) -> tuple[Capability, ...]:
    capabilities = tuple(capability for capability in Capability if mask & capability.bit)
    known_bits = capability_mask(*capabilities)
    unknown_bits = mask & ~known_bits
    if unknown_bits:
        raise ValueError(f"unknown capability bits: 0x{unknown_bits:X}")
    return capabilities


def stable_hash32(value: str) -> int:
    """Return the stable FNV-1a 32-bit hash used by bridge metadata."""

    result = FNV1A32_OFFSET
    for byte in value.encode("utf-8"):
        result ^= byte
        result = (result * FNV1A32_PRIME) & 0xFFFF_FFFF
    return result


def _enum_from_label(enum_type: type[IntEnum], label: str) -> IntEnum:
    normalized = label.upper()
    for item in enum_type:
        if item.name == normalized:
            return item
    raise ValueError(f"unknown {enum_type.__name__} label: {label}")


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

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "MemoryBudget":
        return cls(
            int(payload["flash_bytes"]),
            int(payload["ram_bytes"]),
            int(payload.get("pool_slots", 0)),
        )


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
        _validate_name(self.module)
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

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "ModuleSpec":
        deadline = payload.get("deadline")
        period_us = None
        max_jitter_us = None
        if deadline is not None:
            period_us = int(deadline["period_us"])
            max_jitter_us = int(deadline["max_jitter_us"])

        return cls(
            module=str(payload["module"]),
            criticality=_enum_from_label(Criticality, str(payload["criticality"])),
            memory=MemoryBudget.from_dict(payload["memory"]),
            requires=capabilities_from_mask(int(payload.get("requires_bits", 0))),
            owns=capabilities_from_mask(int(payload.get("owns_bits", 0))),
            period_us=period_us,
            max_jitter_us=max_jitter_us,
        )


@dataclass(frozen=True)
class StartupDependency:
    module: str
    depends_on: str

    def validate(self) -> None:
        _validate_name(self.module)
        _validate_name(self.depends_on)
        if self.module == self.depends_on:
            raise ValueError(f"startup dependency self-cycle: {self.module}")

    def to_dict(self) -> dict[str, str]:
        self.validate()
        return {
            "module": self.module,
            "depends_on": self.depends_on,
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "StartupDependency":
        return cls(str(payload["module"]), str(payload["depends_on"]))


@dataclass(frozen=True)
class StartupPlan:
    order: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "order": list(self.order),
            "startup_len": len(self.order),
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

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "AiModelContract":
        return cls(
            model_id=int(payload["model_id"]),
            backend=_enum_from_label(AiBackendKind, str(payload["backend"])),
            input_bytes_max=int(payload["input_bytes_max"]),
            output_bytes_max=int(payload["output_bytes_max"]),
            arena_bytes=int(payload.get("arena_bytes", 0)),
            timeout_us=int(payload["timeout_us"]),
            stale_after_us=int(payload["stale_after_us"]),
        )


@dataclass(frozen=True)
class AiRuntimeState:
    local_ready: bool
    endpoint_ready: bool
    last_success_age_us: int
    consecutive_endpoint_failures: int

    def validate(self) -> None:
        if self.last_success_age_us < 0:
            raise ValueError("last_success_age_us cannot be negative")
        if self.consecutive_endpoint_failures < 0:
            raise ValueError("consecutive_endpoint_failures cannot be negative")


@dataclass(frozen=True)
class AiInvocationConstraints:
    input_bytes: int
    output_bytes: int
    scratch_bytes: int
    budget_us: int
    max_stale_us: int = 0
    allow_stale_snapshot: bool = False
    allow_degraded_fallback: bool = False
    allow_unavailable: bool = False
    allow_endpoint_circuit_open: bool = False

    def validate(self) -> None:
        _validate_positive("input_bytes", self.input_bytes)
        _validate_positive("output_bytes", self.output_bytes)
        if self.scratch_bytes < 0:
            raise ValueError("scratch_bytes cannot be negative")
        if self.budget_us < 0:
            raise ValueError("budget_us cannot be negative")
        if self.max_stale_us < 0:
            raise ValueError("max_stale_us cannot be negative")

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "input_bytes": self.input_bytes,
            "output_bytes": self.output_bytes,
            "scratch_bytes": self.scratch_bytes,
            "budget_us": self.budget_us,
            "max_stale_us": self.max_stale_us,
            "allow_stale_snapshot": self.allow_stale_snapshot,
            "allow_degraded_fallback": self.allow_degraded_fallback,
            "allow_unavailable": self.allow_unavailable,
            "allow_endpoint_circuit_open": self.allow_endpoint_circuit_open,
        }


@dataclass(frozen=True)
class AiRouteDecision:
    target: AiRouteTarget
    endpoint_circuit_open: bool
    uses_stale_snapshot: bool

    def to_dict(self) -> dict[str, Any]:
        return {
            "target": self.target.name.lower(),
            "endpoint_circuit_open": self.endpoint_circuit_open,
            "uses_stale_snapshot": self.uses_stale_snapshot,
        }


@dataclass(frozen=True)
class AiRoutePolicy:
    preference: AiRoutePreference
    stale_after_us: int
    endpoint_failure_limit: int

    def validate(self) -> None:
        if self.stale_after_us < 0:
            raise ValueError("stale_after_us cannot be negative")
        if self.endpoint_failure_limit < 0:
            raise ValueError("endpoint_failure_limit cannot be negative")

    def decide(
        self,
        contract: AiModelContract,
        state: AiRuntimeState,
        budget_us: int,
    ) -> AiRouteDecision:
        self.validate()
        contract.validate()
        state.validate()
        if budget_us < 0:
            raise ValueError("budget_us cannot be negative")

        failure_limit = self.endpoint_failure_limit or 1
        endpoint_circuit_open = state.consecutive_endpoint_failures >= failure_limit
        stale_ready = state.last_success_age_us <= self.effective_stale_after_us(contract)
        fits_budget = contract.timeout_us <= budget_us

        if not fits_budget:
            return self._fallback(endpoint_circuit_open, stale_ready)

        if contract.backend == AiBackendKind.ON_DEVICE:
            if state.local_ready:
                return AiRouteDecision(
                    AiRouteTarget.ON_DEVICE,
                    endpoint_circuit_open,
                    False,
                )
            return self._fallback(endpoint_circuit_open, stale_ready)

        if contract.backend == AiBackendKind.REMOTE_API:
            return self._remote_or_fallback(
                AiRouteTarget.REMOTE_API,
                state,
                endpoint_circuit_open,
                stale_ready,
            )

        if contract.backend == AiBackendKind.EDGE_SIDECAR:
            return self._remote_or_fallback(
                AiRouteTarget.EDGE_SIDECAR,
                state,
                endpoint_circuit_open,
                stale_ready,
            )

        if contract.backend == AiBackendKind.HYBRID:
            return self._hybrid_decision(state, endpoint_circuit_open, stale_ready)

        return self._fallback(endpoint_circuit_open, stale_ready)

    def _remote_or_fallback(
        self,
        target: AiRouteTarget,
        state: AiRuntimeState,
        endpoint_circuit_open: bool,
        stale_ready: bool,
    ) -> AiRouteDecision:
        if (
            self.preference != AiRoutePreference.LOCAL_ONLY
            and state.endpoint_ready
            and not endpoint_circuit_open
        ):
            return AiRouteDecision(target, endpoint_circuit_open, False)
        return self._fallback(endpoint_circuit_open, stale_ready)

    def _hybrid_decision(
        self,
        state: AiRuntimeState,
        endpoint_circuit_open: bool,
        stale_ready: bool,
    ) -> AiRouteDecision:
        if self.preference in (
            AiRoutePreference.LOCAL_ONLY,
            AiRoutePreference.PREFER_LOCAL,
        ):
            if state.local_ready:
                return AiRouteDecision(
                    AiRouteTarget.ON_DEVICE,
                    endpoint_circuit_open,
                    False,
                )
            return self._remote_or_fallback(
                AiRouteTarget.REMOTE_API,
                state,
                endpoint_circuit_open,
                stale_ready,
            )

        if state.endpoint_ready and not endpoint_circuit_open:
            return AiRouteDecision(AiRouteTarget.REMOTE_API, endpoint_circuit_open, False)
        if state.local_ready:
            return AiRouteDecision(AiRouteTarget.ON_DEVICE, endpoint_circuit_open, False)
        return self._fallback(endpoint_circuit_open, stale_ready)

    def _fallback(
        self,
        endpoint_circuit_open: bool,
        stale_ready: bool,
    ) -> AiRouteDecision:
        if stale_ready:
            return AiRouteDecision(
                AiRouteTarget.STALE_SNAPSHOT,
                endpoint_circuit_open,
                True,
            )
        if self.preference == AiRoutePreference.LOCAL_ONLY:
            return AiRouteDecision(AiRouteTarget.UNAVAILABLE, endpoint_circuit_open, False)
        return AiRouteDecision(
            AiRouteTarget.DEGRADED_FALLBACK,
            endpoint_circuit_open,
            False,
        )

    def effective_stale_after_us(self, contract: AiModelContract) -> int:
        """Return the strict stale snapshot window shared by host and firmware."""

        if self.stale_after_us == 0:
            return contract.stale_after_us
        if contract.stale_after_us == 0:
            return self.stale_after_us
        return min(self.stale_after_us, contract.stale_after_us)


@dataclass(frozen=True)
class AiPreflightReport:
    passing: bool
    errors: tuple[str, ...]
    required_capabilities: tuple[Capability, ...]
    route: AiRouteDecision
    required_ram_bytes: int
    available_ram_bytes: int
    constraints: AiInvocationConstraints

    def to_dict(self) -> dict[str, Any]:
        return {
            "passing": self.passing,
            "errors": list(self.errors),
            "required_capabilities": [
                capability.name.lower() for capability in self.required_capabilities
            ],
            "route": self.route.to_dict(),
            "required_ram_bytes": self.required_ram_bytes,
            "available_ram_bytes": self.available_ram_bytes,
            "constraints": self.constraints.to_dict(),
        }


def preflight_ai_invocation(
    module: ModuleSpec,
    contract: AiModelContract,
    policy: AiRoutePolicy,
    state: AiRuntimeState,
    constraints: AiInvocationConstraints,
) -> AiPreflightReport:
    """Check deterministic AI call admission without contacting an inference backend."""

    module.validate()
    contract.validate()
    policy.validate()
    state.validate()
    constraints.validate()

    route = policy.decide(contract, state, constraints.budget_us)
    required_capabilities = _required_ai_capabilities(contract.backend)
    local_arena_bytes = (
        contract.arena_bytes
        if contract.backend in (AiBackendKind.ON_DEVICE, AiBackendKind.HYBRID)
        else 0
    )
    required_ram_bytes = (
        constraints.input_bytes
        + constraints.output_bytes
        + constraints.scratch_bytes
        + local_arena_bytes
    )
    errors: list[str] = []

    if constraints.input_bytes > contract.input_bytes_max:
        errors.append(
            "AI input exceeds model contract: "
            f"{constraints.input_bytes} > {contract.input_bytes_max}"
        )
    if constraints.output_bytes > contract.output_bytes_max:
        errors.append(
            "AI output exceeds model contract: "
            f"{constraints.output_bytes} > {contract.output_bytes_max}"
        )
    if required_ram_bytes > module.memory.ram_bytes:
        errors.append(
            "AI invocation RAM exceeds module budget: "
            f"{required_ram_bytes} > {module.memory.ram_bytes}"
        )

    missing_capabilities = tuple(
        capability for capability in required_capabilities if capability not in module.requires
    )
    if missing_capabilities:
        labels = ", ".join(capability.name.lower() for capability in missing_capabilities)
        errors.append(f"AI module missing required capabilities: {labels}")

    if route.target == AiRouteTarget.UNAVAILABLE and not constraints.allow_unavailable:
        errors.append("AI route is unavailable")
    if (
        route.target == AiRouteTarget.DEGRADED_FALLBACK
        and not constraints.allow_degraded_fallback
    ):
        errors.append("AI route used degraded fallback")
    if route.uses_stale_snapshot and not constraints.allow_stale_snapshot:
        errors.append("AI route used a stale snapshot")
    if (
        route.uses_stale_snapshot
        and constraints.max_stale_us > 0
        and state.last_success_age_us > constraints.max_stale_us
    ):
        errors.append(
            "AI stale snapshot exceeds invocation limit: "
            f"{state.last_success_age_us} > {constraints.max_stale_us}"
        )
    if route.endpoint_circuit_open and not constraints.allow_endpoint_circuit_open:
        errors.append("AI endpoint circuit is open")

    return AiPreflightReport(
        passing=len(errors) == 0,
        errors=tuple(errors),
        required_capabilities=required_capabilities,
        route=route,
        required_ram_bytes=required_ram_bytes,
        available_ram_bytes=module.memory.ram_bytes,
        constraints=constraints,
    )


def _required_ai_capabilities(backend: AiBackendKind) -> tuple[Capability, ...]:
    if backend == AiBackendKind.ON_DEVICE:
        return (Capability.AI_INFERENCE,)
    if backend in (AiBackendKind.REMOTE_API, AiBackendKind.EDGE_SIDECAR):
        return (Capability.AI_ENDPOINT,)
    if backend == AiBackendKind.HYBRID:
        return (Capability.AI_INFERENCE, Capability.AI_ENDPOINT)
    return ()


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
            "name_hash": stable_hash32(self.name),
            "message_type": self.message_type,
            "message_type_hash": stable_hash32(self.message_type),
            "depth": self.depth,
            "max_message_bytes": self.max_message_bytes,
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RosTopic":
        return cls(
            name=str(payload["name"]),
            message_type=str(payload["message_type"]),
            depth=int(payload["depth"]),
            max_message_bytes=int(payload["max_message_bytes"]),
        )


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
            "name_hash": stable_hash32(self.name),
            "request_bytes_max": self.request_bytes_max,
            "response_bytes_max": self.response_bytes_max,
            "timeout_us": self.timeout_us,
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RosService":
        return cls(
            name=str(payload["name"]),
            request_bytes_max=int(payload["request_bytes_max"]),
            response_bytes_max=int(payload["response_bytes_max"]),
            timeout_us=int(payload["timeout_us"]),
        )


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
            "name_hash": stable_hash32(self.name),
            "goal_bytes_max": self.goal_bytes_max,
            "feedback_bytes_max": self.feedback_bytes_max,
            "result_bytes_max": self.result_bytes_max,
            "timeout_us": self.timeout_us,
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RosAction":
        return cls(
            name=str(payload["name"]),
            goal_bytes_max=int(payload["goal_bytes_max"]),
            feedback_bytes_max=int(payload["feedback_bytes_max"]),
            result_bytes_max=int(payload["result_bytes_max"]),
            timeout_us=int(payload["timeout_us"]),
        )


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
            "name_hash": stable_hash32(self.name),
            "value_bytes_max": self.value_bytes_max,
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RosParameter":
        return cls(
            name=str(payload["name"]),
            value_bytes_max=int(payload["value_bytes_max"]),
        )


@dataclass(frozen=True)
class RosPreflightReport:
    kind: str
    name: str
    passing: bool
    errors: tuple[str, ...]
    required_buffer_bytes: int
    limits: dict[str, int]

    def to_dict(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "name": self.name,
            "passing": self.passing,
            "errors": list(self.errors),
            "required_buffer_bytes": self.required_buffer_bytes,
            "limits": dict(sorted(self.limits.items())),
        }


def preflight_ros_topic(topic: RosTopic, payload_bytes: int) -> RosPreflightReport:
    topic.validate()
    _validate_non_negative("payload_bytes", payload_bytes)
    errors: list[str] = []
    if payload_bytes > topic.max_message_bytes:
        errors.append(
            "ROS topic payload exceeds contract: "
            f"{payload_bytes} > {topic.max_message_bytes}"
        )
    return RosPreflightReport(
        kind="topic",
        name=topic.name,
        passing=len(errors) == 0,
        errors=tuple(errors),
        required_buffer_bytes=topic.depth * topic.max_message_bytes,
        limits={
            "payload_bytes": payload_bytes,
            "depth": topic.depth,
            "max_message_bytes": topic.max_message_bytes,
        },
    )


def preflight_ros_service(
    service: RosService,
    request_bytes: int,
    response_capacity_bytes: int,
    budget_us: int,
) -> RosPreflightReport:
    service.validate()
    _validate_non_negative("request_bytes", request_bytes)
    _validate_non_negative("response_capacity_bytes", response_capacity_bytes)
    _validate_non_negative("budget_us", budget_us)
    errors: list[str] = []
    if request_bytes > service.request_bytes_max:
        errors.append(
            "ROS service request exceeds contract: "
            f"{request_bytes} > {service.request_bytes_max}"
        )
    if response_capacity_bytes < service.response_bytes_max:
        errors.append(
            "ROS service response capacity is too small: "
            f"{response_capacity_bytes} < {service.response_bytes_max}"
        )
    if service.timeout_us > budget_us:
        errors.append(
            "ROS service timeout exceeds budget: "
            f"{service.timeout_us} > {budget_us}"
        )
    return RosPreflightReport(
        kind="service",
        name=service.name,
        passing=len(errors) == 0,
        errors=tuple(errors),
        required_buffer_bytes=service.request_bytes_max + service.response_bytes_max,
        limits={
            "request_bytes": request_bytes,
            "response_capacity_bytes": response_capacity_bytes,
            "budget_us": budget_us,
            "request_bytes_max": service.request_bytes_max,
            "response_bytes_max": service.response_bytes_max,
            "timeout_us": service.timeout_us,
        },
    )


def preflight_ros_action(
    action: RosAction,
    goal_bytes: int,
    feedback_capacity_bytes: int,
    result_capacity_bytes: int,
    budget_us: int,
) -> RosPreflightReport:
    action.validate()
    _validate_non_negative("goal_bytes", goal_bytes)
    _validate_non_negative("feedback_capacity_bytes", feedback_capacity_bytes)
    _validate_non_negative("result_capacity_bytes", result_capacity_bytes)
    _validate_non_negative("budget_us", budget_us)
    errors: list[str] = []
    if goal_bytes > action.goal_bytes_max:
        errors.append(
            "ROS action goal exceeds contract: "
            f"{goal_bytes} > {action.goal_bytes_max}"
        )
    if feedback_capacity_bytes < action.feedback_bytes_max:
        errors.append(
            "ROS action feedback capacity is too small: "
            f"{feedback_capacity_bytes} < {action.feedback_bytes_max}"
        )
    if result_capacity_bytes < action.result_bytes_max:
        errors.append(
            "ROS action result capacity is too small: "
            f"{result_capacity_bytes} < {action.result_bytes_max}"
        )
    if action.timeout_us > budget_us:
        errors.append(
            "ROS action timeout exceeds budget: "
            f"{action.timeout_us} > {budget_us}"
        )
    return RosPreflightReport(
        kind="action",
        name=action.name,
        passing=len(errors) == 0,
        errors=tuple(errors),
        required_buffer_bytes=(
            action.goal_bytes_max
            + action.feedback_bytes_max
            + action.result_bytes_max
        ),
        limits={
            "goal_bytes": goal_bytes,
            "feedback_capacity_bytes": feedback_capacity_bytes,
            "result_capacity_bytes": result_capacity_bytes,
            "budget_us": budget_us,
            "goal_bytes_max": action.goal_bytes_max,
            "feedback_bytes_max": action.feedback_bytes_max,
            "result_bytes_max": action.result_bytes_max,
            "timeout_us": action.timeout_us,
        },
    )


def preflight_ros_parameter(
    parameter: RosParameter,
    value_bytes: int,
) -> RosPreflightReport:
    parameter.validate()
    _validate_non_negative("value_bytes", value_bytes)
    errors: list[str] = []
    if value_bytes > parameter.value_bytes_max:
        errors.append(
            "ROS parameter value exceeds contract: "
            f"{value_bytes} > {parameter.value_bytes_max}"
        )
    return RosPreflightReport(
        kind="parameter",
        name=parameter.name,
        passing=len(errors) == 0,
        errors=tuple(errors),
        required_buffer_bytes=parameter.value_bytes_max,
        limits={
            "value_bytes": value_bytes,
            "value_bytes_max": parameter.value_bytes_max,
        },
    )


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
            "bridge_id_hash": stable_hash32(self.bridge_id),
            "transport": self.transport,
            "transport_hash": stable_hash32(self.transport),
            "topics": [topic.to_dict() for topic in self.topics],
            "services": [service.to_dict() for service in self.services],
            "actions": [action.to_dict() for action in self.actions],
            "parameters": [param.to_dict() for param in self.parameters],
        }

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RosBridgeDescriptor":
        return cls(
            bridge_id=str(payload["bridge_id"]),
            transport=str(payload["transport"]),
            topics=tuple(RosTopic.from_dict(item) for item in payload.get("topics", ())),
            services=tuple(
                RosService.from_dict(item) for item in payload.get("services", ())
            ),
            actions=tuple(
                RosAction.from_dict(item) for item in payload.get("actions", ())
            ),
            parameters=tuple(
                RosParameter.from_dict(item) for item in payload.get("parameters", ())
            ),
        )


@dataclass(frozen=True)
class NobroContractBundle:
    modules: tuple[ModuleSpec, ...] = ()
    ai_models: tuple[AiModelContract, ...] = ()
    ros_bridges: tuple[RosBridgeDescriptor, ...] = ()
    startup_dependencies: tuple[StartupDependency, ...] = ()
    metadata: dict[str, str] = field(default_factory=dict)

    def validate(self) -> None:
        _validate_unique("module", [module.module for module in self.modules])
        _validate_unique("AI model", [str(model.model_id) for model in self.ai_models])
        _validate_unique("ROS bridge", [bridge.bridge_id for bridge in self.ros_bridges])
        _validate_unique(
            "startup dependency",
            [
                f"{dependency.module}->{dependency.depends_on}"
                for dependency in self.startup_dependencies
            ],
        )
        for module in self.modules:
            module.validate()
        for model in self.ai_models:
            model.validate()
        for bridge in self.ros_bridges:
            bridge.validate()
        for dependency in self.startup_dependencies:
            dependency.validate()
        _validate_capability_ownership(self.modules)
        plan_startup(self.modules, self.startup_dependencies)

    def to_dict(self) -> dict[str, Any]:
        self.validate()
        return {
            "schema_version": CONTRACT_SCHEMA_VERSION,
            "metadata": dict(sorted(self.metadata.items())),
            "modules": [module.to_dict() for module in self.modules],
            "ai_models": [model.to_dict() for model in self.ai_models],
            "ros_bridges": [bridge.to_dict() for bridge in self.ros_bridges],
            "startup_dependencies": [
                dependency.to_dict() for dependency in self.startup_dependencies
            ],
        }

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), indent=2, sort_keys=True)

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "NobroContractBundle":
        version = int(payload.get("schema_version", CONTRACT_SCHEMA_VERSION))
        if version != CONTRACT_SCHEMA_VERSION:
            raise ValueError(f"unsupported schema version: {version}")
        return cls(
            metadata={str(key): str(value) for key, value in payload.get("metadata", {}).items()},
            modules=tuple(
                ModuleSpec.from_dict(item) for item in payload.get("modules", ())
            ),
            ai_models=tuple(
                AiModelContract.from_dict(item) for item in payload.get("ai_models", ())
            ),
            ros_bridges=tuple(
                RosBridgeDescriptor.from_dict(item)
                for item in payload.get("ros_bridges", ())
            ),
            startup_dependencies=tuple(
                StartupDependency.from_dict(item)
                for item in payload.get("startup_dependencies", ())
            ),
        )

    @classmethod
    def from_json(cls, payload: str) -> "NobroContractBundle":
        return cls.from_dict(json.loads(payload))

    @classmethod
    def from_file(cls, path: str | Path) -> "NobroContractBundle":
        with Path(path).open("r", encoding="utf-8-sig") as handle:
            return cls.from_dict(json.load(handle))


def plan_startup(
    modules: tuple[ModuleSpec, ...] | list[ModuleSpec],
    dependencies: tuple[StartupDependency, ...] | list[StartupDependency] = (),
) -> StartupPlan:
    """Build a deterministic host-side startup order for contract review."""

    module_names = [module.module for module in modules]
    _validate_unique("module", module_names)
    _validate_unique(
        "startup dependency",
        [f"{dependency.module}->{dependency.depends_on}" for dependency in dependencies],
    )
    module_set = set(module_names)
    graph = {module: set[str]() for module in module_names}

    for dependency in dependencies:
        dependency.validate()
        if dependency.module not in module_set:
            raise ValueError(
                f"startup dependency references unknown module: {dependency.module}"
            )
        if dependency.depends_on not in module_set:
            raise ValueError(
                f"startup dependency references unknown module: {dependency.depends_on}"
            )
        graph[dependency.module].add(dependency.depends_on)

    order: list[str] = []
    ready = [module for module in module_names if not graph[module]]
    while ready:
        module = ready.pop(0)
        order.append(module)
        for candidate in module_names:
            if module not in graph[candidate]:
                continue
            graph[candidate].remove(module)
            if not graph[candidate] and candidate not in order and candidate not in ready:
                ready.append(candidate)

    if len(order) != len(module_names):
        cycle = [module for module in module_names if graph[module]]
        raise ValueError(f"startup dependency cycle: {', '.join(cycle)}")

    return StartupPlan(tuple(order))


def _validate_name(value: str) -> None:
    if not value or value.strip() != value:
        raise ValueError("names must be non-empty and trimmed")


def _validate_positive(name: str, value: int) -> None:
    if value <= 0:
        raise ValueError(f"{name} must be positive")


def _validate_non_negative(name: str, value: int) -> None:
    if value < 0:
        raise ValueError(f"{name} cannot be negative")


def _validate_unique(kind: str, values: list[str]) -> None:
    seen: set[str] = set()
    for value in values:
        if value in seen:
            raise ValueError(f"duplicate {kind}: {value}")
        seen.add(value)


def _validate_capability_ownership(modules: tuple[ModuleSpec, ...]) -> None:
    kernel_capabilities = _kernel_capabilities()
    owned_by: dict[Capability, str] = {}
    for module in modules:
        for capability in module.owns:
            if (
                module.criticality <= Criticality.USER
                and capability in kernel_capabilities
            ):
                raise ValueError(
                    f"user module {module.module} cannot own kernel capability "
                    f"{capability.name.lower()}"
                )
            owner = owned_by.get(capability)
            if owner is not None:
                raise ValueError(
                    f"duplicate capability owner for {capability.name.lower()}: "
                    f"{owner}, {module.module}"
                )
            owned_by[capability] = module.module

    provided = set(owned_by) | kernel_capabilities
    for module in modules:
        missing = [capability for capability in module.requires if capability not in provided]
        if missing:
            labels = ", ".join(capability.name.lower() for capability in missing)
            raise ValueError(f"module {module.module} requires unowned capability: {labels}")


def _kernel_capabilities() -> set[Capability]:
    return {
        Capability.TIMEBASE,
        Capability.DEADLINE_TIMER,
        Capability.SAMPLE_POOL,
        Capability.HOST_REPORT,
    }
