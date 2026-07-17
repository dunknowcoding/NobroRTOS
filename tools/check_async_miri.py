#!/usr/bin/env python3
"""Run Miri over bounded async and one-shot executor-cell initialization."""

import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def host_target() -> str:
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.split(":", 1)[1].strip()
                for line in output.splitlines() if line.startswith("host:"))


def main() -> int:
    env = dict(os.environ)
    # Tests model embedded static-cell placement with Box::leak. Those allocations
    # are intentionally permanent; all other Miri checks remain enabled.
    flags = env.get("MIRIFLAGS", "").strip()
    env["MIRIFLAGS"] = f"{flags} -Zmiri-ignore-leaks".strip()
    filters = [
        "async_rt::tests",
        "async_mpmc::tests",
        "health::tests::in_place_initialization_matches_const_constructor",
        "graph::tests::reactor_domain_linkage",
        "graph::tests::reactor_runtime_binding",
        "graph::tests::graph_spec_starts_and_seals_executor_without_retained_built_graph",
        "graph::tests::graph_validation_failure_does_not_claim_the_executor_cell",
        "graph::tests::graph_executor_init_failure_restores_the_cell_for_retry",
        "kernel_executor::tests::executor_graph_workspace_keeps_scratch_disjoint_from_admission",
    ]
    for test_filter in filters:
        command = ["cargo", "+nightly", "miri", "test", "--locked", "--target", host_target(),
                   "-p", "nobro-kernel", test_filter]
        completed = subprocess.run(command, cwd=ROOT / "core", env=env)
        if completed.returncode != 0:
            return completed.returncode
    command = [
        "cargo", "+nightly", "miri", "test", "--locked", "--target", host_target(),
        "-p", "nobro-hal", "--no-default-features",
        "--features", "platform-nrf52840,board-promicro-nosd,nrf-twim-async",
        "twim_hw::async_provider::tests",
    ]
    completed = subprocess.run(command, cwd=ROOT / "core", env=env)
    if completed.returncode != 0:
        return completed.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
