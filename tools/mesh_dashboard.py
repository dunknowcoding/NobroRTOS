#!/usr/bin/env python3
"""Host health-monitoring dashboard for a NobroRTOS mesh (M75).

Reads node telemetry (the collector's JSON if a path is given, else a built-in sim) and
renders a health table: per-node status (OK / WARN / FAIL) by thresholds, plus a system
summary and the active alerts. Pure stdlib; exit 0 = no FAIL nodes.
"""
import json
import sys
import time

POWER_WARN_MW = 1500
POWER_FAIL_MW = 3000
STALE_S = 5.0


def health(node, now):
    if not node.get("up", True):
        return "FAIL", ["link down"]
    alerts = []
    age = now - node.get("last_seen", now)
    if age > STALE_S:
        alerts.append(f"stale {age:.0f}s")
    p = node.get("power_mw", 0)
    if p >= POWER_FAIL_MW:
        alerts.append(f"power {p} mW over hard limit")
    elif p >= POWER_WARN_MW:
        alerts.append(f"power {p} mW high")
    hard = any("over hard" in a for a in alerts)
    status = "FAIL" if hard else ("WARN" if alerts else "OK")
    return status, alerts


def sim_nodes(now):
    return [
        {"id": "imu_node", "kind": "IMU", "up": True, "last_seen": now, "power_mw": 420},
        {"id": "power_node", "kind": "power", "up": True, "last_seen": now, "power_mw": 1061},
        {"id": "relay", "kind": "relay", "up": True, "last_seen": now - 1, "power_mw": 210},
        {"id": "node_b", "kind": "IMU", "up": True, "last_seen": now - 7, "power_mw": 380},
        {"id": "node_c", "kind": "power", "up": False, "last_seen": now - 20, "power_mw": 0},
    ]


def main():
    now = time.time()
    if len(sys.argv) > 1:
        data = json.load(open(sys.argv[1]))
        nodes = data.get("nodes_list") or sim_nodes(now)
    else:
        nodes = sim_nodes(now)

    print("=== NobroRTOS Mesh Health ===")
    print(f"{'node':12} {'kind':8} {'status':6} {'power':>9}  alerts")
    counts = {"OK": 0, "WARN": 0, "FAIL": 0}
    total_power = 0
    active_alerts = []
    for n in nodes:
        st, al = health(n, now)
        counts[st] += 1
        if n.get("up", True):
            total_power += n.get("power_mw", 0)
        for a in al:
            active_alerts.append((n["id"], a))
        print(f"{n['id']:12} {n['kind']:8} {st:6} {n.get('power_mw', 0):>6} mW  {'; '.join(al)}")

    up = sum(1 for n in nodes if n.get("up", True))
    sys_health = "FAIL" if counts["FAIL"] else ("WARN" if counts["WARN"] else "OK")
    print(
        f"\nSYSTEM: {sys_health}  | up {up}/{len(nodes)}  total_power {total_power} mW  "
        f"OK={counts['OK']} WARN={counts['WARN']} FAIL={counts['FAIL']}"
    )
    if active_alerts:
        print("ALERTS:")
        for nid, a in active_alerts:
            print(f"  - {nid}: {a}")
    return 1 if counts["FAIL"] else 0


if __name__ == "__main__":
    sys.exit(main())
