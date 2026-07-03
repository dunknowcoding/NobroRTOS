#!/usr/bin/env python3
"""Multimodal edge-event fusion for the NobroRTOS mesh (M110, M117).

Fuses the per-node edge analytics the bench already produces on-device - camera motion/
anomaly (Vision8 on the XIAO) and audio VAD/anomaly (ES8311 node) - into:
  M117 - multimodal events: co-occurring vision + audio evidence raises confidence and a
         combined class (e.g. vision_anomaly + audio_anomaly -> "disturbance").
  M110 - each edge event becomes a structured mesh ALERT (node, kind, severity) the
         collector can forward.
--selftest runs synthetic scenarios so it is verifiable with no hardware attached.
"""
import argparse
import sys


def fuse(nodes):
    """nodes: list of dicts with keys among vision_motion, vision_anomaly, audio_vad,
    audio_anomaly. Returns (alerts, multimodal_events)."""
    alerts = []
    vision_evt = any(n.get("vision_anomaly") for n in nodes)
    vision_motion = any(n.get("vision_motion") for n in nodes)
    audio_anom = any(n.get("audio_anomaly") for n in nodes)
    audio_vad = any(n.get("audio_vad") for n in nodes)

    # M110: raise a mesh alert per edge event, with severity
    for n in nodes:
        name = n.get("node", "?")
        if n.get("vision_anomaly"):
            alerts.append((name, "vision_anomaly", "high"))
        elif n.get("vision_motion"):
            alerts.append((name, "vision_motion", "info"))
        if n.get("audio_anomaly"):
            alerts.append((name, "audio_anomaly", "high"))
        elif n.get("audio_vad"):
            alerts.append((name, "audio_vad", "info"))

    # M117: multimodal fusion - co-occurrence upgrades to a named, high-confidence event
    events = []
    if vision_evt and audio_anom:
        events.append(("disturbance", 95))       # both sensors anomalous
    elif vision_motion and audio_vad:
        events.append(("activity", 80))          # motion + sound = someone present
    elif vision_evt or audio_anom:
        events.append(("single_anomaly", 60))    # one modality only
    elif vision_motion or audio_vad:
        events.append(("single_activity", 45))
    return alerts, events


def selftest():
    cases = [
        ("quiet room", [{"node": "xiao", "vision_motion": 0, "vision_anomaly": 0},
                        {"node": "es8311", "audio_vad": 0, "audio_anomaly": 0}],
         0, None),
        ("someone walks in + talks",
         [{"node": "xiao", "vision_motion": 1}, {"node": "es8311", "audio_vad": 1}],
         2, "activity"),
        ("glass break: vision + audio anomaly",
         [{"node": "xiao", "vision_anomaly": 1}, {"node": "es8311", "audio_anomaly": 1}],
         2, "disturbance"),
        ("camera-only anomaly",
         [{"node": "xiao", "vision_anomaly": 1}, {"node": "es8311", "audio_vad": 0}],
         1, "single_anomaly"),
    ]
    ok = True
    for name, nodes, want_alerts, want_event in cases:
        alerts, events = fuse(nodes)
        ev = events[0][0] if events else None
        alert_ok = (want_alerts is None) or (len(alerts) == want_alerts)
        event_ok = ev == want_event
        good = alert_ok and event_ok
        ok = ok and good
        print(f"  [{'OK' if good else 'FAIL'}] {name}: alerts={len(alerts)} event={ev}")
        for n, k, sev in alerts:
            print(f"        ALERT node={n} kind={k} sev={sev}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    ap.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
