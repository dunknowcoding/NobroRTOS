"""Deterministic host-side simulation helpers for NobroRTOS tooling."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Any


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
