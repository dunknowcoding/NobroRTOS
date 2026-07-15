#!/usr/bin/env python3
"""Build-time stack, RAM/flash, and static timing budget report for NobroRTOS.

Most RTOSes size stacks by guesswork and find out at runtime (stack canaries,
HardFaults). NobroRTOS apps are static and heap-free, so the worst case is computable
at build time: this tool disassembles the ELF (arm-none-eabi-objdump), extracts each
function's frame size from its prologue (push/vpush/sub sp), builds the call graph
from bl/b.w edges, and reports the deepest stack path plus static RAM and flash use.
It also estimates a static instruction-cycle envelope for the same call graph and can
gate a cycle budget. Recursive, looped, indirect-call, or unknown-instruction paths are
flagged rather than silently treated as fully priced.

  python3 tools/static_budget.py path/to/app.elf [--objdump PATH] [--flash-budget BYTES]
  python3 tools/static_budget.py path/to/app.elf [--static-ram-budget BYTES]
  python3 tools/static_budget.py path/to/app.elf [--ram-budget BYTES] [--stack-budget BYTES]
  python3 tools/static_budget.py path/to/app.elf [--cycle-budget CYCLES] [--clock-hz HZ]

Exit code 1 if any requested budget is exceeded.
"""
import argparse
from pathlib import Path
import re
import shutil
import subprocess
import sys
from collections import defaultdict

DEFAULT_OBJDUMPS = [
    "arm-none-eabi-objdump",
]


class BudgetToolError(RuntimeError):
    """Actionable failure from an external budget-analysis tool."""


def run_external_tool(cmd: list[str], what: str) -> str:
    """Run a required binary and turn tool failures into one-line gate errors."""

    try:
        return subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except FileNotFoundError as exc:
        raise BudgetToolError(
            f"{what} failed: required tool '{cmd[0]}' was not found; "
            "install the Arm GNU toolchain or pass --objdump"
        ) from exc
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or exc.stdout or "").strip().splitlines()
        suffix = f": {detail[0]}" if detail else ""
        raise BudgetToolError(
            f"{what} failed: {' '.join(cmd)} exited {exc.returncode}{suffix}"
        ) from exc

SYM_RE = re.compile(r"^([0-9a-f]+) <([^>]+)>:$")
PUSH_RE = re.compile(r"\bpush(?:\.w)?\s+\{([^}]*)\}")
VPUSH_RE = re.compile(r"\bvpush\s+\{([^}]*)\}")
SUBSP_RE = re.compile(r"\bsub(?:\.w)?\s+sp,(?:\s*sp,)?\s*#(\d+)")
CALL_RE = re.compile(r"\bbl(?:\.w)?\s+[0-9a-f]+ <([^>+]+)(?:\+0x[0-9a-f]+)?>")
TAILCALL_RE = re.compile(r"\bb(?:\.w)?\s+[0-9a-f]+ <([^>+]+)(?:\+0x[0-9a-f]+)?>")
INDIRECT_RE = re.compile(r"\bblx\s+r\d+")
MNEMONIC_RE = re.compile(r"^\s*([0-9a-f]+):\s+([a-z][a-z0-9.]*)")
BRANCH_TARGET_RE = re.compile(r"^\s*([0-9a-f]+):.*\bb(?:\.[a-z0-9]+)?\s+([0-9a-f]+)\s+<")

SINGLE_CYCLE_OPS = {
    "adc", "add", "adr", "and", "asr", "bic", "cmn", "cmp", "cpsid", "cpsie",
    "dmb", "dsb", "eor", "isb", "lsl", "lsr", "mov", "movs", "mrs", "msr",
    "mul", "mvn", "neg", "nop", "orr", "rev", "rev16", "revsh", "ror", "rsb",
    "sbc", "sev", "smlabb", "smulbb", "sub", "svc", "sxtb", "sxth", "teq", "tst",
    "uxtb", "uxth", "wfe", "wfi", "yield",
}

LOAD_STORE_OPS = {
    "ldrb", "ldrd", "ldrh", "ldr", "ldrsb", "ldrsh", "strb", "strd", "strh", "str",
}

BRANCH_OPS = {
    "b", "beq", "bne", "bcs", "bcc", "bmi", "bpl", "bvs", "bvc", "bhi", "bls",
    "bge", "blt", "bgt", "ble", "bal", "cbz", "cbnz", "bx",
}


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


def normalize_mnemonic(op: str) -> str:
    op = op.lower()
    for suffix in (".w", ".n"):
        if op.endswith(suffix):
            return op[: -len(suffix)]
    return op


def instruction_cycles(line: str) -> tuple[int, str | None]:
    match = MNEMONIC_RE.match(line)
    if not match:
        return 0, None

    op = normalize_mnemonic(match.group(2))
    if op in ("push", "pop"):
        regs = PUSH_RE.search(line) or re.search(r"\bpop(?:\.w)?\s+\{([^}]*)\}", line)
        return (1 + reg_count(regs.group(1)) if regs else 2), None
    if op in ("vpush", "vpop"):
        regs = VPUSH_RE.search(line) or re.search(r"\bvpop\s+\{([^}]*)\}", line)
        return (1 + 2 * reg_count(regs.group(1)) if regs else 3), None
    if op in ("ldm", "ldmia", "stm", "stmia"):
        regs = re.search(r"\{([^}]*)\}", line)
        return (1 + reg_count(regs.group(1)) if regs else 2), None
    if op in ("bl", "blx"):
        return 3, None
    if op in BRANCH_OPS:
        return 2, None
    if op in LOAD_STORE_OPS:
        return 2, None
    if op in ("sdiv", "udiv"):
        return 12, None
    if op.startswith("v"):
        return 4, None
    if op in SINGLE_CYCLE_OPS:
        return 1, None
    return 1, op


def analyze_disassembly(out: str):
    frames: dict[str, int] = defaultdict(int)
    cycles: dict[str, int] = defaultdict(int)
    calls: dict[str, set] = defaultdict(set)
    indirect: set = set()
    loops: set = set()
    unknown_cycles: dict[str, set] = defaultdict(set)
    cur = None
    for line in out.splitlines():
        m = SYM_RE.match(line)
        if m:
            cur = m.group(2)
            frames.setdefault(cur, 0)
            cycles.setdefault(cur, 0)
            continue
        if cur is None:
            continue
        local_cycles, unknown = instruction_cycles(line)
        cycles[cur] += local_cycles
        if unknown:
            unknown_cycles[cur].add(unknown)
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
        if m := BRANCH_TARGET_RE.match(line):
            src = int(m.group(1), 16)
            dst = int(m.group(2), 16)
            if dst < src:
                loops.add(cur)
        if INDIRECT_RE.search(line):
            indirect.add(cur)

    return frames, cycles, calls, sorted(indirect), sorted(loops), unknown_cycles


def deepest_path(costs: dict[str, int], calls: dict[str, set]) -> tuple[int, list, list]:
    depth_cache: dict[str, tuple[int, list]] = {}

    def depth(fn: str, stack: tuple) -> tuple[int, list]:
        if fn in stack:
            return (0, ["<recursion>"])
        if fn in depth_cache:
            return depth_cache[fn]
        best, best_path = 0, []
        for callee in calls.get(fn, ()):
            if callee not in costs:
                continue
            d, p = depth(callee, stack + (fn,))
            if d > best:
                best, best_path = d, p
        result = (costs.get(fn, 0) + best, [fn] + best_path)
        depth_cache[fn] = result
        return result

    roots = [f for f in costs if f in ("main", "Reset", "Reset_Handler", "__cortex_m_rt_main")]
    if not roots:
        roots = list(costs)
    worst, worst_path = 0, []
    for r in roots:
        d, p = depth(r, ())
        if d > worst:
            worst, worst_path = d, p
    recursive = sorted(f for f, (_, p) in depth_cache.items() if "<recursion>" in p)
    return worst, worst_path, recursive


def analyze(elf: str, objdump: str):
    out = run_external_tool(
        [objdump, "-d", "--no-show-raw-insn", elf],
        "disassembly",
    )
    frames, cycles, calls, indirect, loops, unknown_cycles = analyze_disassembly(out)
    stack_worst, stack_path, recursive = deepest_path(frames, calls)
    cycle_worst, cycle_path, cycle_recursive = deepest_path(cycles, calls)
    recursive = sorted(set(recursive).union(cycle_recursive))
    return (
        frames,
        cycles,
        stack_worst,
        stack_path,
        cycle_worst,
        cycle_path,
        indirect,
        loops,
        recursive,
        unknown_cycles,
    )


def sizes(elf: str, objdump: str):
    size_tool = objdump.replace("objdump", "size")
    out = run_external_tool([size_tool, elf], "size report")
    line = out.strip().splitlines()[-1].split()
    text, data, bss = int(line[0]), int(line[1]), int(line[2])
    return text, data, bss


def format_path(path: list) -> str:
    return f"{' -> '.join(path[:8])}{' ...' if len(path) > 8 else ''}"


def run_selftest() -> int:
    sample = """
00000000 <main>:
   0: push {r4, lr}
   2: sub sp, #8
   4: bl 00000010 <foo>
   8: pop {r4, pc}
00000010 <foo>:
  10: push {lr}
  12: ldr r0, [r0]
  14: udiv r0, r0, r1
  18: blx r3
  1a: b.n 00000012 <foo+0x2>
  1c: bx lr
"""
    frames, cycles, calls, indirect, loops, unknown = analyze_disassembly(sample)
    stack_worst, stack_path, recursive = deepest_path(frames, calls)
    cycle_worst, cycle_path, _ = deepest_path(cycles, calls)
    assert frames["main"] == 16
    assert frames["foo"] == 4
    assert stack_worst == 20
    assert stack_path[:2] == ["main", "foo"]
    assert cycle_worst >= 29
    assert cycle_path[:2] == ["main", "foo"]
    assert "foo" in indirect
    assert "foo" in loops
    assert recursive == []
    assert unknown == {}
    try:
        run_external_tool(
            [sys.executable, "-c", "import sys; sys.stderr.write('boom\\n'); sys.exit(7)"],
            "synthetic budget tool",
        )
        raise AssertionError("expected external-tool failure")
    except BudgetToolError as exc:
        assert "synthetic budget tool failed" in str(exc)
        assert "exited 7" in str(exc)
        assert "boom" in str(exc)
    print("static_budget selftest: PASS")
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("elf", nargs="?")
    ap.add_argument("--objdump", default=None)
    ap.add_argument("--flash-budget", type=int, default=None,
                    help="fail if flash text+data exceeds this")
    ap.add_argument("--static-ram-budget", type=int, default=None,
                    help="fail if static RAM data+bss exceeds this")
    ap.add_argument("--ram-budget", type=int, default=None,
                    help="fail if static RAM + worst-case stack exceeds this")
    ap.add_argument("--stack-budget", type=int, default=None,
                    help="fail if the computed worst-case stack exceeds this")
    ap.add_argument("--cycle-budget", type=int, default=None,
                    help="fail if the static deepest-path cycle estimate exceeds this")
    ap.add_argument("--clock-hz", type=int, default=None,
                    help="print the static cycle estimate as microseconds at this clock")
    ap.add_argument("--top", type=int, default=5, help="largest frames to list")
    ap.add_argument("--selftest", action="store_true",
                    help="run the parser and timing estimator self-test")
    args = ap.parse_args()

    if args.selftest:
        return run_selftest()
    if not args.elf:
        ap.error("elf is required unless --selftest is used")
    if not Path(args.elf).is_file():
        sys.exit(f"ELF not found: {args.elf}")

    objdump = args.objdump or next(
        (t for t in DEFAULT_OBJDUMPS if shutil.which(t) or t.endswith(".exe")), None)
    if objdump is None:
        sys.exit("no arm-none-eabi-objdump found; pass --objdump")

    try:
        (
            frames,
            cycles,
            worst,
            path,
            worst_cycles,
            cycle_path,
            indirect,
            loops,
            recursive,
            unknown_cycles,
        ) = analyze(args.elf, objdump)
        text, data, bss = sizes(args.elf, objdump)
    except BudgetToolError as exc:
        sys.exit(str(exc))
    static_ram = data + bss
    total_ram = static_ram + worst

    print(f"flash (text+data):        {text + data:8d} B")
    print(f"static RAM (data+bss):    {static_ram:8d} B")
    print(f"worst-case stack:         {worst:8d} B")
    print(f"  deepest path: {format_path(path)}")
    print(f"worst-case total RAM:     {total_ram:8d} B")
    biggest = sorted(frames.items(), key=lambda kv: -kv[1])[:args.top]
    print("largest frames: " + ", ".join(f"{f}={s}B" for f, s in biggest))
    print(f"worst-case static cycles: {worst_cycles:8d} cycles")
    print(f"  cycle path: {format_path(cycle_path)}")
    if args.clock_hz:
        time_us = (worst_cycles * 1_000_000.0) / args.clock_hz
        print(f"  at {args.clock_hz} Hz: {time_us:.2f} us")
    cycle_biggest = sorted(cycles.items(), key=lambda kv: -kv[1])[:args.top]
    print("largest cycle bodies: " + ", ".join(f"{f}={s}cy" for f, s in cycle_biggest))
    if indirect:
        print(f"CAUTION {len(indirect)} function(s) make indirect calls (unpriced): "
              + ", ".join(indirect[:4]) + ("..." if len(indirect) > 4 else ""))
    if loops:
        print(f"CAUTION {len(loops)} function(s) contain backward branches/loops: "
              + ", ".join(loops[:4]) + ("..." if len(loops) > 4 else ""))
    if recursive:
        print(f"CAUTION recursion detected in: {', '.join(recursive[:4])}")
    if unknown_cycles:
        names = sorted(unknown_cycles)[:4]
        details = ", ".join(f"{name}:{'/'.join(sorted(unknown_cycles[name]))}" for name in names)
        print(f"CAUTION {len(unknown_cycles)} function(s) use estimated unknown mnemonics: {details}"
              + ("..." if len(unknown_cycles) > 4 else ""))

    ok = True
    if args.flash_budget is not None:
        flash_ok = text + data <= args.flash_budget
        print(f"flash budget {args.flash_budget} B: {'PASS' if flash_ok else 'FAIL'}")
        ok = ok and flash_ok
    if args.static_ram_budget is not None:
        static_ram_ok = static_ram <= args.static_ram_budget
        print(
            f"static RAM budget {args.static_ram_budget} B: "
            f"{'PASS' if static_ram_ok else 'FAIL'}"
        )
        ok = ok and static_ram_ok
    if args.ram_budget is not None:
        ram_ok = total_ram <= args.ram_budget
        print(f"RAM budget {args.ram_budget} B: {'PASS' if ram_ok else 'FAIL'}")
        ok = ok and ram_ok
    if args.stack_budget is not None:
        stack_ok = worst <= args.stack_budget
        print(f"stack budget {args.stack_budget} B: {'PASS' if stack_ok else 'FAIL'}")
        ok = ok and stack_ok
    if args.cycle_budget is not None:
        cycle_ok = worst_cycles <= args.cycle_budget
        print(f"cycle budget {args.cycle_budget} cycles: {'PASS' if cycle_ok else 'FAIL'}")
        ok = ok and cycle_ok
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
