#!/usr/bin/env python3
"""Fleet Evidence: fold every audit artifact into one fleet-level pack (Wave 11).

`nobro verify` proves one workspace; a fleet is many nodes, OTA decisions, hardware
runs, and capability traces. This tool aggregates whatever evidence exists under
`_work/evidence/` - the software Evidence Pack, OTA preflight verdicts, hardware eval
JSONs (`hw/*.json`), and capability replay audits (`replay/*.json` or raw `.trace`
blobs decoded via nobro_rtos.replay) - into `fleet_pack.json` + a self-contained
`fleet_pack.html`. Missing artifact kinds are reported as absent, never invented.

    python tools/fleet_evidence.py            # aggregate + write the fleet pack
    python tools/fleet_evidence.py --selftest # synthetic artifacts end-to-end (gate)
"""
import argparse
import datetime
import glob
import html
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EVIDENCE = os.path.join(ROOT, "_work", "evidence")
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))


def load_json(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def collect(evidence_dir):
    from nobro_rtos.replay import decode_trace, to_audit

    fleet = {
        "tool": "nobro verify --fleet",
        "generated_utc": datetime.datetime.now(datetime.timezone.utc)
                                 .strftime("%Y-%m-%dT%H:%M:%SZ"),
        "sections": {},
    }
    problems = []

    pack = load_json(os.path.join(evidence_dir, "evidence_pack.json"))
    if pack:
        s = pack.get("summary", {})
        fleet["sections"]["software"] = {
            "present": True, "result": s.get("result"),
            "gates": f"{s.get('passed')}/{s.get('total')}",
            "budgets_ok": s.get("budgets_ok", True),
            "commit": pack.get("commit"),
        }
        if s.get("result") != "ALL PASS":
            problems.append("software gates not ALL PASS")
    else:
        fleet["sections"]["software"] = {"present": False}

    ota = load_json(os.path.join(evidence_dir, "ota_preflight.json"))
    if ota:
        # real schema: {"verify": {name: {"verdict": ..., "accepted": bool}}};
        # the selftest uses the flat {"verdicts": {name: verdict}} shorthand.
        raw = ota.get("verify") or ota.get("verdicts") or {}
        verdicts = {k: (v.get("verdict") if isinstance(v, dict) else v)
                    for k, v in raw.items()}
        # keep only Accept/Reject verdict strings (drop bookkeeping like "ok": true)
        verdicts = {k: v for k, v in verdicts.items()
                    if isinstance(v, str) and (v.startswith("Accept") or v.startswith("Reject"))}
        fleet["sections"]["ota"] = {"present": True, "verdicts": verdicts}
        good_ok = str(verdicts.get("good", "")).lower().startswith("accept")
        bads_rejected = all(str(v).lower().startswith("reject")
                            for k, v in verdicts.items() if k != "good")
        if not (good_ok and bads_rejected and verdicts):
            problems.append("ota preflight verdicts unexpected")
    else:
        fleet["sections"]["ota"] = {"present": False}

    hw_nodes = []
    for path in sorted(glob.glob(os.path.join(evidence_dir, "hw", "*.json"))):
        node = load_json(path)
        if not node:
            continue
        hw_nodes.append({
            "run": os.path.basename(path),
            "app": node.get("app"), "profile": node.get("profile"),
            "backend": node.get("backend"),
            "all_pass": node.get("all_pass"),
        })
        if node.get("all_pass") != 1:
            problems.append(f"hw run {os.path.basename(path)} not all_pass")
    fleet["sections"]["hardware"] = {"present": bool(hw_nodes), "runs": hw_nodes}

    audits = []
    for path in sorted(glob.glob(os.path.join(evidence_dir, "replay", "*.json"))):
        audit = load_json(path)
        if audit:
            audits.append({"file": os.path.basename(path),
                           "records": audit.get("records"),
                           "faults": audit.get("faults")})
    for path in sorted(glob.glob(os.path.join(evidence_dir, "replay", "*.trace"))):
        with open(path, "rb") as f:
            audit = to_audit(decode_trace(f.read()))
        out = path[:-6] + ".decoded.json"
        with open(out, "w", encoding="utf-8") as f:
            json.dump(audit, f, indent=2)
        audits.append({"file": os.path.basename(path),
                       "records": audit["records"], "faults": audit["faults"]})
    for a in audits:
        if a.get("faults"):
            problems.append(f"replay {a['file']}: {a['faults']} fault record(s)")
    fleet["sections"]["replay"] = {"present": bool(audits), "audits": audits}

    present = sum(1 for sec in fleet["sections"].values() if sec.get("present"))
    fleet["summary"] = {
        "sections_present": present,
        "problems": problems,
        "result": "FLEET PASS" if present and not problems else
                  ("FLEET FAIL" if problems else "NO EVIDENCE"),
    }
    return fleet


def render_html(fleet):
    rows = []
    for name, sec in fleet["sections"].items():
        state = "present" if sec.get("present") else "absent"
        detail = html.escape(json.dumps({k: v for k, v in sec.items() if k != "present"})[:220])
        rows.append(f"<tr><td>{name}</td><td class='{state}'>{state}</td><td><code>{detail}</code></td></tr>")
    s = fleet["summary"]
    color = {"FLEET PASS": "#16a34a", "FLEET FAIL": "#dc2626"}.get(s["result"], "#ca8a04")
    problems = "".join(f"<li>{html.escape(p)}</li>" for p in s["problems"]) or "<li>none</li>"
    return f"""<!doctype html><meta charset="utf-8"><title>NobroRTOS Fleet Evidence</title>
<style>body{{font-family:system-ui;margin:2rem auto;max-width:60rem;padding:0 1rem}}
table{{border-collapse:collapse;width:100%}}td,th{{border:1px solid #d4d4d8;padding:.4rem .6rem;text-align:left}}
.present{{color:#16a34a}}.absent{{color:#a1a1aa}}h1 span{{color:{color}}}</style>
<h1>Fleet Evidence — <span>{s["result"]}</span></h1>
<p>{fleet["generated_utc"]} · sections present: {s["sections_present"]}/4</p>
<table><tr><th>section</th><th>state</th><th>summary</th></tr>{"".join(rows)}</table>
<h2>Problems</h2><ul>{problems}</ul>"""


def selftest():
    import struct
    import tempfile
    from nobro_rtos.replay import TraceRecord, encode_record

    with tempfile.TemporaryDirectory() as tmp:
        os.makedirs(os.path.join(tmp, "hw"))
        os.makedirs(os.path.join(tmp, "replay"))
        json.dump({"summary": {"result": "ALL PASS", "passed": 19, "total": 19,
                               "budgets_ok": True}, "commit": "deadbeef"},
                  open(os.path.join(tmp, "evidence_pack.json"), "w"))
        json.dump({"verdicts": {"good": "Accept", "tampered": "RejectTampered",
                                "rollback": "RejectRollback"}},
                  open(os.path.join(tmp, "ota_preflight.json"), "w"))
        json.dump({"app": "udi", "profile": "s140", "backend": "eh", "all_pass": 1},
                  open(os.path.join(tmp, "hw", "udi_eh.json"), "w"))
        blob = b"".join(encode_record(TraceRecord(i, i * 100, 4, 3, 3, 0, 0, 0))
                        for i in range(5))
        open(os.path.join(tmp, "replay", "sensor.trace"), "wb").write(blob)

        fleet = collect(tmp)
        ok = (fleet["summary"]["result"] == "FLEET PASS"
              and fleet["summary"]["sections_present"] == 4
              and fleet["sections"]["replay"]["audits"][0]["records"] == 5)
        # a fault record must flip the verdict
        blob2 = blob + encode_record(TraceRecord(9, 900, 4, 3, 6, 0, 0, 1))
        open(os.path.join(tmp, "replay", "sensor.trace"), "wb").write(blob2)
        fleet2 = collect(tmp)
        ok = ok and fleet2["summary"]["result"] == "FLEET FAIL"
        html_out = render_html(fleet)
        ok = ok and "FLEET PASS" in html_out
        print(f"sections=4 replay_records=5 fault_flips_verdict={fleet2['summary']['result']=='FLEET FAIL'}")
        print(f"RESULT: {'PASS' if ok else 'FAIL'}")
        return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    fleet = collect(EVIDENCE)
    os.makedirs(EVIDENCE, exist_ok=True)
    jpath = os.path.join(EVIDENCE, "fleet_pack.json")
    hpath = os.path.join(EVIDENCE, "fleet_pack.html")
    json.dump(fleet, open(jpath, "w", encoding="utf-8"), indent=2)
    open(hpath, "w", encoding="utf-8").write(render_html(fleet))
    s = fleet["summary"]
    print(f"{s['result']} - sections {s['sections_present']}/4")
    for p in s["problems"]:
        print("  !", p)
    print(f"JSON: {os.path.relpath(jpath, ROOT)}\nHTML: {os.path.relpath(hpath, ROOT)}")
    return 0 if s["result"] == "FLEET PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
