#!/usr/bin/env python3
"""NobroRTOS multi-board data collector (manifest-driven, autonomous).

Reads a board manifest (boards.json, or boards.example.json): each board declares a *transport* (how to reach it) and a
*protocol* (how to read it). New boards/apps plug in by editing the manifest - no code
change. Everything is non-destructive (J-Link halt/resume + serial read); no DFU, no
flashing, no manual steps. Exit 0 = every non-optional board delivered valid data.

Protocols:
  nobro_report  - halt-read a NOBRO_* RAM report over J-Link, decode by named schema
  jsonl_bridge  - drive the INA "JSONL bridge" (START/STOP), return the last sample
  serial_regex  - read a COM port and match a regex (presence/health of any app)

Usage:  python3 tools/multiboard_collect.py [--manifest <file>]
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

JLINK = os.environ.get("JLINK_EXE") or shutil.which("JLink.exe") or shutil.which("JLinkExe") or "JLinkExe"
HERE = os.path.dirname(os.path.abspath(__file__))


def jlink_mem32(addr, words, device):
    script = f"si SWD\nspeed 4000\nconnect\nhalt\nmem32 0x{addr:08X},{words}\ng\nq\n"
    with tempfile.NamedTemporaryFile("w", suffix=".jlink", delete=False) as f:
        f.write(script)
        path = f.name
    try:
        out = subprocess.run(
            [JLINK, "-device", device, "-if", "SWD", "-speed", "4000",
             "-autoconnect", "1", "-NoGui", "1", "-CommandFile", path],
            capture_output=True, text=True, timeout=30).stdout
    finally:
        os.unlink(path)
    vals = []
    for line in out.splitlines():
        m = re.match(r"^[0-9A-Fa-f]{8} = (.+)$", line.strip())
        if m:
            vals += [int(x, 16) for x in m.group(1).split()]
    return vals[:words]


def read_nobro_report(b, schemas):
    sch = schemas[b["schema"]]
    w = jlink_mem32(int(b["addr"], 16), sch["words"], b["device"])
    if len(w) < sch["words"] or w[0] != int(sch["magic"], 16):
        return None, f"no {b['schema']} report (magic={hex(w[0]) if w else 'none'})"
    rec = {f: w[i] for i, f in enumerate(sch["fields"]) if i < len(w)}
    rec["pass"] = bool(rec.get(sch["pass_field"], 0))
    return rec, None


def _open_serial(b):
    import serial
    sp = serial.Serial()
    sp.port = b["port"]
    sp.baudrate = b.get("baud", 115200)
    sp.timeout = 0.3
    sp.dtr = b.get("reset_on_open", False)
    sp.rts = b.get("reset_on_open", False)
    sp.open()
    return sp


def read_jsonl_bridge(b):
    sp = _open_serial(b)
    time.sleep(0.4)
    sp.write(b"START\n")
    last = None
    t0 = time.time()
    while time.time() - t0 < b.get("seconds", 3):
        ln = sp.readline().decode(errors="ignore").strip()
        if ln.startswith("{"):
            try:
                j = json.loads(ln)
                if "channels" in j:
                    last = j
            except json.JSONDecodeError:
                pass
    try:
        sp.write(b"STOP\n")
    except Exception:
        pass
    sp.close()
    if last:
        return {"pass": True, **last}, None
    return None, "no JSONL samples"


def read_serial_regex(b):
    sp = _open_serial(b)
    time.sleep(0.6)
    bb = ""
    t0 = time.time()
    while time.time() - t0 < b.get("seconds", 2.5):
        bb += sp.read(256).decode(errors="ignore")
    sp.close()
    m = re.search(b["match"], bb)
    if m:
        last_line = (bb.strip().splitlines() or [""])[-1][:80]
        return {"pass": True, "matched": m.group(0), "sample": last_line}, None
    return None, "no match"


def read_sim(b):
    """Simulate a sensor node's datastream + mesh route, for hardware not yet present.
    Reproducible-but-varying (seeded by a coarse time bucket); models occasional mesh
    packet loss. Lets the sensor-network / mesh collection logic be tested at scale."""
    import random
    rng = random.Random(b.get("seed", 0) + int(time.time()) // 5)
    if rng.random() < b.get("loss", 0.0):
        return None, "sim packet lost (mesh)"
    kind = b.get("sim_kind", "power")
    rec = {"pass": True, "sim": True, "sim_kind": kind,
           "route": b.get("route", ["collector"]), "hops": len(b.get("route", ["c"]))}
    if kind == "power":
        v = 5.0 + rng.uniform(-0.05, 0.05)
        i = rng.uniform(0.01, 0.2)
        rec.update({"bus_V": round(v, 3), "current_A": round(i, 4),
                    "power_W": round(v * i, 4)})
    elif kind == "imu":
        rec["accel_mg"] = 1000 + rng.randint(-40, 40)
    elif kind == "temp":
        rec["temp_c"] = round(24.0 + rng.uniform(-2, 3), 1)
    return rec, None


PARSERS = {
    "nobro_report": read_nobro_report,
    "jsonl_bridge": lambda b, s: read_jsonl_bridge(b),
    "serial_regex": lambda b, s: read_serial_regex(b),
    "sim": lambda b, s: read_sim(b),
    "vision": lambda b, s: read_vision(b),
}


def read_vision(b):
    """Run the vision-recognition tool (tools/vision_node.py) against a camera node and
    parse its VISION line. `tool_python` selects the interpreter that has PIL/numpy."""
    tool = os.path.join(HERE, "vision_node.py")
    py = b.get("tool_python", sys.executable)
    cmd = [py, "-u", tool, "--port", b["port"], "--frames", "2",
           "--save", os.path.join(HERE, "..", "_work", "vision_last.jpg")]
    if b.get("person_model"):
        cmd += ["--person-model", b["person_model"]]
    out = subprocess.run(
        cmd,
        capture_output=True, text=True, timeout=b.get("seconds", 60)).stdout
    m = re.search(
        r"VISION \S+ scene=(\S+) activity=(\S+) luma=([\d.]+) entropy=([\d.]+) "
        r"sharpness=(\d+) diff=([\d.]+) live=(\d)", out)
    if not m:
        return None, "no vision line (" + out.strip().splitlines()[-1][:60] + ")" if out.strip() else "no output"
    rec = {"pass": m.group(7) == "1", "scene": m.group(1), "activity": m.group(2),
           "luma": float(m.group(3)), "entropy": float(m.group(4)),
           "sharpness": int(m.group(5)), "diff": float(m.group(6))}
    person = re.search(r"person=(\d+) person_score=([-\d.]+)", out)
    if person:
        rec["person"] = int(person.group(1))
        rec["person_score"] = float(person.group(2))
    return rec, None


def network_rollup(boards):
    """Sensor-network view over all collected nodes (real + simulated): node count,
    aggregate power, and the (simulated) mesh topology + depth."""
    total_power = 0.0
    for d in boards.values():
        if "channels" in d:
            total_power += sum(c.get("power_W", 0.0) for c in d["channels"])
        total_power += d.get("power_W", 0.0)
    edges = []
    for name, d in boards.items():
        route = d.get("route")
        if route:
            chain = [name] + list(route)
            edges += [f"{a}->{b}" for a, b in zip(chain, chain[1:])]
    max_hops = max([d.get("hops", 1) for d in boards.values()] + [1])
    # AI fusion (M39): combine the camera's activity classification with the IMU
    # nodes' deviation from 1 g into one bench-activity verdict.
    cameras = []
    imu_dev_mg = 0
    for name, d in boards.items():
        if "activity" in d:
            cameras.append({"node": name, "scene": d.get("scene"),
                            "activity": d["activity"], "person": d.get("person")})
        for key in ("accel_mg", "mag_mg"):
            if key in d:
                imu_dev_mg = max(imu_dev_mg, abs(int(d[key]) - 1000))
    fusion = None
    if cameras or imu_dev_mg:
        any_motion = any(c["activity"] == "motion" for c in cameras)
        moving = any_motion or imu_dev_mg > 80
        fusion = {"verdict": "active" if moving else "quiet",
                  "cameras": cameras, "cameras_seen": len(cameras),
                  "imu_deviation_mg": imu_dev_mg}
    # mixed-MCU registry (M97): group the live nodes by CPU architecture
    arch_registry = {}
    for name, d in boards.items():
        arch_registry.setdefault(d.get("arch", "unknown"), []).append(name)
    return {
        "nodes": len(boards),
        "total_power_W": round(total_power, 3),
        "mesh_max_hops": max_hops,
        "mesh_edges": sorted(set(edges)),
        "ai_fusion": fusion,
        "arch_registry": arch_registry,
    }


def summary_line(name, proto, rec):
    if proto == "nobro_report":
        return (f"accel={rec.get('accel_mg')} mg who=0x{rec.get('who_am_i', 0):02X} "
                f"reads={rec.get('reads')} all_pass={rec.get('all_pass')}")
    if proto == "jsonl_bridge":
        ch = rec.get("channels", [])
        return (f"{rec.get('chip')} bus={rec.get('bus_V'):.3f}V " +
                " ".join(f"ch{i}:{c['current_A']*1000:.1f}mA" for i, c in enumerate(ch)))
    if proto == "serial_regex":
        return f"matched '{rec.get('matched')}'  ({rec.get('sample')})"
    if proto == "sim":
        via = "->".join(rec.get("route", []))
        k = rec.get("sim_kind")
        if k == "power":
            return f"[sim] {rec['power_W']:.3f} W  via {via}  ({rec['hops']} hops)"
        if k == "imu":
            return f"[sim] accel={rec['accel_mg']} mg  via {via}"
        if k == "temp":
            return f"[sim] {rec['temp_c']} C  relay"
    return ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default=None,
                    help="board manifest (default: boards.json if present, else "
                         "boards.example.json)")
    args = ap.parse_args()
    manifest = args.manifest
    if manifest is None:
        private = os.path.join(HERE, "boards.json")
        manifest = private if os.path.exists(private) else os.path.join(
            HERE, "boards.example.json")
    cfg = json.load(open(manifest))
    schemas = cfg.get("schemas", {})

    print("=== NobroRTOS multi-board collection (manifest-driven, autonomous) ===")
    snapshot = {"t": time.strftime("%H:%M:%S"), "boards": {}}
    all_ok = True
    for b in cfg["boards"]:
        proto, name = b["protocol"], b["name"]
        optional = b.get("optional", False)
        try:
            rec, err = PARSERS[proto](b, schemas)
        except Exception as e:  # noqa: BLE001
            rec, err = None, str(e)
        if rec:
            snapshot["boards"][name] = {"kind": b["kind"], "arch": b.get("arch", "unknown"),
                                        **{k: v for k, v in rec.items() if k != "pass"}}
            print(f"[{name:8s}] OK   {b['kind']}")
            print(f"           {summary_line(name, proto, rec)}")
            if not rec.get("pass", True) and not optional:
                all_ok = False
        else:
            print(f"[{name:8s}] {'skip(optional)' if optional else 'FAIL'}  "
                  f"{b['kind']}  ({err})")
            if not optional:
                all_ok = False

    net = network_rollup(snapshot["boards"])
    snapshot["network"] = net
    print("\n--- sensor-network rollup ---")
    print(f"  nodes={net['nodes']}  total_power={net['total_power_W']} W  "
          f"mesh_max_hops={net['mesh_max_hops']}")
    if net["mesh_edges"]:
        print(f"  mesh: {'  '.join(net['mesh_edges'])}")
    if net.get("arch_registry"):
        archs = "  ".join(f"{a}({len(n)})" for a, n in sorted(net["arch_registry"].items()))
        print(f"  architectures: {archs}")
    if net.get("ai_fusion"):
        f = net["ai_fusion"]
        cams = " ".join(f"{c['node']}:{c['scene']}/{c['activity']}" for c in f["cameras"])
        print(f"  ai-fusion: bench={f['verdict']}  cameras={f['cameras_seen']} [{cams}]  "
              f"imu_dev={f['imu_deviation_mg']} mg")
    print("\n--- unified snapshot ---")
    print(json.dumps(snapshot, indent=2, default=str))
    print(f"\nRESULT: {'PASS' if all_ok else 'FAIL'}")
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
