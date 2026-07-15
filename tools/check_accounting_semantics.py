#!/usr/bin/env python3
"""Guard the executor accounting contract.

Executor bookkeeping is currently synchronous because NobroRTOS does not yet
have an admitted maintenance-service reserve or saturated debt model. This gate
prevents a future regression where accounting is quietly deferred in the hot
executor path without first adding that reserve/debt proof.
"""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT / "core/crates/nobro_kernel/src"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def main() -> int:
    kernel_executor = read(KERNEL / "kernel_executor.rs")
    instrumentation = read(KERNEL / "instrumentation.rs")
    architecture = read(ROOT / "docs/ARCHITECTURE.md")
    limitations = read(ROOT / "docs/LIMITATIONS.md")

    failures: list[str] = []
    required = {
        "kernel_executor": (
            "recorder.record_bookkeeping(end_us, bookkeeping_finished_us);",
            "work_pending = self.tasks.has_due(power_now_us)",
            "snapshot, never the stale poll-end/bookkeeping values",
        ),
        "instrumentation": (
            "pub(crate) fn record_bookkeeping",
            "poll_bookkeeping_samples",
            "poll_bookkeeping_max_us",
        ),
        "architecture": (
            "Bookkeeping remains synchronous in the current executor",
            "maintenance-service reserve",
            "saturated debt model",
        ),
        "limitations": (
            "Executor accounting",
            "No admitted maintenance-service reserve or saturated accounting-debt model exists yet",
        ),
    }
    texts = {
        "kernel_executor": kernel_executor,
        "instrumentation": instrumentation,
        "architecture": architecture,
        "limitations": limitations,
    }
    for name, tokens in required.items():
        for token in tokens:
            if token not in texts[name]:
                failures.append(f"{name}: missing {token!r}")

    forbidden = (
        "DeferredAccounting",
        "deferred_accounting",
        "defer_accounting",
        "AccountingDebt",
        "accounting_debt",
        "maintenance_debt",
        "bookkeeping_debt",
    )
    for path in KERNEL.rglob("*.rs"):
        text = read(path)
        for token in forbidden:
            if token in text:
                failures.append(
                    f"{path.relative_to(ROOT)}: deferred accounting token {token!r} "
                    "requires an admitted reserve/debt proof before it can ship"
                )

    if failures:
        print("ACCOUNTING SEMANTICS GATE: FAIL")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(
        "ACCOUNTING SEMANTICS GATE: PASS "
        "(bookkeeping is inline; no deferred accounting reserve/debt is claimed)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
