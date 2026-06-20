"""Deterministic host-side simulation helpers for NobroRTOS tooling."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Any

from .contracts import Criticality, MemoryBudget, ModuleSpec


class SensorStubMode(str, Enum):
    """Fault modes mirrored from the Rust sensor-stub adapter."""

    NOMINAL = "nominal"
    SILENT = "silent"
    ERROR_EVERY = "error_every"
    BAD_DATA_EVERY = "bad_data_every"


class SensorStubError(RuntimeError):
    """Raised when the simulated sensor adapter injects a fault."""


class ServoSimulatorError(RuntimeError):
    """Raised when the simulated actuator contract is violated."""


class WatchdogSimulatorError(RuntimeError):
    """Raised when the simulated watchdog contract is violated."""


class QuotaLedgerSimulatorError(RuntimeError):
    """Raised when the simulated quota ledger contract is violated."""


class DegradePlannerSimulatorError(RuntimeError):
    """Raised when the simulated degraded-mode planner cannot fit a profile."""


DEFAULT_DEADLINE_PERIOD_US = 20_000
DEFAULT_JITTER_TOLERANCE_US = 10


class RecoveryAction(str, Enum):
    """Recovery actions mirrored from the Rust kernel action model."""

    RETRY_NOW = "retry_now"
    RETRY_DELAY = "retry_delay"
    NOTIFY_USER_TASK = "notify_user_task"
    REBOOT_MODULE = "reboot_module"
    IGNORE = "ignore"


class KernelErrorKind(str, Enum):
    """Kernel error labels used by host-side recovery simulations."""

    LEASE_CONFLICT = "lease_conflict"
    BUS_TIMEOUT = "bus_timeout"
    RADIO_TX_FAIL = "radio_tx_fail"
    SENSOR_READ_FAIL = "sensor_read_fail"
    DEADLINE_MISSED = "deadline_missed"


@dataclass(frozen=True)
class ResourceBudget:
    """Zero-valid resource budget used by host-side runtime simulations."""

    flash_bytes: int = 0
    ram_bytes: int = 0
    pool_slots: int = 0

    def __post_init__(self) -> None:
        for field_name in ("flash_bytes", "ram_bytes", "pool_slots"):
            if getattr(self, field_name) < 0:
                raise ValueError(f"{field_name} must be non-negative")

    @classmethod
    def from_memory(cls, memory: MemoryBudget) -> "ResourceBudget":
        return cls(memory.flash_bytes, memory.ram_bytes, memory.pool_slots)

    def fits_within(self, limit: "ResourceBudget") -> bool:
        return (
            self.flash_bytes <= limit.flash_bytes
            and self.ram_bytes <= limit.ram_bytes
            and self.pool_slots <= limit.pool_slots
        )

    def checked_add(self, other: "ResourceBudget") -> "ResourceBudget":
        result = ResourceBudget(
            self.flash_bytes + other.flash_bytes,
            self.ram_bytes + other.ram_bytes,
            self.pool_slots + other.pool_slots,
        )
        if result.flash_bytes > 0xFFFF_FFFF:
            raise QuotaLedgerSimulatorError("flash quota overflow")
        if result.ram_bytes > 0xFFFF_FFFF:
            raise QuotaLedgerSimulatorError("RAM quota overflow")
        if result.pool_slots > 0xFFFF:
            raise QuotaLedgerSimulatorError("pool quota overflow")
        return result

    def checked_sub(self, other: "ResourceBudget") -> "ResourceBudget":
        if (
            other.flash_bytes > self.flash_bytes
            or other.ram_bytes > self.ram_bytes
            or other.pool_slots > self.pool_slots
        ):
            raise QuotaLedgerSimulatorError("quota release underflow")
        return ResourceBudget(
            self.flash_bytes - other.flash_bytes,
            self.ram_bytes - other.ram_bytes,
            self.pool_slots - other.pool_slots,
        )

    def to_dict(self) -> dict[str, int]:
        return {
            "flash_bytes": self.flash_bytes,
            "ram_bytes": self.ram_bytes,
            "pool_slots": self.pool_slots,
        }


@dataclass(frozen=True)
class QuotaEntry:
    """A host-side quota entry with fixed module identity and usage."""

    module: str
    limit: ResourceBudget
    used: ResourceBudget = ResourceBudget()

    def to_dict(self) -> dict[str, object]:
        return {
            "module": self.module,
            "limit": self.limit.to_dict(),
            "used": self.used.to_dict(),
            "available": self.limit.checked_sub(self.used).to_dict(),
        }


@dataclass
class QuotaLedgerSimulator:
    """Fixed-capacity resource quota ledger for host-side admission drills."""

    capacity: int = 8

    def __post_init__(self) -> None:
        if self.capacity <= 0:
            raise ValueError("capacity must be positive")
        self._entries: dict[str, QuotaEntry] = {}

    @property
    def len(self) -> int:
        return len(self._entries)

    @property
    def is_empty(self) -> bool:
        return self.len == 0

    def register(self, module: str, limit: ResourceBudget | MemoryBudget) -> None:
        if module in self._entries:
            raise QuotaLedgerSimulatorError("duplicate quota module")
        if len(self._entries) >= self.capacity:
            raise QuotaLedgerSimulatorError("quota capacity exhausted")
        self._entries[module] = QuotaEntry(module, _resource_budget(limit))

    def register_modules(self, modules: tuple[ModuleSpec, ...] | list[ModuleSpec]) -> None:
        for spec in modules:
            self.register(spec.module, ResourceBudget.from_memory(spec.memory))

    def reserve(self, module: str, amount: ResourceBudget) -> None:
        entry = self._entry(module)
        used = entry.used.checked_add(amount)
        if not used.fits_within(entry.limit):
            raise QuotaLedgerSimulatorError("quota limit exceeded")
        self._entries[module] = QuotaEntry(module, entry.limit, used)

    def release(self, module: str, amount: ResourceBudget) -> None:
        entry = self._entry(module)
        self._entries[module] = QuotaEntry(
            module,
            entry.limit,
            entry.used.checked_sub(amount),
        )

    def reset_usage(self, module: str) -> ResourceBudget:
        entry = self._entry(module)
        self._entries[module] = QuotaEntry(module, entry.limit)
        return entry.used

    def usage(self, module: str) -> ResourceBudget | None:
        entry = self._entries.get(module)
        return None if entry is None else entry.used

    def limit(self, module: str) -> ResourceBudget | None:
        entry = self._entries.get(module)
        return None if entry is None else entry.limit

    def available(self, module: str) -> ResourceBudget | None:
        entry = self._entries.get(module)
        return None if entry is None else entry.limit.checked_sub(entry.used)

    def total_used(self) -> ResourceBudget:
        total = ResourceBudget()
        for entry in self._entries.values():
            total = total.checked_add(entry.used)
        return total

    def entries(self) -> tuple[QuotaEntry, ...]:
        return tuple(self._entries.values())

    def to_dict(self) -> dict[str, object]:
        return {
            "capacity": self.capacity,
            "len": self.len,
            "total_used": self.total_used().to_dict(),
            "entries": [entry.to_dict() for entry in self.entries()],
        }

    def _entry(self, module: str) -> QuotaEntry:
        entry = self._entries.get(module)
        if entry is None:
            raise QuotaLedgerSimulatorError("missing quota module")
        return entry


@dataclass(frozen=True)
class SystemProfile:
    """Host-side system profile used by the degraded-mode planner."""

    flash_limit_bytes: int
    ram_limit_bytes: int
    pool_slot_limit: int
    max_modules: int

    def __post_init__(self) -> None:
        if self.flash_limit_bytes < 0 or self.ram_limit_bytes < 0:
            raise ValueError("profile byte limits must be non-negative")
        if self.pool_slot_limit < 0:
            raise ValueError("pool_slot_limit must be non-negative")
        if self.max_modules < 0:
            raise ValueError("max_modules must be non-negative")

    @property
    def budget(self) -> ResourceBudget:
        return ResourceBudget(
            self.flash_limit_bytes,
            self.ram_limit_bytes,
            self.pool_slot_limit,
        )

    def to_dict(self) -> dict[str, int]:
        return {
            "flash_limit_bytes": self.flash_limit_bytes,
            "ram_limit_bytes": self.ram_limit_bytes,
            "pool_slot_limit": self.pool_slot_limit,
            "max_modules": self.max_modules,
        }


class DegradeReason(str, Enum):
    """Degraded-mode pressure labels mirrored from the Rust planner."""

    FLASH_BUDGET = "flash_budget"
    RAM_BUDGET = "ram_budget"
    POOL_BUDGET = "pool_budget"
    MODULE_LIMIT = "module_limit"


@dataclass(frozen=True)
class DegradeDecision:
    """A deterministic host-side degraded-mode planning result."""

    enabled: tuple[str, ...]
    disabled: tuple[str, ...]
    budget: ResourceBudget
    reason: DegradeReason | None = None

    @property
    def disabled_count(self) -> int:
        return len(self.disabled)

    def to_dict(self) -> dict[str, object]:
        return {
            "enabled": list(self.enabled),
            "disabled": list(self.disabled),
            "disabled_count": self.disabled_count,
            "budget": self.budget.to_dict(),
            "reason": None if self.reason is None else self.reason.value,
        }


class DegradePlannerSimulator:
    """Host-side mirror of the kernel degraded-mode module fitting policy."""

    @staticmethod
    def fit(
        modules: tuple[ModuleSpec, ...] | list[ModuleSpec],
        profile: SystemProfile,
        capacity: int | None = None,
    ) -> DegradeDecision:
        if capacity is not None and len(modules) > capacity:
            raise DegradePlannerSimulatorError("too many modules for planner capacity")

        enabled = [True for _ in modules]
        disabled: list[str] = []
        budget = _total_budget(modules, enabled)
        reason: DegradeReason | None = None

        while not budget.fits_within(profile.budget) or sum(enabled) > profile.max_modules:
            reason = _overflow_reason(budget, profile, sum(enabled))
            drop_idx = _pick_drop_candidate(modules, enabled)
            if drop_idx is None:
                raise DegradePlannerSimulatorError("essential modules exceed profile")

            enabled[drop_idx] = False
            disabled.append(modules[drop_idx].module)
            budget = _total_budget(modules, enabled)

        return DegradeDecision(
            enabled=tuple(spec.module for idx, spec in enumerate(modules) if enabled[idx]),
            disabled=tuple(disabled),
            budget=budget,
            reason=reason,
        )


def _resource_budget(value: ResourceBudget | MemoryBudget) -> ResourceBudget:
    if isinstance(value, ResourceBudget):
        return value
    return ResourceBudget.from_memory(value)


def _total_budget(
    modules: tuple[ModuleSpec, ...] | list[ModuleSpec],
    enabled: list[bool],
) -> ResourceBudget:
    total = ResourceBudget()
    for index, spec in enumerate(modules):
        if enabled[index]:
            total = total.checked_add(ResourceBudget.from_memory(spec.memory))
    return total


def _pick_drop_candidate(
    modules: tuple[ModuleSpec, ...] | list[ModuleSpec],
    enabled: list[bool],
) -> int | None:
    selected: int | None = None
    for index, spec in enumerate(modules):
        if not enabled[index] or spec.criticality >= Criticality.SYSTEM:
            continue
        if selected is None:
            selected = index
            continue
        current = modules[selected]
        if spec.criticality < current.criticality:
            selected = index
        elif (
            spec.criticality == current.criticality
            and spec.memory.flash_bytes > current.memory.flash_bytes
        ):
            selected = index
    return selected


def _overflow_reason(
    budget: ResourceBudget,
    profile: SystemProfile,
    modules: int,
) -> DegradeReason:
    if modules > profile.max_modules:
        return DegradeReason.MODULE_LIMIT
    if budget.flash_bytes > profile.flash_limit_bytes:
        return DegradeReason.FLASH_BUDGET
    if budget.ram_bytes > profile.ram_limit_bytes:
        return DegradeReason.RAM_BUDGET
    return DegradeReason.POOL_BUDGET


@dataclass(frozen=True)
class RecoveryDecision:
    """A deterministic recovery decision with health counter context."""

    module: str
    error: str
    action: RecoveryAction
    total_errors: int
    consecutive_errors: int
    now_us: int
    delay_us: int = 0

    @property
    def state(self) -> str:
        if self.action == RecoveryAction.REBOOT_MODULE:
            return "recovering"
        if self.action == RecoveryAction.NOTIFY_USER_TASK:
            return "degraded"
        return "running"

    def to_dict(self) -> dict[str, Any]:
        return {
            "module": self.module,
            "error": self.error,
            "action": self.action.value,
            "delay_us": self.delay_us,
            "state": self.state,
            "total_errors": self.total_errors,
            "consecutive_errors": self.consecutive_errors,
            "now_us": self.now_us,
        }


@dataclass
class RecoveryPolicySimulator:
    """Host-side mirror of health thresholds and default recovery actions."""

    notify_after: int = 3
    reboot_after: int = 8
    total_errors: int = 0
    consecutive_errors: int = 0
    last_seen_us: int = 0
    last_recovery_us: int = 0

    def __post_init__(self) -> None:
        if self.notify_after <= 0:
            raise ValueError("notify_after must be positive")
        if self.reboot_after <= 0:
            raise ValueError("reboot_after must be positive")
        if self.notify_after > self.reboot_after:
            raise ValueError("notify_after must be less than or equal to reboot_after")

    def record_ok(self, now_us: int) -> dict[str, int | str]:
        self.consecutive_errors = 0
        self.last_seen_us = int(now_us)
        return {
            "event": "ok",
            "state": "running",
            "consecutive_errors": self.consecutive_errors,
            "total_errors": self.total_errors,
            "now_us": self.last_seen_us,
        }

    def record_error(
        self, module: str, error: str | KernelErrorKind, now_us: int
    ) -> RecoveryDecision:
        error_kind = KernelErrorKind(error)
        self.total_errors += 1
        self.consecutive_errors += 1
        self.last_seen_us = int(now_us)

        if self.consecutive_errors >= self.reboot_after:
            action = RecoveryAction.REBOOT_MODULE
            delay_us = 0
            self.last_recovery_us = int(now_us)
        elif self.consecutive_errors >= self.notify_after:
            action = RecoveryAction.NOTIFY_USER_TASK
            delay_us = 0
        else:
            action, delay_us = default_recovery_action(error_kind)

        return RecoveryDecision(
            module=module,
            error=error_kind.value,
            action=action,
            delay_us=delay_us,
            total_errors=self.total_errors,
            consecutive_errors=self.consecutive_errors,
            now_us=int(now_us),
        )


def default_recovery_action(error: KernelErrorKind) -> tuple[RecoveryAction, int]:
    if error == KernelErrorKind.BUS_TIMEOUT:
        return RecoveryAction.RETRY_DELAY, 1000
    if error == KernelErrorKind.RADIO_TX_FAIL:
        return RecoveryAction.RETRY_DELAY, 1000
    if error == KernelErrorKind.DEADLINE_MISSED:
        return RecoveryAction.NOTIFY_USER_TASK, 0
    return RecoveryAction.IGNORE, 0


@dataclass(frozen=True)
class SchedulerStats:
    """A host-readable snapshot of deadline scheduler counters."""

    tick_count: int
    max_jitter_us: int
    deadline_misses: int
    jitter_tolerance_us: int

    def to_dict(self) -> dict[str, int]:
        return {
            "tick_count": self.tick_count,
            "max_jitter_us": self.max_jitter_us,
            "deadline_misses": self.deadline_misses,
            "jitter_tolerance_us": self.jitter_tolerance_us,
        }


@dataclass
class SchedulerSimulator:
    """Deterministic Python mirror of the kernel deadline tick counters."""

    deadline_period_us: int = DEFAULT_DEADLINE_PERIOD_US
    jitter_tolerance_us: int = DEFAULT_JITTER_TOLERANCE_US
    expected_next_us: int = 0
    max_jitter_us: int = 0
    tick_count: int = 0
    deadline_misses: int = 0

    def __post_init__(self) -> None:
        if self.deadline_period_us <= 0:
            raise ValueError("deadline_period_us must be positive")
        if self.jitter_tolerance_us < 0:
            raise ValueError("jitter_tolerance_us must be non-negative")

    def reset_stats(self) -> None:
        self.expected_next_us = 0
        self.max_jitter_us = 0
        self.tick_count = 0
        self.deadline_misses = 0
        self.jitter_tolerance_us = DEFAULT_JITTER_TOLERANCE_US

    def set_jitter_tolerance_us(self, tolerance_us: int) -> None:
        if tolerance_us < 0:
            raise ValueError("tolerance_us must be non-negative")
        self.jitter_tolerance_us = int(tolerance_us)

    def on_deadline_tick(self, now_us: int) -> SchedulerStats:
        now = int(now_us) & 0xFFFF_FFFF
        expected = self.expected_next_us & 0xFFFF_FFFF
        if expected != 0:
            late = (now - expected) & 0xFFFF_FFFF
            early = (expected - now) & 0xFFFF_FFFF
            jitter = min(late, early)
            self.max_jitter_us = max(self.max_jitter_us, jitter)
            if jitter > self.jitter_tolerance_us:
                self.deadline_misses += 1

        self.expected_next_us = (now + self.deadline_period_us) & 0xFFFF_FFFF
        self.tick_count += 1
        return self.stats()

    def stats(self) -> SchedulerStats:
        return SchedulerStats(
            tick_count=self.tick_count,
            max_jitter_us=self.max_jitter_us,
            deadline_misses=self.deadline_misses,
            jitter_tolerance_us=self.jitter_tolerance_us,
        )


class EventSeverity(str, Enum):
    """Event severity labels mirrored from the Rust event log."""

    TRACE = "trace"
    INFO = "info"
    WARN = "warn"
    ERROR = "error"
    FATAL = "fatal"

    @property
    def code(self) -> int:
        return {
            EventSeverity.TRACE: 0,
            EventSeverity.INFO: 1,
            EventSeverity.WARN: 2,
            EventSeverity.ERROR: 3,
            EventSeverity.FATAL: 4,
        }[self]


class EventKind(str, Enum):
    """Event kind labels mirrored from the Rust event log."""

    BOOT = "boot"
    HEALTH = "health"
    RECOVERY = "recovery"
    TASK_OVERRUN = "task_overrun"
    LEASE = "lease"
    SAMPLE_POOL = "sample_pool"
    MANIFEST = "manifest"
    HOST = "host"


@dataclass(frozen=True)
class EventRecord:
    """A compact host-side event record with fixed numeric payload fields."""

    seq: int
    at_us: int
    module: str
    severity: EventSeverity
    kind: EventKind
    payload_kind: str = "none"
    payload0: int = 0
    payload1: int = 0

    def to_dict(self) -> dict[str, int | str]:
        return {
            "seq": self.seq,
            "at_us": self.at_us,
            "module": self.module,
            "severity": self.severity.value,
            "kind": self.kind.value,
            "payload_kind": self.payload_kind,
            "payload0": self.payload0,
            "payload1": self.payload1,
        }


@dataclass
class EventLogSimulator:
    """Fixed-capacity ring log for host-side diagnostics drills."""

    capacity: int = 8

    def __post_init__(self) -> None:
        if self.capacity < 0:
            raise ValueError("capacity must be non-negative")
        self._records: list[EventRecord | None] = [None] * self.capacity
        self._next = 0
        self._len = 0
        self._seq = 0
        self._dropped = 0

    @property
    def len(self) -> int:
        return self._len

    @property
    def dropped(self) -> int:
        return self._dropped

    @property
    def latest_sequence(self) -> int:
        return self._seq

    @property
    def remaining_capacity(self) -> int:
        return max(0, self.capacity - self._len)

    @property
    def is_full(self) -> bool:
        return self._len == self.capacity

    @property
    def has_dropped_events(self) -> bool:
        return self._dropped != 0

    def push(
        self,
        at_us: int,
        module: str,
        severity: str | EventSeverity,
        kind: str | EventKind,
        payload_kind: str = "none",
        payload0: int = 0,
        payload1: int = 0,
    ) -> EventRecord | None:
        record = EventRecord(
            seq=0,
            at_us=int(at_us),
            module=module,
            severity=EventSeverity(severity),
            kind=EventKind(kind),
            payload_kind=payload_kind,
            payload0=int(payload0),
            payload1=int(payload1),
        )
        if self.capacity == 0:
            self._dropped = min(self._dropped + 1, 0xFFFF_FFFF)
            return record

        self._seq = (self._seq + 1) & 0xFFFF_FFFF
        record = EventRecord(
            seq=self._seq,
            at_us=record.at_us,
            module=record.module,
            severity=record.severity,
            kind=record.kind,
            payload_kind=record.payload_kind,
            payload0=record.payload0,
            payload1=record.payload1,
        )
        overwritten = self._records[self._next]
        self._records[self._next] = record
        self._next = (self._next + 1) % self.capacity
        if self._len < self.capacity:
            self._len += 1
        else:
            self._dropped = min(self._dropped + 1, 0xFFFF_FFFF)
        return overwritten

    def latest(self) -> EventRecord | None:
        if self.capacity == 0 or self._len == 0:
            return None
        index = self.capacity - 1 if self._next == 0 else self._next - 1
        return self._records[index]

    def copy_recent(self, count: int) -> list[EventRecord]:
        if count < 0:
            raise ValueError("count must be non-negative")
        if self.capacity == 0 or self._len == 0 or count == 0:
            return []
        copied = min(count, self._len)
        start_age = self._len - copied
        recent: list[EventRecord] = []
        for age in range(start_age, self._len):
            index = (self._next + self.capacity - self._len + age) % self.capacity
            record = self._records[index]
            if record is not None:
                recent.append(record)
        return recent

    def count_at_or_above(self, severity: str | EventSeverity) -> int:
        threshold = EventSeverity(severity).code
        return sum(
            1
            for record in self._records
            if record is not None and record.severity.code >= threshold
        )

    def summary(self) -> dict[str, int | bool]:
        return {
            "len": self._len,
            "capacity": self.capacity,
            "remaining_capacity": self.remaining_capacity,
            "latest_sequence": self._seq,
            "dropped": self._dropped,
            "is_full": self.is_full,
            "has_dropped_events": self.has_dropped_events,
        }


@dataclass(frozen=True)
class RuntimeDrillResult:
    """Combined host-side drill result for runtime admission pressure."""

    profile: SystemProfile
    decision: DegradeDecision
    quota: dict[str, object]
    event_log: dict[str, object]
    recovery: tuple[RecoveryDecision, ...]

    def to_dict(self) -> dict[str, object]:
        return {
            "profile": self.profile.to_dict(),
            "decision": self.decision.to_dict(),
            "quota": self.quota,
            "event_log": self.event_log,
            "recovery": [decision.to_dict() for decision in self.recovery],
        }


@dataclass
class RuntimeDrillSimulator:
    """Compose planning, quota, event-log, and recovery checks for host CI."""

    modules: tuple[ModuleSpec, ...]
    profile: SystemProfile
    capacity: int = 8
    event_log_capacity: int = 8

    def __post_init__(self) -> None:
        if self.capacity <= 0:
            raise ValueError("capacity must be positive")
        if self.event_log_capacity < 0:
            raise ValueError("event_log_capacity must be non-negative")

    def run(
        self,
        quota_usage: dict[str, ResourceBudget] | None = None,
        fault_module: str = "sensor",
        fault_error: str | KernelErrorKind = KernelErrorKind.SENSOR_READ_FAIL,
        fault_count: int = 2,
    ) -> RuntimeDrillResult:
        if fault_count < 0:
            raise ValueError("fault_count must be non-negative")

        decision = DegradePlannerSimulator.fit(
            self.modules,
            self.profile,
            capacity=self.capacity,
        )
        ledger = QuotaLedgerSimulator(capacity=self.capacity)
        ledger.register_modules(self.modules)
        events = EventLogSimulator(capacity=self.event_log_capacity)
        recovery = RecoveryPolicySimulator(notify_after=2, reboot_after=4)

        events.push(0, "kernel", "info", "boot", "counter", len(self.modules))
        for module in decision.disabled:
            events.push(10, module, "warn", "recovery", "counter", 1)

        enabled = set(decision.enabled)
        usage = quota_usage
        if usage is None:
            usage = {
                spec.module: _default_runtime_usage(spec)
                for spec in self.modules
                if spec.module in enabled
            }
        for module, amount in usage.items():
            if module not in enabled:
                continue
            ledger.reserve(module, amount)
            events.push(20, module, "info", "sample_pool", "counter", amount.pool_slots)

        recovery_decisions: list[RecoveryDecision] = []
        if fault_module in enabled:
            error = KernelErrorKind(fault_error)
            for index in range(fault_count):
                now_us = 100 + index * 10
                fault = recovery.record_error(fault_module, error, now_us)
                recovery_decisions.append(fault)
                severity = "warn"
                if fault.action == RecoveryAction.REBOOT_MODULE:
                    severity = "error"
                events.push(
                    now_us,
                    fault_module,
                    severity,
                    "recovery",
                    "counter",
                    fault.consecutive_errors,
                )

        return RuntimeDrillResult(
            profile=self.profile,
            decision=decision,
            quota=ledger.to_dict(),
            event_log={
                **events.summary(),
                "warn_or_higher": events.count_at_or_above("warn"),
                "recent": [record.to_dict() for record in events.copy_recent(8)],
            },
            recovery=tuple(recovery_decisions),
        )


def _default_runtime_usage(spec: ModuleSpec) -> ResourceBudget:
    return ResourceBudget(
        max(1, spec.memory.flash_bytes // 4),
        max(1, spec.memory.ram_bytes // 4),
        min(1, spec.memory.pool_slots),
    )


@dataclass(frozen=True)
class WatchdogEntry:
    """A host-side liveness entry matching the Rust watchdog entry shape."""

    module: str
    timeout_us: int
    last_beat_us: int
    missed: int = 0

    def age_us(self, now_us: int) -> int:
        return max(0, int(now_us) - self.last_beat_us)

    def overdue_us(self, now_us: int) -> int:
        return max(0, self.age_us(now_us) - self.timeout_us)

    def is_expired(self, now_us: int) -> bool:
        return self.age_us(now_us) > self.timeout_us

    def to_dict(self) -> dict[str, int | str]:
        return {
            "module": self.module,
            "timeout_us": self.timeout_us,
            "last_beat_us": self.last_beat_us,
            "missed": self.missed,
        }


@dataclass
class WatchdogSimulator:
    """Deterministic Python twin of the kernel software heartbeat tracker."""

    capacity: int = 8

    def __post_init__(self) -> None:
        if self.capacity <= 0:
            raise ValueError("capacity must be positive")
        self._entries: dict[str, WatchdogEntry] = {}

    def register(self, module: str, timeout_us: int, now_us: int = 0) -> None:
        if timeout_us <= 0:
            raise WatchdogSimulatorError("timeout_us must be positive")
        if module in self._entries:
            raise WatchdogSimulatorError("duplicate watchdog module")
        if len(self._entries) >= self.capacity:
            raise WatchdogSimulatorError("watchdog capacity exhausted")
        self._entries[module] = WatchdogEntry(
            module=module,
            timeout_us=int(timeout_us),
            last_beat_us=int(now_us),
        )

    def beat(self, module: str, now_us: int) -> None:
        entry = self._entry(module)
        self._entries[module] = WatchdogEntry(
            module=entry.module,
            timeout_us=entry.timeout_us,
            last_beat_us=int(now_us),
            missed=0,
        )

    def expired(self, now_us: int, out_limit: int | None = None) -> list[WatchdogEntry]:
        if out_limit is not None and out_limit < 0:
            raise ValueError("out_limit must be non-negative")

        expired_entries: list[WatchdogEntry] = []
        for module, entry in list(self._entries.items()):
            if not entry.is_expired(now_us):
                continue
            updated = WatchdogEntry(
                module=entry.module,
                timeout_us=entry.timeout_us,
                last_beat_us=entry.last_beat_us,
                missed=min(entry.missed + 1, 0xFFFF_FFFF),
            )
            self._entries[module] = updated
            if out_limit is None or len(expired_entries) < out_limit:
                expired_entries.append(updated)
        return expired_entries

    def expired_count(self, now_us: int) -> int:
        return sum(1 for entry in self._entries.values() if entry.is_expired(now_us))

    def remove(self, module: str) -> WatchdogEntry | None:
        return self._entries.pop(module, None)

    def get(self, module: str) -> WatchdogEntry | None:
        return self._entries.get(module)

    def entries(self) -> tuple[WatchdogEntry, ...]:
        return tuple(self._entries.values())

    def _entry(self, module: str) -> WatchdogEntry:
        entry = self.get(module)
        if entry is None:
            raise WatchdogSimulatorError("missing watchdog module")
        return entry


@dataclass(frozen=True)
class ImuSample:
    """A small host-side IMU sample matching the Rust fixture's shape."""

    tick: int
    captured_us: int
    accel_g: tuple[float, float, float]
    gyro_dps: tuple[float, float, float]

    @property
    def accel_mag_sq(self) -> float:
        x, y, z = self.accel_g
        return x * x + y * y + z * z

    @property
    def plausible(self) -> bool:
        return 0.81 <= self.accel_mag_sq < 1.44

    def to_dict(self) -> dict[str, Any]:
        return {
            "tick": self.tick,
            "captured_us": self.captured_us,
            "accel_g": list(self.accel_g),
            "gyro_dps": list(self.gyro_dps),
            "plausible": self.plausible,
        }


@dataclass
class SensorStubSimulator:
    """Deterministic Python twin of the no-hardware Rust sensor-stub adapter."""

    sample_period_ticks: int = 50
    mode: SensorStubMode = SensorStubMode.NOMINAL
    fault_period: int = 0
    tick: int = 0

    def __post_init__(self) -> None:
        self.mode = SensorStubMode(self.mode)
        if self.sample_period_ticks <= 0:
            raise ValueError("sample_period_ticks must be positive")
        if self.fault_period < 0:
            raise ValueError("fault_period must be non-negative")

    @classmethod
    def nominal(cls, sample_period_ticks: int = 50) -> "SensorStubSimulator":
        return cls(sample_period_ticks=sample_period_ticks)

    @classmethod
    def silent(cls, sample_period_ticks: int = 50) -> "SensorStubSimulator":
        return cls(sample_period_ticks=sample_period_ticks, mode=SensorStubMode.SILENT)

    @classmethod
    def error_every(
        cls, fault_period: int, sample_period_ticks: int = 1
    ) -> "SensorStubSimulator":
        return cls(
            sample_period_ticks=sample_period_ticks,
            mode=SensorStubMode.ERROR_EVERY,
            fault_period=fault_period,
        )

    @classmethod
    def bad_data_every(
        cls, fault_period: int, sample_period_ticks: int = 1
    ) -> "SensorStubSimulator":
        return cls(
            sample_period_ticks=sample_period_ticks,
            mode=SensorStubMode.BAD_DATA_EVERY,
            fault_period=fault_period,
        )

    def poll(self, now_us: int | None = None) -> ImuSample | None:
        self.tick += 1
        captured_us = self.tick if now_us is None else int(now_us)

        if self.mode == SensorStubMode.SILENT:
            return None
        if self.mode == SensorStubMode.ERROR_EVERY and self._fault_tick():
            raise SensorStubError("injected sensor-stub fault")
        if self.tick % self.sample_period_ticks != 0:
            return None

        return self._sample(captured_us)

    def run(self, ticks: int, start_us: int = 0, step_us: int = 1) -> list[ImuSample]:
        samples: list[ImuSample] = []
        for index in range(ticks):
            sample = self.poll(start_us + index * step_us)
            if sample is not None:
                samples.append(sample)
        return samples

    def _sample(self, captured_us: int) -> ImuSample:
        if self.mode == SensorStubMode.BAD_DATA_EVERY and self._fault_tick():
            return ImuSample(
                tick=self.tick,
                captured_us=captured_us,
                accel_g=(4.0, 0.0, 0.0),
                gyro_dps=(0.0, 0.0, 0.0),
            )

        wobble = ((self.tick // self.sample_period_ticks) % 360) * 0.01
        return ImuSample(
            tick=self.tick,
            captured_us=captured_us,
            accel_g=(wobble, 0.0, 1.0),
            gyro_dps=(0.0, 0.0, wobble * 10.0),
        )

    def _fault_tick(self) -> bool:
        return self.fault_period > 0 and self.tick % self.fault_period == 0


@dataclass(frozen=True)
class ServoCommand:
    """A host-side actuator command record with deadline and readback checks."""

    channel: int
    pulse_us: int
    issued_at_us: int
    deadline_us: int
    readback_us: int
    deadline_met: bool
    readback_tolerance_us: int

    @property
    def readback_delta_us(self) -> int:
        return abs(self.pulse_us - self.readback_us)

    @property
    def readback_ok(self) -> bool:
        return self.readback_delta_us <= self.readback_tolerance_us

    @property
    def accepted(self) -> bool:
        return self.deadline_met and self.readback_ok

    def to_dict(self) -> dict[str, Any]:
        return {
            "channel": self.channel,
            "pulse_us": self.pulse_us,
            "issued_at_us": self.issued_at_us,
            "deadline_us": self.deadline_us,
            "readback_us": self.readback_us,
            "deadline_met": self.deadline_met,
            "readback_delta_us": self.readback_delta_us,
            "readback_ok": self.readback_ok,
            "accepted": self.accepted,
        }


@dataclass
class ServoSimulator:
    """Deterministic Python twin of the RoboServo-style actuator contract."""

    min_us: int = 500
    max_us: int = 2500
    center_us: int = 1500
    readback_offset_us: int = 0
    readback_tolerance_us: int = 50
    active_pulse_us: int = 1500
    attached: bool = False

    def __post_init__(self) -> None:
        if self.min_us > self.max_us:
            raise ValueError("min_us must be less than or equal to max_us")
        if self.readback_tolerance_us < 0:
            raise ValueError("readback_tolerance_us must be non-negative")
        self.attach_50hz(self.center_us)

    def attach_50hz(self, center_us: int | None = None) -> None:
        center = self.center_us if center_us is None else int(center_us)
        self._validate_pulse(center)
        self.center_us = center
        self.active_pulse_us = center
        self.attached = True

    def set_duty_us(
        self,
        channel: int,
        pulse_us: int,
        deadline_us: int,
        issued_at_us: int | None = None,
    ) -> ServoCommand:
        if channel != 0:
            raise ServoSimulatorError("invalid servo channel")
        self._validate_pulse(pulse_us)
        issued = 0 if issued_at_us is None else int(issued_at_us)
        deadline = int(deadline_us)
        self.active_pulse_us = int(pulse_us)
        return ServoCommand(
            channel=channel,
            pulse_us=int(pulse_us),
            issued_at_us=issued,
            deadline_us=deadline,
            readback_us=int(pulse_us) + self.readback_offset_us,
            deadline_met=issued <= deadline,
            readback_tolerance_us=self.readback_tolerance_us,
        )

    def sweep(
        self,
        start_us: int = 1200,
        stop_us: int = 1800,
        step_us: int = 30,
        deadline_spacing_us: int = 20_000,
    ) -> list[ServoCommand]:
        if step_us <= 0:
            raise ValueError("step_us must be positive")
        commands: list[ServoCommand] = []
        pulse = int(start_us)
        index = 0
        while pulse <= stop_us:
            issued_at_us = index * deadline_spacing_us
            commands.append(
                self.set_duty_us(
                    0,
                    pulse,
                    deadline_us=issued_at_us + deadline_spacing_us,
                    issued_at_us=issued_at_us,
                )
            )
            pulse += step_us
            index += 1
        return commands

    def _validate_pulse(self, pulse_us: int) -> None:
        if pulse_us < self.min_us or pulse_us > self.max_us:
            raise ServoSimulatorError("pulse out of range")
