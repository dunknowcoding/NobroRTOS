#!/usr/bin/env python3
"""Admission cost analysis for a NobroRTOS workload.

Given a workload description (tasks with criticality / period / execution
budget / memory, plus a platform profile), this reports the MARGINAL cost of
every task — the flash / RAM / pool-slots / CPU-utilization each one adds —
whether the set fits both the memory budget and the 100% utilization bound,
and, when it does not, the smallest best-effort-first SHED PLAN to make it
schedulable. Deadline-critical work (System / HardRealtime) and the kernel are
never shed; if the critical core alone overflows the tool says INFEASIBLE
(cut budgets or move to a bigger profile) rather than dropping safety work.

This is the exact policy `nobro_kernel::admission_analysis` enforces on-device,
made runnable offline so a build gate or an operator can act on it before flash.

    python tools/nobro_admission.py workload.json
    python tools/nobro_admission.py workload.json --profile-ram 8192
    python tools/nobro_admission.py --selftest      # gate: reproduces the robotics verdicts

Exit 0 when the workload (after any suggested shedding) is schedulable, 1 when
it is infeasible or the file is malformed.
"""
import argparse
import json
import sys

# Criticality order must match nobro_kernel::Criticality.
CRITICALITY = {
    "best_effort": 0, "user": 1, "driver": 2, "system": 3, "hard_realtime": 4,
}
SHEDDABLE_BELOW = CRITICALITY["system"]  # system + hard_realtime are protected


def utilization_permyriad(task: dict) -> int:
    period = int(task.get("period_us", 0))
    budget = int(task.get("budget_us", 0))
    if period <= 0 or budget <= 0:
        return 0
    return (budget * 10_000) // period


def task_cost(task: dict) -> dict:
    return {
        "name": task["name"],
        "criticality": task.get("criticality", "best_effort"),
        "flash": int(task.get("flash", 0)),
        "ram": int(task.get("ram", 0)),
        "pool": int(task.get("pool", 0)),
        "util": utilization_permyriad(task),
    }


def totals(costs: list[dict]) -> dict:
    return {
        "flash": sum(c["flash"] for c in costs),
        "ram": sum(c["ram"] for c in costs),
        "pool": sum(c["pool"] for c in costs),
        "util": sum(c["util"] for c in costs),
    }


def fits(used: dict, profile: dict) -> bool:
    return (used["flash"] <= profile["flash"]
            and used["ram"] <= profile["ram"]
            and used["pool"] <= profile["pool"]
            and used["util"] <= 10_000)


def analyze(workload: dict) -> dict:
    profile = {
        "flash": int(workload["profile"]["flash"]),
        "ram": int(workload["profile"]["ram"]),
        "pool": int(workload["profile"].get("pool", 0)),
    }
    costs = [task_cost(task) for task in workload["tasks"]]
    used = totals(costs)
    schedulable = fits(used, profile)

    result = {
        "profile": profile,
        "used": used,
        "schedulable": schedulable,
        "headroom": {
            "flash": max(profile["flash"] - used["flash"], 0),
            "ram": max(profile["ram"] - used["ram"], 0),
            "util": max(10_000 - used["util"], 0),
        },
        "costs": costs,
    }
    if schedulable:
        result["shed_plan"] = {"shed": [], "freed": {"flash": 0, "ram": 0, "util": 0}}
        return result

    # Best-effort-first shed order: ascending criticality, then heaviest
    # utilization, then heaviest RAM. Kernel and >= system are never candidates.
    candidates = [
        c for c in costs
        if c["name"] != "kernel" and CRITICALITY.get(c["criticality"], 4) < SHEDDABLE_BELOW
    ]
    candidates.sort(key=lambda c: (CRITICALITY.get(c["criticality"], 4), -c["util"], -c["ram"]))

    shed: list[str] = []
    freed = {"flash": 0, "ram": 0, "util": 0}
    running = dict(used)
    for c in candidates:
        if fits(running, profile):
            break
        shed.append(c["name"])
        for key in ("flash", "ram", "util"):
            freed[key] += c[key]
        running["flash"] -= c["flash"]
        running["ram"] -= c["ram"]
        running["pool"] -= c["pool"]
        running["util"] -= c["util"]

    result["shed_plan"] = {
        "shed": shed,
        "freed": freed,
        "feasible": fits(running, profile),
    }
    return result


def render(result: dict) -> str:
    lines = ["task                 crit          flash    ram  pool   util%"]
    for c in result["costs"]:
        lines.append(f"{c['name']:20} {c['criticality']:12} {c['flash']:6} "
                     f"{c['ram']:6} {c['pool']:5} {c['util']/100:6.1f}")
    used, profile = result["used"], result["profile"]
    lines.append(f"{'TOTAL':20} {'':12} {used['flash']:6} {used['ram']:6} "
                 f"{used['pool']:5} {used['util']/100:6.1f}")
    lines.append(f"{'PROFILE':20} {'':12} {profile['flash']:6} {profile['ram']:6} "
                 f"{profile['pool']:5} {100.0:6.1f}")
    if result["schedulable"]:
        lines.append("VERDICT: SCHEDULABLE")
    else:
        plan = result["shed_plan"]
        if plan.get("feasible"):
            lines.append(f"VERDICT: OVER BUDGET -> shed best-effort first: "
                         f"{', '.join(plan['shed'])}  "
                         f"(frees flash={plan['freed']['flash']} "
                         f"ram={plan['freed']['ram']} util={plan['freed']['util']/100:.1f}%)")
        else:
            lines.append("VERDICT: INFEASIBLE -> the deadline-critical core alone "
                         "exceeds the profile; cut budgets or use a bigger profile")
    return "\n".join(lines)


def robotics_workload() -> dict:
    """Reference workload that mirrors the Rust graph declaration."""
    return {
        "profile": {"flash": 128 * 1024, "ram": 8 * 1024, "pool": 8},
        "tasks": [
            {"name": "kernel", "criticality": "hard_realtime",
             "flash": 12 * 1024, "ram": 3 * 1024, "pool": 2,
             "period_us": 20_000, "budget_us": 0},
            {"name": "motor", "criticality": "hard_realtime",
             "flash": 2 * 1024, "ram": 512, "period_us": 5_000, "budget_us": 400},
            {"name": "imu", "criticality": "system",
             "flash": 2 * 1024, "ram": 512, "pool": 1,
             "period_us": 10_000, "budget_us": 300},
            {"name": "radio", "criticality": "driver",
             "flash": 4 * 1024, "ram": 512, "period_us": 20_000, "budget_us": 800},
            {"name": "storage", "criticality": "driver",
             "flash": 3 * 1024, "ram": 512, "period_us": 50_000, "budget_us": 1_000},
            {"name": "audio", "criticality": "best_effort",
             "flash": 4 * 1024, "ram": 1024},
            {"name": "camera_ai", "criticality": "best_effort",
             "flash": 8 * 1024, "ram": 2 * 1024},
            {"name": "ota", "criticality": "best_effort",
             "flash": 4 * 1024, "ram": 512},
            {"name": "diagnostics", "criticality": "best_effort",
             "flash": 2 * 1024, "ram": 256},
        ],
    }


def selftest() -> int:
    # Roomy profile: schedulable, no shedding.
    roomy = robotics_workload()
    roomy["profile"]["ram"] = 64 * 1024
    result = analyze(roomy)
    assert result["schedulable"], "roomy profile should fit"

    # Tight RAM: sheds best-effort first, protects motor/imu, camera_ai goes first.
    result = analyze(robotics_workload())
    assert not result["schedulable"]
    plan = result["shed_plan"]
    assert plan["feasible"], "should be rescuable by shedding"
    assert "motor" not in plan["shed"] and "imu" not in plan["shed"]
    assert "kernel" not in plan["shed"]
    assert plan["shed"][0] == "camera_ai", "heaviest best-effort shed first"

    # Tiny profile: even the critical core overflows -> infeasible, nothing critical shed.
    tiny = robotics_workload()
    tiny["profile"] = {"flash": 4 * 1024, "ram": 1024, "pool": 1}
    result = analyze(tiny)
    assert not result["shed_plan"]["feasible"]

    # Utilization linearity + bound.
    def util_of(n):
        wl = {"profile": {"flash": 1 << 20, "ram": 1 << 20, "pool": 8},
              "tasks": [{"name": f"t{i}", "criticality": "driver",
                         "flash": 512, "ram": 128,
                         "period_us": 10_000, "budget_us": 1_000} for i in range(n)]}
        return analyze(wl)["used"]["util"]
    assert util_of(5) == 5_000 and util_of(9) == 9_000
    assert not analyze({"profile": {"flash": 1 << 20, "ram": 1 << 20, "pool": 8},
                        "tasks": [{"name": f"t{i}", "criticality": "driver",
                                   "flash": 512, "ram": 128, "period_us": 10_000,
                                   "budget_us": 1_000} for i in range(11)]})["schedulable"]
    print("NOBRO ADMISSION SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workload", nargs="?", help="workload JSON (tasks + profile)")
    parser.add_argument("--profile-ram", type=int, help="override the profile RAM budget")
    parser.add_argument("--profile-flash", type=int, help="override the profile flash budget")
    parser.add_argument("--json", action="store_true", help="emit the analysis as JSON")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.workload:
        parser.error("a workload JSON path is required (or pass --selftest)")

    try:
        workload = json.loads(open(args.workload, encoding="utf-8").read())
    except (OSError, ValueError) as error:
        print(f"cannot read workload: {error}")
        return 1
    if args.profile_ram is not None:
        workload["profile"]["ram"] = args.profile_ram
    if args.profile_flash is not None:
        workload["profile"]["flash"] = args.profile_flash

    result = analyze(workload)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(render(result))
    if result["schedulable"]:
        return 0
    return 0 if result["shed_plan"].get("feasible") else 1


if __name__ == "__main__":
    sys.exit(main())
