"""Declare once, simulate on the host, then export native-firmware input."""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bindings" / "python"))

from nobro_rtos import HZ, NobroApp  # noqa: E402


def build_app() -> NobroApp:
    return (
        NobroApp("python_rover", board="nrf52840-nosd")
        .task("motor", HZ(200), role="control")
        .task("imu", HZ(100))
        .task("camera", HZ(25), role="service")
        .wire("imu", "motor", 8)
    )


def main() -> int:
    output = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("_work/python-rover/app.json")
    app = build_app()
    report = app.run(50_000)
    app.write_json(output)
    print(f"PYTHON APP: {report.event_count} simulated releases; wrote {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
