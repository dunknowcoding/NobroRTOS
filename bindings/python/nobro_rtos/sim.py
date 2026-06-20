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
