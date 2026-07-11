#!/usr/bin/env python3
"""Run sanitized discovery and a state-restoring deep-HAL evaluation matrix."""

import argparse
import collections
import datetime
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "tools" / "dev" / "boards.json"
DEFAULT_OUT = ROOT / "_work" / "evidence" / "hil" / "fleet.json"
DEFAULT_APPS = ("sal", "sched", "udi:native", "udi:eh", "udi:arduino", "wcet",
                "stack", "mpu", "async", "database")


def serial_endpoints() -> set[str]:
    try:
        from serial.tools import list_ports
        return {port.device for port in list_ports.comports()}
    except ImportError:
        if os.name == "nt":
            return set()
        return {str(path) for pattern in ("ttyACM*", "ttyUSB*")
                for path in pathlib.Path("/dev").glob(pattern)}


def load_manifest(path: pathlib.Path) -> list[dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    boards = data.get("boards")
    if not isinstance(boards, list):
        raise ValueError("manifest must contain a boards list")
    return boards


def discover(entries: list[dict], ports: set[str], jlink_present: bool) -> dict:
    by_transport = collections.Counter()
    by_protocol = collections.Counter()
    present = 0
    optional_missing = 0
    required_missing = 0
    for entry in entries:
        transport = str(entry.get("transport", "virtual"))
        protocol = str(entry.get("protocol", "unknown"))
        by_transport[transport] += 1
        by_protocol[protocol] += 1
        if transport == "serial":
            reachable = entry.get("port") in ports
        elif transport == "jlink":
            reachable = jlink_present
        elif protocol == "sim":
            reachable = True
        elif transport == "tool":
            reachable = bool(entry.get("tool_python"))
        else:
            reachable = False
        if reachable:
            present += 1
        elif entry.get("optional", False):
            optional_missing += 1
        else:
            required_missing += 1
    return {
        "configured": len(entries),
        "present": present,
        "required_missing": required_missing,
        "optional_missing": optional_missing,
        "transport_counts": dict(sorted(by_transport.items())),
        "protocol_counts": dict(sorted(by_protocol.items())),
    }


def classify_sample(protocol: str, entry: dict, data: bytes) -> bool:
    """True when the captured bytes contain a valid report for the protocol."""
    text = data.decode("utf-8", "replace")
    if protocol == "nobro_report":
        return "NOBRO" in text
    if protocol == "serial_regex":
        pattern = entry.get("report_regex") or entry.get("regex")
        if pattern:
            return re.search(pattern, text) is not None
        return bool(text.strip())
    if protocol == "jsonl_bridge":
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("{"):
                try:
                    json.loads(line)
                    return True
                except ValueError:
                    continue
        return False
    return bool(text.strip())


def sample_serial(entries: list[dict], ports: set[str], seconds: float) -> dict:
    """Read-only sample of each present serial endpoint: 'present' vs
    'currently emitting a valid conformance report'. One short direct read per
    port, DTR/RTS never asserted (native-USB targets must not be reset), and
    only protocol-level counts leave this function — no identities."""
    try:
        import serial  # pyserial
    except ImportError:
        return {"skipped": "pyserial_unavailable"}
    per_protocol: dict[str, dict] = {}
    for entry in entries:
        if entry.get("transport") != "serial":
            continue
        port = entry.get("port")
        if port not in ports:
            continue
        protocol = str(entry.get("protocol", "unknown"))
        stats = per_protocol.setdefault(
            protocol, {"sampled": 0, "emitting": 0, "quiet": 0, "open_error": 0}
        )
        stats["sampled"] += 1
        try:
            probe = serial.Serial()
            probe.port = port
            probe.baudrate = int(entry.get("baud", 115200))
            probe.timeout = 0.5
            # Configure control lines BEFORE open so native-USB targets are
            # never reset by a DTR/RTS pulse.
            probe.dtr = False
            probe.rts = False
            probe.open()
        except Exception:
            stats["open_error"] += 1
            continue
        try:
            if os.name == "nt":
                try:
                    probe.set_buffer_size(rx_size=1 << 20)
                except Exception:
                    pass
            deadline = time.monotonic() + seconds
            data = b""
            while time.monotonic() < deadline and len(data) < 65536:
                chunk = probe.read(4096)
                if chunk:
                    data += chunk
        finally:
            probe.close()
        stats["emitting" if classify_sample(protocol, entry, data) else "quiet"] += 1
    return per_protocol


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: pathlib.Path, record: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, indent=2, sort_keys=True), encoding="utf-8")


def run_deep_hal(apps: tuple[str, ...], profile: str, jlink: str,
                 restore_dfu: bool, initial_dfu: bool) -> tuple[list[dict], bool]:
    import nobro_hw_eval as hw

    raw_dir = ROOT / "_work" / "evidence" / "hil" / "raw"
    raw_dir.mkdir(parents=True, exist_ok=True)
    results = []
    restored = not restore_dfu
    try:
        for spec in apps:
            app, _, backend = spec.partition(":")
            backend = backend or "native"
            if app not in hw.APPS or profile not in hw.APPS[app]["bin"]:
                results.append({"app": app, "backend": backend, "ok": False,
                                "reason": "unsupported"})
                continue
            raw_path = raw_dir / f"{app}-{backend}.json"
            command = [sys.executable, str(ROOT / "tools" / "nobro_hw_eval.py"), app,
                       "--profile", profile, "--jlink", jlink,
                       "--flash", "jlink", "--launch", "vector",
                       "--backend", backend,
                       "--json-out", str(raw_path)]
            completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
            (raw_dir / f"{app}-{backend}.log").write_text(
                completed.stdout + completed.stderr, encoding="utf-8"
            )
            binary = pathlib.Path(hw.RELEASE) / hw.APPS[app]["bin"][profile]
            result = {
                "app": app,
                "backend": backend if hw.APPS[app].get("backends") else None,
                "ok": completed.returncode == 0,
                "firmware_sha256": sha256(binary) if binary.is_file() else None,
            }
            if completed.returncode:
                result["reason"] = "evaluation_failed"
            results.append(result)
    finally:
        if restore_dfu:
            hw.enter_dfu(jlink)
            for _ in range(30):
                if hw.dfu_drives():
                    restored = True
                    break
                time.sleep(1)
        elif initial_dfu:
            restored = False
    return results, restored


def selftest() -> int:
    entries = [
        {"name": "private-a", "transport": "serial", "port": "secret-a",
         "protocol": "serial_regex"},
        {"name": "private-b", "transport": "virtual", "protocol": "sim"},
    ]
    result = discover(entries, {"secret-a"}, False)
    encoded = json.dumps(result)
    assert result["configured"] == 2 and result["present"] == 2
    assert "private" not in encoded and "secret" not in encoded

    # Sampling classifier: valid report vs noise per protocol, no identities.
    assert classify_sample("nobro_report", {}, b"NOBRO-CDC k=v all_pass=1\n")
    assert not classify_sample("nobro_report", {}, b"garbage noise\n")
    assert classify_sample(
        "serial_regex", {"report_regex": r"VERDICT: PASS"}, b"... VERDICT: PASS\n"
    )
    assert not classify_sample(
        "serial_regex", {"report_regex": r"VERDICT: PASS"}, b"boot banner only\n"
    )
    assert classify_sample("jsonl_bridge", {}, b'{"node": 1, "ok": true}\n')
    assert not classify_sample("jsonl_bridge", {}, b"{broken json\n")
    print("HIL MATRIX SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=pathlib.Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--apps", default=",".join(DEFAULT_APPS))
    parser.add_argument("--profile", choices=("nosd", "s140"), default="s140")
    parser.add_argument("--discover-only", action="store_true")
    parser.add_argument("--sample", action="store_true",
                        help="read-only sample of present serial endpoints: are "
                             "they currently emitting a valid report?")
    parser.add_argument("--sample-seconds", type=float, default=3.0)
    parser.add_argument("--restore-dfu", action="store_true")
    parser.add_argument("--json-out", type=pathlib.Path, default=DEFAULT_OUT)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.manifest.is_file():
        print("HIL MATRIX: FAIL (private manifest not found)")
        return 2

    sys.path.insert(0, str(ROOT / "tools"))
    import nobro_hw_eval as hw
    try:
        jlink = hw.find_jlink(None)
    except SystemExit:
        jlink = None
    entries = load_manifest(args.manifest)
    ports = serial_endpoints()
    discovery = discover(entries, ports, jlink is not None)
    initial_dfu = bool(hw.dfu_drives())
    evidence = {
        "schema": "nobro-hil-fleet-v1",
        "generated_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "discovery": discovery,
        "deep_hal": [],
        "initial_state": "dfu" if initial_dfu else "application_or_unknown",
        "restored": True,
    }
    print("HIL DISCOVERY:", json.dumps(discovery, sort_keys=True))
    if args.sample:
        sampling = sample_serial(entries, ports, args.sample_seconds)
        evidence["serial_sampling"] = sampling
        print("HIL SAMPLING:", json.dumps(sampling, sort_keys=True))
    if args.discover_only:
        write_json(args.json_out, evidence)
        return int(discovery["required_missing"] > 0)
    if jlink is None:
        print("HIL MATRIX: FAIL (debug probe CLI unavailable)")
        return 2
    if not initial_dfu:
        print("HIL MATRIX: FAIL (automated destructive evaluation requires an initial DFU state)")
        return 2
    if not args.restore_dfu:
        print("HIL MATRIX: FAIL (target began in DFU; pass --restore-dfu)")
        return 2

    apps = tuple(app.strip() for app in args.apps.split(",") if app.strip())
    results, restored = run_deep_hal(apps, args.profile, jlink, args.restore_dfu, initial_dfu)
    evidence["deep_hal"] = results
    evidence["restored"] = restored
    write_json(args.json_out, evidence)
    passed = all(result["ok"] for result in results) and restored
    print(f"HIL MATRIX: {'PASS' if passed else 'FAIL'} "
          f"({sum(result['ok'] for result in results)}/{len(results)} apps; restored={restored})")
    return int(not passed)


if __name__ == "__main__":
    sys.exit(main())
