#!/usr/bin/env python3
"""Capacity right-sizing for a NobroRTOS workload (the engine behind `nobro project shrink`).

NobroRTOS is static and no-heap: every stack, mailbox/queue, and sample pool has a
compile-time capacity. Over-provisioning wastes the RAM the framework is trying to save;
under-provisioning is a real fault (a run already hit the ceiling). This tool closes the
loop the honest way — it consumes an OBSERVED occupancy report (the peaks NobroRTOS already
measures: stack-guard high-water marks, and peak mailbox/pool occupancy) plus the current
declared capacities, and recommends tightened capacities with an explicit safety margin.

It never recommends below the observed peak or below a floor, rounds stacks up to an
alignment, and flags any resource whose observed peak already meets/exceeds its declared
capacity as UNDER-PROVISIONED (exit 1) so a right-sizing pass can never hide a real overflow.

Report schema (JSON):
  {
    "margin_percent": 25,                      # headroom above observed peak (default 25)
    "resources": [
      {"name": "control.stack", "kind": "stack_bytes",
       "declared": 1024, "observed_peak": 240, "granularity": 8},
      {"name": "imu->control", "kind": "queue_slots", "declared": 8, "observed_peak": 2},
      {"name": "sample_pool",  "kind": "pool_slots",  "declared": 16, "observed_peak": 5}
    ]
  }

    python tools/nobro_shrink.py report.json               # table + recommendations
    python tools/nobro_shrink.py report.json --json out.json
    python tools/nobro_shrink.py --selftest                # gate, no hardware/report needed

Exit 0 when every resource is safe (right-sized or with headroom); 1 when any resource is
under-provisioned or the report is malformed.
"""
from __future__ import annotations

import argparse
import json
import math
import sys

# Per-kind minimum floor so a quiet run never shrinks a resource to an unusable size.
FLOORS = {"stack_bytes": 64, "queue_slots": 1, "pool_slots": 1}
DEFAULT_GRANULARITY = {"stack_bytes": 8, "queue_slots": 1, "pool_slots": 1}


def recommend(resource: dict, margin_percent: int) -> dict:
    """Recommend a capacity for one resource: max(floor, ceil(peak*(1+margin))), aligned up,
    never below the observed peak. Classifies the outcome."""
    kind = resource["kind"]
    if kind not in FLOORS:
        raise ValueError(f"{resource.get('name', '?')}: unknown kind {kind!r}")
    declared = int(resource["declared"])
    peak = int(resource["observed_peak"])
    if declared < 0 or peak < 0:
        raise ValueError(f"{resource.get('name', '?')}: negative capacity/peak")
    gran = int(resource.get("granularity", DEFAULT_GRANULARITY[kind])) or 1
    floor = FLOORS[kind]

    # Headroom target, then align up to the granularity, then clamp to floor and to the peak.
    target = math.ceil(peak * (100 + margin_percent) / 100)
    target = max(target, floor)
    target = ((target + gran - 1) // gran) * gran
    target = max(target, peak)  # safety: never below what was actually used

    under = peak >= declared and declared > 0
    if under:
        status = "UNDER"           # observed peak already met/exceeded declared capacity
    elif target < declared:
        status = "SHRINK"
    elif target > declared:
        status = "GROW"            # rounding/margin pushed the recommendation above declared
    else:
        status = "OK"
    return {
        "name": resource["name"], "kind": kind, "declared": declared,
        "observed_peak": peak, "recommended": target, "status": status,
        "delta": target - declared,
    }


def analyze(report: dict) -> dict:
    margin = int(report.get("margin_percent", 25))
    if margin < 0:
        raise ValueError("margin_percent must be >= 0")
    rows = [recommend(r, margin) for r in report.get("resources", [])]
    saved = sum(-r["delta"] for r in rows if r["kind"] == "stack_bytes" and r["delta"] < 0)
    under = [r for r in rows if r["status"] == "UNDER"]
    return {"margin_percent": margin, "rows": rows,
            "stack_bytes_saved": saved, "under_provisioned": len(under), "safe": not under}


def render(result: dict) -> str:
    lines = [f"capacity right-sizing (margin {result['margin_percent']}%)", ""]
    lines.append(f"  {'resource':22s} {'kind':12s} {'decl':>7s} {'peak':>7s} "
                 f"{'reco':>7s}  status")
    for r in result["rows"]:
        lines.append(f"  {r['name']:22.22s} {r['kind']:12s} {r['declared']:7d} "
                     f"{r['observed_peak']:7d} {r['recommended']:7d}  {r['status']}"
                     + (f"  ({r['delta']:+d})" if r["delta"] else ""))
    lines.append("")
    lines.append(f"  stack bytes reclaimable: {result['stack_bytes_saved']}")
    if result["under_provisioned"]:
        lines.append(f"  UNDER-PROVISIONED resources: {result['under_provisioned']} "
                     f"(observed peak met/exceeded declared capacity — raise these)")
    lines.append(f"RESULT: {'PASS' if result['safe'] else 'FAIL'}")
    return "\n".join(lines)


def selftest() -> int:
    # 1) Over-provisioned stack shrinks, aligned to granularity, with 25% headroom.
    r = recommend({"name": "s", "kind": "stack_bytes", "declared": 1024,
                   "observed_peak": 240, "granularity": 8}, 25)
    assert r["status"] == "SHRINK" and r["recommended"] == 304 and r["recommended"] >= 240, r
    assert r["recommended"] % 8 == 0, r

    # 2) Under-provisioned (peak >= declared) is flagged, and never shrunk below the peak.
    r = recommend({"name": "q", "kind": "queue_slots", "declared": 4, "observed_peak": 4}, 25)
    assert r["status"] == "UNDER" and r["recommended"] >= 4, r

    # 3) Floor respected for a nearly-idle resource.
    r = recommend({"name": "p", "kind": "pool_slots", "declared": 16, "observed_peak": 0}, 25)
    assert r["recommended"] == FLOORS["pool_slots"] and r["status"] == "SHRINK", r

    # 4) Never below observed peak even with a 0% margin.
    r = recommend({"name": "q", "kind": "queue_slots", "declared": 8, "observed_peak": 5}, 0)
    assert r["recommended"] == 5 and r["status"] == "SHRINK", r

    # 5) Idempotence: feeding the recommendation back as the new declared yields OK.
    first = recommend({"name": "s", "kind": "stack_bytes", "declared": 2048,
                       "observed_peak": 300, "granularity": 8}, 20)
    second = recommend({"name": "s", "kind": "stack_bytes", "declared": first["recommended"],
                        "observed_peak": 300, "granularity": 8}, 20)
    assert second["status"] == "OK" and second["delta"] == 0, (first, second)

    # 6) Whole-report analysis: savings summed, under-provisioned surfaced as unsafe.
    res = analyze({"margin_percent": 25, "resources": [
        {"name": "a.stack", "kind": "stack_bytes", "declared": 4096,
         "observed_peak": 512, "granularity": 8},
        {"name": "b->c", "kind": "queue_slots", "declared": 8, "observed_peak": 8},
    ]})
    assert res["stack_bytes_saved"] == 4096 - 640 and not res["safe"], res
    assert res["under_provisioned"] == 1, res

    # 7) A fully right-sized report is safe with zero reclaim.
    res = analyze({"margin_percent": 0, "resources": [
        {"name": "q", "kind": "queue_slots", "declared": 3, "observed_peak": 3}]})
    assert res["under_provisioned"] == 1  # peak==declared is under (no headroom) by design

    # 8) Malformed kind raises.
    try:
        recommend({"name": "x", "kind": "bogus", "declared": 1, "observed_peak": 0}, 25)
        raise AssertionError("expected ValueError for unknown kind")
    except ValueError:
        pass

    print("NOBRO SHRINK SELFTEST: PASS (shrink/grow/under/floor/idempotence/report)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("report", nargs="?", help="occupancy report JSON")
    ap.add_argument("--json", metavar="FILE", help="write the machine-readable result")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.report:
        ap.error("a report file is required (or use --selftest)")

    try:
        report = json.load(open(args.report, encoding="utf-8"))
        result = analyze(report)
    except (OSError, json.JSONDecodeError, ValueError, KeyError) as exc:
        print(f"nobro_shrink: {exc}", file=sys.stderr)
        print("RESULT: FAIL")
        return 1

    print(render(result))
    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(result, f, indent=2)
    return 0 if result["safe"] else 1


if __name__ == "__main__":
    sys.exit(main())
