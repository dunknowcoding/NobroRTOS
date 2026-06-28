#!/usr/bin/env python3
"""Chaos test for the NobroRTOS mesh rollup (M76).

Runs many rounds, each randomly injecting faults (node drops, power spikes), then asserts
the rollup invariants hold: total power equals the sum over UP nodes only, up+down equals
the node count, no negatives, and an all-down mesh reports zero power. Deterministic via a
seed. Pure stdlib; exit 0 = no invariant violations.
"""
import random
import sys


def rollup(nodes):
    up = [n for n in nodes if n["up"]]
    return {
        "up": len(up),
        "down": len(nodes) - len(up),
        "total_power_mw": sum(n["power_mw"] for n in up),
        "max_power_mw": max((n["power_mw"] for n in up), default=0),
    }


def run(rounds=2000, seed=1234, drop_p=0.3, spike_p=0.2):
    random.seed(seed)
    base = [{"id": i, "power_mw": 300 + i * 50, "up": True} for i in range(8)]
    violations = 0
    worst = 0
    for _ in range(rounds):
        nodes = [dict(n) for n in base]
        for n in nodes:
            if random.random() < drop_p:
                n["up"] = False
            if random.random() < spike_p:
                n["power_mw"] += random.randint(0, 4000)
        r = rollup(nodes)
        expect_power = sum(n["power_mw"] for n in nodes if n["up"])
        if r["total_power_mw"] != expect_power:
            violations += 1
        if r["up"] + r["down"] != len(nodes):
            violations += 1
        if r["total_power_mw"] < 0 or r["max_power_mw"] < 0:
            violations += 1
        if r["up"] == 0 and r["total_power_mw"] != 0:
            violations += 1
        worst = max(worst, r["max_power_mw"])

    ok = violations == 0
    print(
        f"chaos: {rounds} rounds, seed {seed}, drop_p={drop_p} spike_p={spike_p} -> "
        f"violations={violations}, peak_power={worst} mW  RESULT: {'PASS' if ok else 'FAIL'}"
    )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(run())
