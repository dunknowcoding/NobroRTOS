"""Small Python authoring and deterministic simulation API for NobroRTOS apps.

Python declares the graph and may attach host-only test callbacks. Exported JSON
contains timing and topology only; firmware generated from it is native code.
"""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import re
from typing import Callable, Mapping

from .diagnostics import diagnostic

APP_SCHEMA = "nobro-app-v1"
APP_SCHEMA_ALIASES = {"nobro-python-app-v1"}
MAX_TASKS = 8
MAX_WIRES = 8
MAX_SIMULATION_EVENTS = 100_000
MAX_WRAP_SAFE_INTERVAL_US = 0x7FFF_FFFF

_NAME = re.compile(r"^[a-z][a-z0-9_-]{0,47}$")
_BOARDS = {
    "nrf52840-s140": ("s140", 128 * 1024, 32 * 1024),
    "nrf52840-nosd": ("nosd", 128 * 1024, 32 * 1024),
}
_ROLES = {
    "periodic": ("driver", 1024, 256, 10),
    "control": ("hard_realtime", 1024, 256, 10),
    "service": ("best_effort", 1024, 256, 10),
}
_ROLE_ALIASES = {"sensor": "periodic"}
_ROOT_KEYS = {"schema", "app", "board", "tasks", "wires"}
_TASK_KEYS = {
    "name",
    "role",
    "period_us",
    "phase_us",
    "deadline_us",
    "budget_us",
    "blocking_us",
    "flash_bytes",
    "ram_bytes",
}
_WIRE_KEYS = {"from", "to", "capacity"}


class AppDeclarationError(ValueError):
    """Raised when a Python-authored graph is invalid."""

    def __init__(self, key: str, detail: str = "") -> None:
        self.key = key
        self.code, self.summary = diagnostic(key)
        self.detail = detail
        suffix = f" {detail}" if detail else ""
        super().__init__(f"{self.code}: {self.summary}{suffix}")


class AppSimulationError(RuntimeError):
    """Raised when deterministic host simulation cannot continue safely."""


class AppCallbackError(AppSimulationError):
    """Raised when a host-only task callback fails."""


def HZ(rate: int) -> int:
    """Return an integer microsecond period for a positive frequency."""

    if isinstance(rate, bool) or not isinstance(rate, int) or rate <= 0:
        raise AppDeclarationError("app-period", "rate must be a positive integer")
    if rate > 1_000_000:
        raise AppDeclarationError("app-period", "rate exceeds one release per microsecond")
    return 1_000_000 // rate


@dataclass(frozen=True)
class TaskContext:
    """Context passed to a host-only simulated task callback."""

    app: str
    task: str
    now_us: int
    release: int


TaskStep = Callable[[TaskContext], None]


@dataclass(frozen=True)
class TaskDeclaration:
    """One periodic task declaration shared by export and simulation."""

    name: str
    role: str
    period_us: int
    phase_us: int
    deadline_us: int
    budget_us: int
    blocking_us: int
    flash_bytes: int
    ram_bytes: int
    step: TaskStep | None = None

    def to_dict(self) -> dict[str, int | str]:
        return {
            "name": self.name,
            "role": self.role,
            "period_us": self.period_us,
            "phase_us": self.phase_us,
            "deadline_us": self.deadline_us,
            "budget_us": self.budget_us,
            "blocking_us": self.blocking_us,
            "flash_bytes": self.flash_bytes,
            "ram_bytes": self.ram_bytes,
        }


@dataclass(frozen=True)
class WireDeclaration:
    """A bounded directed graph edge."""

    source: str
    destination: str
    capacity: int

    def to_dict(self) -> dict[str, int | str]:
        return {
            "from": self.source,
            "to": self.destination,
            "capacity": self.capacity,
        }


@dataclass(frozen=True)
class SimulationEvent:
    """One deterministic periodic release."""

    task: str
    at_us: int
    release: int


@dataclass(frozen=True)
class SimulationReport:
    """Bounded result returned by :meth:`NobroApp.simulate`."""

    app: str
    duration_us: int
    events: tuple[SimulationEvent, ...]
    runs: Mapping[str, int]

    @property
    def event_count(self) -> int:
        return len(self.events)

    def to_dict(self) -> dict[str, object]:
        return {
            "app": self.app,
            "duration_us": self.duration_us,
            "event_count": self.event_count,
            "runs": dict(self.runs),
            "events": [
                {"task": event.task, "at_us": event.at_us, "release": event.release}
                for event in self.events
            ],
        }


class NobroApp:
    """Declare, export, and deterministically simulate one small task graph."""

    def __init__(self, name: str, *, board: str = "nrf52840-nosd") -> None:
        _check_name(name, "app")
        if not isinstance(board, str) or board not in _BOARDS:
            raise AppDeclarationError(
                "app-target",
                f"unsupported board {board!r}; choose {', '.join(_BOARDS)}"
            )
        self.name = name
        self.board = board
        self._tasks: list[TaskDeclaration] = []
        self._wires: list[WireDeclaration] = []
        self._running = False

    @property
    def tasks(self) -> tuple[TaskDeclaration, ...]:
        return tuple(self._tasks)

    @property
    def wires(self) -> tuple[WireDeclaration, ...]:
        return tuple(self._wires)

    def task(
        self,
        name: str,
        period_us: int,
        step: TaskStep | None = None,
        *,
        role: str = "periodic",
        phase_us: int = 0,
        deadline_us: int | None = None,
        budget_us: int | None = None,
        blocking_us: int = 0,
        flash_bytes: int | None = None,
        ram_bytes: int | None = None,
    ) -> "NobroApp":
        """Add one periodic task and return this app for optional chaining."""

        self._require_mutable()
        _check_name(name, "task")
        period = _positive_interval(period_us, "period_us")
        if any(task.name == name for task in self._tasks):
            raise AppDeclarationError("app-duplicate-task", f"duplicate task: {name}")
        if len(self._tasks) >= MAX_TASKS:
            raise AppDeclarationError(
                "app-task-capacity", f"task capacity exceeds {MAX_TASKS}"
            )
        if not isinstance(role, str):
            raise AppDeclarationError("app-role", "role must be a string")
        role = _ROLE_ALIASES.get(role, role)
        if role not in _ROLES:
            raise AppDeclarationError(
                "app-role",
                f"unsupported role {role!r}; choose {', '.join(_ROLES)}"
            )
        phase = _integer(phase_us, "phase_us", minimum=0)
        if phase >= period:
            raise AppDeclarationError(
                "app-options", "phase_us must be below period_us"
            )
        deadline = period if deadline_us is None else _positive_interval(
            deadline_us, "deadline_us"
        )
        if deadline > period:
            raise AppDeclarationError(
                "app-options", "deadline_us exceeds period_us"
            )
        blocking = _integer(blocking_us, "blocking_us", minimum=0)
        _, default_flash, default_ram, divisor = _ROLES[role]
        budget = (
            min(deadline, max(1, period // divisor))
            if budget_us is None
            else _positive_interval(budget_us, "budget_us")
        )
        if budget + blocking > deadline:
            raise AppDeclarationError(
                "app-options", "budget_us + blocking_us exceeds deadline_us"
            )
        flash = (
            default_flash
            if flash_bytes is None
            else _integer(flash_bytes, "flash_bytes", minimum=1)
        )
        ram = (
            default_ram
            if ram_bytes is None
            else _integer(ram_bytes, "ram_bytes", minimum=1)
        )
        if step is not None and not callable(step):
            raise AppDeclarationError("app-shape", "step must be callable or None")
        self._tasks.append(
            TaskDeclaration(
                name,
                role,
                period,
                phase,
                deadline,
                budget,
                blocking,
                flash,
                ram,
                step,
            )
        )
        return self

    def wire(self, source: str, destination: str, capacity: int = 1) -> "NobroApp":
        """Add one directed edge and return this app for optional chaining."""

        self._require_mutable()
        _check_name(source, "wire source")
        _check_name(destination, "wire destination")
        if len(self._wires) >= MAX_WIRES:
            raise AppDeclarationError(
                "app-wire-capacity", f"wire count exceeds {MAX_WIRES}"
            )
        depth = _integer(capacity, "capacity", minimum=1, maximum=64)
        edge = (source, destination)
        if source == destination:
            raise AppDeclarationError("app-self-wire", "a task cannot wire to itself")
        if any((wire.source, wire.destination) == edge for wire in self._wires):
            raise AppDeclarationError(
                "app-duplicate-wire", f"duplicate wire: {source}->{destination}"
            )
        names = {task.name for task in self._tasks}
        if source not in names:
            raise AppDeclarationError(
                "app-endpoint", f"wire source references unknown task: {source}"
            )
        if destination not in names:
            raise AppDeclarationError(
                "app-endpoint",
                f"wire destination references unknown task: {destination}"
            )
        self._wires.append(WireDeclaration(source, destination, depth))
        return self

    def validate(self) -> None:
        """Fail closed if the complete graph is not firmware-authorable."""

        if not self._tasks:
            raise AppDeclarationError("app-empty", "at least one task is required")
        names = {task.name for task in self._tasks}
        for wire in self._wires:
            if wire.source not in names:
                raise AppDeclarationError(
                    "app-endpoint",
                    f"wire source references unknown task: {wire.source}"
                )
            if wire.destination not in names:
                raise AppDeclarationError(
                    "app-endpoint",
                    f"wire destination references unknown task: {wire.destination}"
                )

    def to_dict(self) -> dict[str, object]:
        """Return the strict, callback-free firmware authoring document."""

        self.validate()
        return {
            "schema": APP_SCHEMA,
            "app": self.name,
            "board": self.board,
            "tasks": [task.to_dict() for task in self._tasks],
            "wires": [wire.to_dict() for wire in self._wires],
        }

    def write_json(self, path: str | Path) -> Path:
        """Write canonical JSON for ``nobro firmware``."""

        destination = Path(path)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(
            json.dumps(self.to_dict(), indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        return destination

    @classmethod
    def from_dict(
        cls,
        value: object,
        *,
        steps: Mapping[str, TaskStep] | None = None,
    ) -> "NobroApp":
        """Validate and reconstruct one versioned Python app document."""

        root = _exact_object(value, _ROOT_KEYS, "app")
        schema = root["schema"]
        if not isinstance(schema, str) or (
            schema != APP_SCHEMA and schema not in APP_SCHEMA_ALIASES
        ):
            raise AppDeclarationError(
                "app-target",
                f"unsupported app schema {schema!r}; expected {APP_SCHEMA!r}"
            )
        if not isinstance(root["tasks"], list) or not isinstance(root["wires"], list):
            raise AppDeclarationError("app-shape", "tasks and wires must be arrays")
        app = cls(_text(root["app"], "app"), board=_text(root["board"], "board"))
        callbacks = {} if steps is None else dict(steps)
        for index, item in enumerate(root["tasks"]):
            task = _exact_object(item, _TASK_KEYS, f"tasks[{index}]")
            name = _text(task["name"], f"tasks[{index}].name")
            app.task(
                name,
                _integer(task["period_us"], "period_us", minimum=1),
                callbacks.pop(name, None),
                role=_text(task["role"], "role"),
                phase_us=_integer(task["phase_us"], "phase_us", minimum=0),
                deadline_us=_integer(task["deadline_us"], "deadline_us", minimum=1),
                budget_us=_integer(task["budget_us"], "budget_us", minimum=1),
                blocking_us=_integer(task["blocking_us"], "blocking_us", minimum=0),
                flash_bytes=_integer(task["flash_bytes"], "flash_bytes", minimum=1),
                ram_bytes=_integer(task["ram_bytes"], "ram_bytes", minimum=1),
            )
        if callbacks:
            raise AppDeclarationError(
                "app-endpoint",
                f"callbacks reference unknown tasks: {', '.join(sorted(callbacks))}"
            )
        for index, item in enumerate(root["wires"]):
            wire = _exact_object(item, _WIRE_KEYS, f"wires[{index}]")
            app.wire(
                _text(wire["from"], "wire.from"),
                _text(wire["to"], "wire.to"),
                _integer(wire["capacity"], "wire.capacity", minimum=1, maximum=64),
            )
        app.validate()
        return app

    @classmethod
    def read_json(
        cls,
        path: str | Path,
        *,
        steps: Mapping[str, TaskStep] | None = None,
    ) -> "NobroApp":
        """Read and validate canonical JSON without evaluating Python source."""

        try:
            value = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise AppDeclarationError(
                "app-shape", f"cannot read Python app JSON: {error}"
            ) from error
        return cls.from_dict(value, steps=steps)

    def firmware_spec(self) -> dict[str, object]:
        """Return the existing native firmware generator's admitted workload."""

        self.validate()
        _, flash_limit, ram_limit = _BOARDS[self.board]
        tasks = [
            {
                "name": "kernel",
                "criticality": "hard_realtime",
                "flash": 12 * 1024,
                "ram": 3 * 1024,
                "pool": 2,
                "phase_us": 0,
                "deadline_us": 20_000,
                "period_us": 20_000,
                "budget_us": 0,
            }
        ]
        tasks.extend(
            {
                "name": task.name,
                "role": task.role,
                "criticality": _ROLES[task.role][0],
                "flash": task.flash_bytes,
                "ram": task.ram_bytes,
                "period_us": task.period_us,
                "phase_us": task.phase_us,
                "deadline_us": task.deadline_us,
                "budget_us": task.budget_us,
                "blocking_us": task.blocking_us,
            }
            for task in self._tasks
        )
        return {
            "app": self.name,
            "board": self.board,
            "workload": {
                "profile": {
                    "flash": flash_limit,
                    "ram": ram_limit,
                    "pool": max(8, len(tasks) + 1),
                    "wake_latency_us": 0,
                },
                "tasks": tasks,
                "channels": [
                    [wire.source, wire.destination] for wire in self._wires
                ],
                "wire_capacities": [
                    [wire.source, wire.destination, wire.capacity]
                    for wire in self._wires
                ],
            },
            "user_lines": len(self._tasks) + len(self._wires) + 2,
        }

    def simulate(
        self,
        duration_us: int,
        *,
        max_events: int = 10_000,
    ) -> SimulationReport:
        """Run phase-ordered host callbacks over a bounded virtual timeline."""

        self.validate()
        duration = _integer(duration_us, "duration_us", minimum=0)
        limit = _integer(
            max_events,
            "max_events",
            minimum=1,
            maximum=MAX_SIMULATION_EVENTS,
        )
        if self._running:
            raise AppSimulationError("app simulation is already running")
        self._running = True
        events: list[SimulationEvent] = []
        runs = {task.name: 0 for task in self._tasks}
        next_release = [task.phase_us for task in self._tasks]
        try:
            while next_release:
                now = min(next_release)
                if now >= duration:
                    break
                for index, task in enumerate(self._tasks):
                    if next_release[index] != now:
                        continue
                    if len(events) >= limit:
                        raise AppSimulationError(
                            f"simulation event limit exceeded: {limit}"
                        )
                    release = runs[task.name]
                    context = TaskContext(self.name, task.name, now, release)
                    if task.step is not None:
                        try:
                            task.step(context)
                        except Exception as error:
                            raise AppCallbackError(
                                f"task {task.name!r} callback failed at {now} us: {error}"
                            ) from error
                    events.append(SimulationEvent(task.name, now, release))
                    runs[task.name] = release + 1
                    next_release[index] += task.period_us
        finally:
            self._running = False
        return SimulationReport(self.name, duration, tuple(events), runs)

    def run(
        self,
        duration_us: int,
        *,
        max_events: int = 10_000,
    ) -> SimulationReport:
        """Alias for :meth:`simulate`, matching the authoring vocabulary."""

        return self.simulate(duration_us, max_events=max_events)

    def _require_mutable(self) -> None:
        if self._running:
            raise AppSimulationError("cannot change an app while it is running")


def _check_name(value: object, label: str) -> None:
    if not isinstance(value, str) or not _NAME.fullmatch(value):
        raise AppDeclarationError(
            "app-name", f"{label} name must match [a-z][a-z0-9_-]{{0,47}}"
        )


def _integer(
    value: object,
    label: str,
    *,
    minimum: int,
    maximum: int | None = None,
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise AppDeclarationError("app-shape", f"{label} must be an integer")
    if value < minimum or (maximum is not None and value > maximum):
        suffix = (
            f"between {minimum} and {maximum}"
            if maximum is not None
            else f"at least {minimum}"
        )
        key = "app-period" if label in {"period_us", "deadline_us"} else "app-options"
        if label in {"capacity", "wire.capacity"}:
            key = "app-wire-capacity"
        raise AppDeclarationError(key, f"{label} must be {suffix}")
    return value


def _positive_interval(value: object, label: str) -> int:
    return _integer(value, label, minimum=1, maximum=MAX_WRAP_SAFE_INTERVAL_US)


def _text(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise AppDeclarationError("app-shape", f"{label} must be a string")
    return value


def _exact_object(
    value: object,
    expected: set[str],
    label: str,
) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise AppDeclarationError("app-shape", f"{label} must be an object")
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        details = []
        if missing:
            details.append(f"missing {', '.join(missing)}")
        if extra:
            details.append(f"unknown {', '.join(extra)}")
        raise AppDeclarationError(
            "app-shape", f"{label} fields: {'; '.join(details)}"
        )
    return value
