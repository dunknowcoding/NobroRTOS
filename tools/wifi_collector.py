#!/usr/bin/env python3
"""WiFi telemetry sink for the NobroRTOS collector (M95).

Listens on a TCP port; a NobroRTOS WiFi node connects and streams JSONL telemetry (the
same schema the serial jsonl_bridge uses). Prints each sample; exits PASS once a valid
JSONL telemetry line arrives. Stdlib only - no broker, no credentials here.

  python3 tools/wifi_collector.py --port 9099 --seconds 30
"""
import argparse
import json
import socket
import sys
import time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="0.0.0.0", help="bind address")
    ap.add_argument("--port", type=int, default=9099)
    ap.add_argument("--seconds", type=float, default=30.0)
    args = ap.parse_args()

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((args.host, args.port))
    srv.listen(1)
    srv.settimeout(args.seconds)
    print(f"listening on {args.host}:{args.port} for {args.seconds:.0f}s ...")

    got = 0
    try:
        conn, addr = srv.accept()
        print(f"node connected from {addr[0]}")
        conn.settimeout(args.seconds)
        buf = b""
        t0 = time.time()
        while time.time() - t0 < args.seconds and got < 5:
            try:
                chunk = conn.recv(1024)
            except socket.timeout:
                break
            if not chunk:
                break
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                line = line.decode(errors="ignore").strip()
                if line.startswith("{"):
                    try:
                        j = json.loads(line)
                        got += 1
                        print(f"  telemetry: {json.dumps(j)[:120]}")
                    except ValueError:
                        pass
        conn.close()
    except socket.timeout:
        print("no node connected in time")
    finally:
        srv.close()

    ok = got >= 1
    print(f"received {got} JSONL telemetry samples over WiFi")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
