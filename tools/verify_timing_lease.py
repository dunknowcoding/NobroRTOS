#!/usr/bin/env python3
"""Finite-state timing and lease verifier for NobroRTOS software contracts.

This is a dependency-free model checker for the small state spaces that matter
before hardware validation:

- lease acquire/release/recovery cleanup over bounded owners and resources
- scheduler jitter accounting, including unsigned 32-bit time wraparound

It complements randomized Rust tests and can run anywhere Python is available.
"""

from __future__ import annotations

import argparse
import itertools
import json
import sys
from dataclasses import dataclass

U32_MASK = 0xFFFF_FFFF
DEADLINE_PERIOD_US = 20_000


@dataclass(frozen=True)
class LeaseResult:
    states_checked: int
    transitions_checked: int
    depth: int


def lease_step(state: tuple[int, ...], op: tuple[str, int, int]) -> tuple[tuple[int, ...], str]:
    kind, resource, owner = op
    slots = list(state)
    before = tuple(slots)
    if kind == "acquire":
        if slots[resource] != 0:
            return before, "already_held"
        slots[resource] = owner
        return tuple(slots), "ok"
    if kind == "release":
        if slots[resource] == 0:
            return before, "not_held"
        if slots[resource] != owner:
            return before, "wrong_owner"
        slots[resource] = 0
        return tuple(slots), "ok"
    if kind == "release_all":
        return tuple(0 if slot == owner else slot for slot in slots), "ok"
    raise ValueError(f"unknown lease op: {kind}")


def verify_leases(resources: int, owners: int, depth: int) -> LeaseResult:
    owner_ids = tuple(range(1, owners + 1))
    ops: list[tuple[str, int, int]] = []
    for resource in range(resources):
        for owner in owner_ids:
            ops.append(("acquire", resource, owner))
            ops.append(("release", resource, owner))
    for owner in owner_ids:
        ops.append(("release_all", 0, owner))

    initial = tuple(0 for _ in range(resources))
    states_checked = 0
    transitions_checked = 0
    seen_states = {initial}

    for length in range(depth + 1):
        for seq in itertools.product(ops, repeat=length):
            state = initial
            for op in seq:
                before = state
                state, result = lease_step(state, op)
                transitions_checked += 1
                assert all(0 <= slot <= owners for slot in state)
                kind, resource, owner = op
                if result in {"already_held", "not_held", "wrong_owner"}:
                    assert state == before
                if kind == "release_all":
                    assert owner not in state
                if kind == "release" and result == "ok":
                    assert state[resource] == 0
                if kind == "acquire" and result == "ok":
                    assert state[resource] == owner
            states_checked += 1
            seen_states.add(state)

    return LeaseResult(
        states_checked=len(seen_states),
        transitions_checked=transitions_checked,
        depth=depth,
    )


def wrapping_jitter(now: int, expected: int) -> int:
    late = (now - expected) & U32_MASK
    early = (expected - now) & U32_MASK
    return min(late, early)


@dataclass(frozen=True)
class TimingResult:
    sequences_checked: int
    ticks_checked: int
    max_jitter_us: int
    deadline_misses: int


def verify_timing(tolerance_us: int, jitter_span_us: int) -> TimingResult:
    offsets = range(-jitter_span_us, jitter_span_us + 1)
    bases = (1_000, U32_MASK - 5)
    sequences_checked = 0
    ticks_checked = 0
    global_max = 0
    global_misses = 0

    for base in bases:
        for seq in itertools.product(offsets, repeat=3):
            expected = None
            max_jitter = 0
            misses = 0
            for i, offset in enumerate(seq):
                now = (base + i * DEADLINE_PERIOD_US + offset) & U32_MASK
                if expected is not None:
                    jitter = wrapping_jitter(now, expected)
                    max_jitter = max(max_jitter, jitter)
                    if jitter > tolerance_us:
                        misses += 1
                expected = (now + DEADLINE_PERIOD_US) & U32_MASK
                ticks_checked += 1

            expected_jitters = [abs(seq[i] - seq[i - 1]) for i in range(1, len(seq))]
            assert max_jitter == max(expected_jitters, default=0)
            assert misses == sum(1 for jitter in expected_jitters if jitter > tolerance_us)
            global_max = max(global_max, max_jitter)
            global_misses += misses
            sequences_checked += 1

    return TimingResult(
        sequences_checked=sequences_checked,
        ticks_checked=ticks_checked,
        max_jitter_us=global_max,
        deadline_misses=global_misses,
    )


def run(args: argparse.Namespace) -> dict:
    leases = verify_leases(args.resources, args.owners, args.depth)
    timing = verify_timing(args.tolerance_us, args.jitter_span_us)
    return {
        "passing": True,
        "lease": leases.__dict__,
        "timing": timing.__dict__,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--resources", type=int, default=2)
    parser.add_argument("--owners", type=int, default=2)
    parser.add_argument("--depth", type=int, default=5)
    parser.add_argument("--tolerance-us", type=int, default=2)
    parser.add_argument("--jitter-span-us", type=int, default=3)
    args = parser.parse_args()

    if args.resources <= 0 or args.owners <= 0 or args.depth < 0:
        parser.error("resources and owners must be positive; depth must be non-negative")
    if args.tolerance_us < 0 or args.jitter_span_us < 0:
        parser.error("timing bounds must be non-negative")

    print(json.dumps(run(args), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
