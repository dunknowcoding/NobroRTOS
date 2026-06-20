"""Repository-local entry point for NobroRTOS Python contract tooling."""

from __future__ import annotations

from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
PYTHON_BINDINGS = ROOT / "bindings" / "python"
sys.path.insert(0, str(PYTHON_BINDINGS))

from nobro_rtos.cli import main


if __name__ == "__main__":
    raise SystemExit(main())
