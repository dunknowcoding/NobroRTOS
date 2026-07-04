#!/usr/bin/env python3
"""Build-time worst-case stack + RAM/flash budget report for a NobroRTOS ELF (M206).

Most RTOSes size stacks by guesswork and find out at runtime (stack canaries,
HardFaults). NobroRTOS apps are static and heap-free, so the worst case is computable
at build time: this tool disassembles the ELF (arm-none-eabi-objdump), extracts each
function's frame size from its prologue (push/vpush/sub sp), builds the call graph
from bl/b.w edges, and reports the deepest stack path plus static RAM and flash use.
Recursive or indirect-call functions are flagged rather than silently mispriced.

  python3 tools/static_budget.py path/to/app.elf [--objdump PATH] [--ram-budget BYTES]

Exit code 1 if --ram-budget (static RAM + worst-case stack) is exceeded.
"""
import argparse
import re
import shutil
import subprocess
import sys
from collections import defaultdict

DEFAULT_OBJDUMPS = [
    "arm-none-eabi-objdump",
    r"C:\msys64\ucrt64\bin\arm-none-eabi-objdump.exe",
]

SYM_RE = re.compile(r"^([0-9a-f]+) <([^>]+)>:$")
PUSH_RE = re.compile(r"\bpush(?:\.w)?\s+\{([^}]*)\}")
VPUSH_RE = re.compile(r"\bvpush\s+\{([^}]*)\}")
SUBSP_RE = re.compile(r"\bsub(?:\.w)?\s+sp,(?:\s*sp,)?\s*#(\d+)")
CALL_RE = re.compile(r"\bbl(?:\.w)?\s+[0-9a-f]+ <([^>+]+)(?:\+0x[0-9a-f]+)?>")
TAILCALL_RE = re.compile(r"\bb(?:\.w)?\s+[0-9a-f]+ <([^>+]+)(?:\+0x[0-9a-f]+)?>")
INDIRECT_RE = re.compile(r"\bblx\s+r\d+")


def reg_count(reglist: str) -> int:
    n = 0
    for part in reglist.split(","):
        part = part.strip()
        if "-" in part:  # e.g. r4-r7 or d8-d15
            lo, hi = part.split("-")
            n += int(hi[1:]) - int(lo[1:]) + 1
        elif part:
            n += 1
    return n


def analyze(elf: str, objdump: str):
    out = subprocess.run(
        [objdump, "-d", "--no-show-raw-insn", elf],
        capture_output=True, text=True, check=True,
    ).stdout

    frames: dict[str, int] = defaultdict(int)
    calls: dict[str, set] = defaultdict(set)
    indirect: set = set()
    cur = None
    for line in out.splitlines():
        m = SYM_RE.match(line)
        if m:
            cur = m.group(2)
            frames.setdefault(cur, 0)
            continue
        if cur is None:
            continue
        if m := PUSH_RE.search(line):
            frames[cur] += 4 * reg_count(m.group(1))
        elif m := VPUSH_RE.search(line):
            frames[cur] += 8 * reg_count(m.group(1))
        elif m := SUBSP_RE.search(line):
            frames[cur] += int(m.group(1))
        if m := CALL_RE.search(line):
            calls[cur].add(m.group(1))
        elif m := TAILCALL_RE.search(line):
            callee = m.group(1)
            if callee in frames or not callee.startswith("."):
                calls[cur].add(callee)
        if INDIRECT_RE.search(line):
            indirect.add(cur)

    # deepest stack path by DFS with cycle detection
    depth_cache: dict[str, tuple[int, list]] = {}

    def depth(fn: str, stack: tuple) -> tuple[int, list]:
        if fn in stack:  # recursion: cost of one frame, flagged separately
            return (0, ["<recursion>"])
        if fn in depth_cache:
            return depth_cache[fn]
        best, best_path = 0, []
        for callee in calls.get(fn, ()):
            if callee not in frames:
                continue
            d, p = depth(callee, stack + (fn,))
            if d > best:
                best, best_path = d, p
        result = (frames.get(fn, 0) + best, [fn] + best_path)
        depth_cache[fn] = result
        return result

    roots = [f for f in frames if f in ("main", "Reset", "Reset_Handler", "__cortex_m_rt_main")]
    if not roots:
        roots = list(frames)
    worst, worst_path = 0, []
    for r in roots:
        d, p = depth(r, ())
        if d > worst:
            worst, worst_path = d, p
    recursive = sorted(f for f, (_, p) in depth_cache.items() if "<recursion>" in p)
    return frames, worst, worst_path, sorted(indirect), recursive


def sizes(elf: str, objdump: str):
    size_tool = objdump.replace("objdump", "size")
    out = subprocess.run([size_tool, elf], capture_output=True, text=True, check=True).stdout
    line = out.strip().splitlines()[-1].split()
    text, data, bss = int(line[0]), int(line[1]), int(line[2])
    return text, data, bss


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("elf")
    ap.add_argument("--objdump", default=None)
    ap.add_argument("--ram-budget", type=int, default=None,
                    help="fail if static RAM + worst-case stack exceeds this")
    ap.add_argument("--top", type=int, default=5, help="largest frames to list")
    args = ap.parse_args()

    objdump = args.objdump or next(
        (t for t in DEFAULT_OBJDUMPS if shutil.which(t) or t.endswith(".exe")), None)
    if objdump is None:
        sys.exit("no arm-none-eabi-objdump found; pass --objdump")

    frames, worst, path, indirect, recursive = analyze(args.elf, objdump)
    text, data, bss = sizes(args.elf, objdump)
    static_ram = data + bss
    total_ram = static_ram + worst

    print(f"flash (text+data):        {text + data:8d} B")
    print(f"static RAM (data+bss):    {static_ram:8d} B")
    print(f"worst-case stack:         {worst:8d} B")
    print(f"  deepest path: {' -> '.join(path[:8])}{' ...' if len(path) > 8 else ''}")
    print(f"worst-case total RAM:     {total_ram:8d} B")
    biggest = sorted(frames.items(), key=lambda kv: -kv[1])[:args.top]
    print("largest frames: " + ", ".join(f"{f}={s}B" for f, s in biggest))
    if indirect:
        print(f"CAUTION {len(indirect)} function(s) make indirect calls (unpriced): "
              + ", ".join(indirect[:4]) + ("..." if len(indirect) > 4 else ""))
    if recursive:
        print(f"CAUTION recursion detected in: {', '.join(recursive[:4])}")

    if args.ram_budget is not None:
        ok = total_ram <= args.ram_budget
        print(f"RAM budget {args.ram_budget} B: {'PASS' if ok else 'FAIL'}")
        return 0 if ok else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
