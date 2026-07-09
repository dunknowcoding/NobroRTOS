#!/usr/bin/env python3
"""`nobro verify` - the Evidence Pack generator. "The RTOS that shows its work."

Runs the full software gate matrix (via run_checks) and, when built firmware ELFs are
present, the build-time budgets (via static_budget), then emits one machine-checkable
Evidence Pack as JSON **and** a self-contained human-readable HTML report. This is the
2026 audit-driven selling point in one command: every claim the system makes is a file
you can diff, attach to a review, or gate CI on.

    python tools/nobro_verify.py                 # full matrix -> _work/evidence/
    python tools/nobro_verify.py --quick         # skip the slow cargo gate
    python tools/nobro_verify.py --out-dir dist  # choose the output directory

Exit code 0 only when every gate passes (budgets are informational). The pack is
bench-agnostic by construction: no host paths, usernames, ports, or board identities -
only gate results, build-time budgets, and generic tool versions.
"""
import argparse
import datetime
import html
import json
import os
import platform
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_checks
import static_budget

ROOT = run_checks.ROOT
RELEASE = os.path.join(ROOT, "_work", "cargo-target", "thumbv7em-none-eabihf", "release")
# Representative demo ELFs to price if a build is present (basename only ever emitted).
BUDGET_TARGETS = ["udi_imu_demo_s140", "udi_imu_demo", "imu_i2c_demo",
                "resource_sched_demo", "async_exec_demo"]


def git_commit():
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, text=True,
            stderr=subprocess.DEVNULL).strip()
    except Exception:
        return None


def tool_versions():
    """Generic tool version strings only - no install paths (bench-agnostic)."""
    versions = {"python": platform.python_version()}
    try:
        versions["rustc"] = subprocess.check_output(
            ["rustc", "--version"], text=True, stderr=subprocess.DEVNULL).strip()
    except Exception:
        versions["rustc"] = None
    return versions


def find_objdump():
    for t in static_budget.DEFAULT_OBJDUMPS:
        if shutil.which(t):
            return t
    return None


def budget_limits():
    """Per-app ceilings from host/nobro-host-contract.json (Wave 15). Absent section
    or app = informational only; present = a hard gate."""
    try:
        with open(os.path.join(ROOT, "host", "nobro-host-contract.json"), encoding="utf-8") as f:
            return json.load(f).get("build_budgets", {})
    except OSError:
        return {}


def collect_budgets():
    objdump = find_objdump()
    if objdump is None:
        return {"available": False,
                "note": "arm-none-eabi-objdump not found; build-time budgets skipped"}
    targets = []
    for name in BUDGET_TARGETS:
        elf = os.path.join(RELEASE, name)
        if not os.path.exists(elf):
            continue
        try:
            (_frames, _cycles, worst, _path, worst_cycles, *_rest) = static_budget.analyze(
                elf, objdump)
            text, data, bss = static_budget.sizes(elf, objdump)
            entry = {
                "app": name,  # basename only, no path
                "flash_b": text + data,
                "static_ram_b": data + bss,
                "worst_stack_b": worst,
                "worst_total_ram_b": data + bss + worst,
                "worst_cycles": worst_cycles,
            }
            lim = budget_limits().get(name)
            if lim:
                checks = {
                    "max_flash_b": entry["flash_b"] <= lim["max_flash_b"],
                    "max_worst_total_ram_b":
                        entry["worst_total_ram_b"] <= lim["max_worst_total_ram_b"],
                    "max_worst_cycles": entry["worst_cycles"] <= lim["max_worst_cycles"],
                }
                entry["budget"] = {"limits": lim, "pass": all(checks.values()),
                                   "checks": checks}
            targets.append(entry)
        except Exception as exc:
            targets.append({"app": name, "error": str(exc)[:200]})
    if not targets:
        return {"available": False,
                "note": "no built demo ELFs under _work; run a build or hw_eval first"}
    graded = [t for t in targets if "budget" in t]
    return {"available": True, "targets": targets,
            "enforced": len(graded),
            "all_within_budget": all(t["budget"]["pass"] for t in graded)}


def build_pack_from_results(results, quick):
    """Assemble an Evidence Pack from gate results already collected (no re-run)."""
    passed = sum(1 for r in results if r["ok"])
    all_ok = all(r["ok"] for r in results)
    budgets = collect_budgets()
    budgets_ok = budgets.get("all_within_budget", True)
    verdict_ok = all_ok and budgets_ok
    return {
        "tool": "nobro verify",
        "tagline": "The RTOS that shows its work.",
        "generated_utc": datetime.datetime.now(datetime.timezone.utc)
                                 .strftime("%Y-%m-%dT%H:%M:%SZ"),
        "commit": git_commit(),
        "tool_versions": tool_versions(),
        "quick": quick,
        "summary": {
            "total": len(results),
            "passed": passed,
            "result": "ALL PASS" if verdict_ok else "FAIL",
            "budgets_ok": budgets_ok,
        },
        "gates": [{"name": r["name"],
                   "result": "PASS" if r["ok"] else "FAIL",
                   "detail": r["detail"]} for r in results],
        "budgets": budgets,
    }, verdict_ok


def build_pack(quick):
    results, all_ok = run_checks.run_gates(quick=quick)
    pack, _ = build_pack_from_results(results, quick)
    return pack, all_ok


def selftest():
    """Smoke-test pack assembly + HTML render without re-running the gate matrix."""
    results = [{"name": "smoke", "ok": True, "detail": ["ok"]}]
    pack, ok = build_pack_from_results(results, quick=True)
    html_out = render_html(pack)
    good = (ok and pack["summary"]["result"] == "ALL PASS"
            and "Evidence Pack" in html_out and "smoke" in html_out)
    print(f"pack result     : {pack['summary']['result']}")
    print(f"html bytes      : {len(html_out)}")
    print(f"RESULT: {'PASS' if good else 'FAIL'}")
    return 0 if good else 1


def render_html(pack):
    s = pack["summary"]
    ok = s["result"] == "ALL PASS"
    accent = "#1f9d55" if ok else "#c0392b"
    rows = []
    for g in pack["gates"]:
        gok = g["result"] == "PASS"
        badge = ("#1f9d55" if gok else "#c0392b")
        detail = html.escape(" \u00b7 ".join(g["detail"])) if g["detail"] else ""
        rows.append(
            f'<tr><td>{html.escape(g["name"])}</td>'
            f'<td><span class="badge" style="background:{badge}">{g["result"]}</span></td>'
            f'<td class="detail">{detail}</td></tr>')
    gate_rows = "\n".join(rows)

    budgets = pack["budgets"]
    if budgets.get("available"):
        brows = []
        for t in budgets["targets"]:
            if "error" in t:
                brows.append(f'<tr><td>{html.escape(t["app"])}</td>'
                             f'<td colspan="5" class="detail">{html.escape(t["error"])}</td></tr>')
                continue
            brows.append(
                f'<tr><td>{html.escape(t["app"])}</td>'
                f'<td>{t["flash_b"]:,}</td><td>{t["static_ram_b"]:,}</td>'
                f'<td>{t["worst_stack_b"]:,}</td><td>{t["worst_total_ram_b"]:,}</td>'
                f'<td>{t["worst_cycles"]:,}</td></tr>')
        budget_html = (
            '<table><thead><tr><th>app</th><th>flash B</th><th>static RAM B</th>'
            '<th>worst stack B</th><th>worst total RAM B</th><th>worst cycles</th></tr>'
            f'</thead><tbody>{"".join(brows)}</tbody></table>')
    else:
        budget_html = f'<p class="note">{html.escape(budgets.get("note", "unavailable"))}</p>'

    versions = " \u00b7 ".join(
        f"{k} {html.escape(str(v))}" for k, v in pack["tool_versions"].items() if v)
    meta = " \u00b7 ".join(filter(None, [
        f'commit {html.escape(pack["commit"])}' if pack.get("commit") else None,
        f'generated {html.escape(pack["generated_utc"])}',
        "quick" if pack["quick"] else None,
        versions,
    ]))
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>NobroRTOS Evidence Pack</title>
<style>
:root {{ color-scheme: light dark; }}
body {{ font-family: -apple-system, Segoe UI, Roboto, Helvetica, Arial, sans-serif;
        margin: 0; background: #0d1117; color: #e6edf3; }}
.wrap {{ max-width: 960px; margin: 0 auto; padding: 32px 20px 64px; }}
header {{ border-left: 6px solid {accent}; padding: 8px 0 8px 16px; margin-bottom: 8px; }}
h1 {{ margin: 0; font-size: 22px; }}
.tagline {{ color: #9da7b3; font-style: italic; margin: 4px 0 0; }}
.result {{ display: inline-block; margin: 18px 0; padding: 10px 18px; border-radius: 8px;
           font-weight: 700; font-size: 18px; background: {accent}; color: #fff; }}
.meta {{ color: #8b949e; font-size: 13px; margin: 6px 0 20px; }}
h2 {{ font-size: 16px; margin: 28px 0 8px; border-bottom: 1px solid #30363d; padding-bottom: 4px; }}
table {{ width: 100%; border-collapse: collapse; font-size: 14px; }}
th, td {{ text-align: left; padding: 7px 10px; border-bottom: 1px solid #21262d; vertical-align: top; }}
th {{ color: #9da7b3; font-weight: 600; }}
.badge {{ color: #fff; padding: 2px 10px; border-radius: 999px; font-size: 12px; font-weight: 700; }}
.detail {{ color: #8b949e; font-size: 12px; font-family: ui-monospace, Consolas, monospace; }}
.note {{ color: #8b949e; }}
</style></head>
<body><div class="wrap">
<header><h1>NobroRTOS &mdash; Evidence Pack</h1>
<p class="tagline">{html.escape(pack["tagline"])}</p></header>
<div class="result">{s["result"]} &nbsp;{s["passed"]}/{s["total"]} gates</div>
<div class="meta">{meta}</div>
<h2>Gate matrix</h2>
<table><thead><tr><th>gate</th><th>result</th><th>detail</th></tr></thead>
<tbody>{gate_rows}</tbody></table>
<h2>Build-time budgets (static, pre-flash)</h2>
{budget_html}
</div></body></html>
"""


def main():
    ap = argparse.ArgumentParser(description="nobro verify - emit an Evidence Pack.")
    ap.add_argument("--quick", action="store_true", help="skip the slow cargo test gate")
    ap.add_argument("--selftest", action="store_true", help="smoke-test pack render only")
    ap.add_argument("--out-dir", default=os.path.join(ROOT, "_work", "evidence"),
                    help="directory for evidence_pack.json / .html (default: _work/evidence)")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    pack, all_ok = build_pack(args.quick)
    os.makedirs(args.out_dir, exist_ok=True)
    json_path = os.path.join(args.out_dir, "evidence_pack.json")
    html_path = os.path.join(args.out_dir, "evidence_pack.html")
    with open(json_path, "w", encoding="utf-8") as f:
        json.dump(pack, f, indent=2)
    with open(html_path, "w", encoding="utf-8") as f:
        f.write(render_html(pack))

    s = pack["summary"]
    print(f"\nEvidence Pack: {s['result']} ({s['passed']}/{s['total']} gates)")
    b = pack["budgets"]
    if b.get("available"):
        print(f"  budgets: {len(b['targets'])} app(s) priced")
    else:
        print(f"  budgets: {b.get('note')}")
    print(f"  JSON: {os.path.relpath(json_path, ROOT)}")
    print(f"  HTML: {os.path.relpath(html_path, ROOT)}")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
