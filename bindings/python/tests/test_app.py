import json
from pathlib import Path
import tempfile
import unittest

from nobro_rtos import (
    APP_SCHEMA,
    AppCallbackError,
    AppDeclarationError,
    AppSimulationError,
    HZ,
    NobroApp,
)


class PythonAppTests(unittest.TestCase):
    def sample(self) -> NobroApp:
        return (
            NobroApp("rover", board="nrf52840-s140")
            .task("motor", HZ(200), role="control")
            .task("imu", HZ(100), role="periodic", phase_us=1_000)
            .task("camera", HZ(25), role="service")
            .wire("imu", "motor", 8)
        )

    def test_round_trip_and_firmware_spec_share_one_graph(self) -> None:
        app = self.sample()
        document = app.to_dict()
        restored = NobroApp.from_dict(document)
        spec = restored.firmware_spec()

        self.assertEqual(document["schema"], APP_SCHEMA)
        self.assertEqual(restored.to_dict(), document)
        self.assertEqual(spec["board"], "nrf52840-s140")
        self.assertEqual(spec["workload"]["channels"], [["imu", "motor"]])
        self.assertEqual(
            spec["workload"]["wire_capacities"],
            [["imu", "motor", 8]],
        )
        self.assertEqual(
            [task["name"] for task in spec["workload"]["tasks"]],
            ["kernel", "motor", "imu", "camera"],
        )

    def test_write_and_read_json_does_not_serialize_callbacks(self) -> None:
        calls = []
        app = NobroApp("blink").task(
            "led",
            HZ(2),
            lambda context: calls.append(context.now_us),
        )
        with tempfile.TemporaryDirectory() as tmp:
            path = app.write_json(Path(tmp) / "app.json")
            payload = json.loads(path.read_text(encoding="utf-8"))
            restored = NobroApp.read_json(
                path,
                steps={"led": lambda context: calls.append(context.now_us)},
            )

        self.assertNotIn("step", payload["tasks"][0])
        restored.run(1_000_001)
        self.assertEqual(calls, [0, 500_000, 1_000_000])

    def test_simulation_is_phase_ordered_and_declaration_stable(self) -> None:
        observed = []
        app = (
            NobroApp("timing")
            .task("first", 5_000, lambda context: observed.append(
                (context.task, context.now_us, context.release)
            ))
            .task("second", 10_000, lambda context: observed.append(
                (context.task, context.now_us, context.release)
            ), phase_us=5_000)
        )

        report = app.simulate(16_000)

        self.assertEqual(
            [(event.task, event.at_us) for event in report.events],
            [
                ("first", 0),
                ("first", 5_000),
                ("second", 5_000),
                ("first", 10_000),
                ("first", 15_000),
                ("second", 15_000),
            ],
        )
        self.assertEqual(report.runs, {"first": 4, "second": 2})
        self.assertEqual(observed[2], ("second", 5_000, 0))

    def test_callback_fault_and_running_state_fail_closed(self) -> None:
        app = NobroApp("fault")

        def invalid_mutation(_context) -> None:
            app.task("late", 1_000)

        app.task("worker", 1_000, invalid_mutation)
        with self.assertRaisesRegex(AppCallbackError, "cannot change"):
            app.run(1)

        app.task("after", 2_000)
        self.assertEqual([task.name for task in app.tasks], ["worker", "after"])

    def test_invalid_graphs_and_strict_schema_are_rejected(self) -> None:
        with self.assertRaises(AppDeclarationError):
            NobroApp("BadName")
        with self.assertRaisesRegex(AppDeclarationError, "unsupported board"):
            NobroApp("bad_board", board=[])
        with self.assertRaisesRegex(AppDeclarationError, "role must be a string"):
            NobroApp("bad_role").task("one", 1_000, role=[])
        with self.assertRaises(AppDeclarationError):
            HZ(0)
        app = NobroApp("invalid").task("one", 1_000)
        with self.assertRaisesRegex(AppDeclarationError, "unknown task"):
            app.wire("one", "missing")
        with self.assertRaisesRegex(AppDeclarationError, "duplicate task"):
            NobroApp("duplicate").task("one", 1_000).task("one", 2_000)

        document = self.sample().to_dict()
        document["unexpected"] = True
        with self.assertRaisesRegex(AppDeclarationError, "unknown unexpected"):
            NobroApp.from_dict(document)
        document = self.sample().to_dict()
        document["schema"] = []
        with self.assertRaisesRegex(AppDeclarationError, "unsupported app schema"):
            NobroApp.from_dict(document)

    def test_capacity_and_event_bounds_are_enforced(self) -> None:
        app = NobroApp("capacity")
        for index in range(8):
            app.task(f"task{index}", 1)
        with self.assertRaisesRegex(AppDeclarationError, "task capacity"):
            app.task("task8", 1)
        with self.assertRaisesRegex(AppSimulationError, "event limit"):
            app.run(2, max_events=8)

    def test_sensor_alias_normalizes_to_periodic_defaults(self) -> None:
        task = NobroApp("alias").task("imu", HZ(100), role="sensor").tasks[0]
        self.assertEqual(task.role, "periodic")
        self.assertEqual(task.deadline_us, 10_000)
        self.assertEqual(task.budget_us, 1_000)
        self.assertEqual(task.flash_bytes, 1024)
        self.assertEqual(task.ram_bytes, 256)

        legacy = NobroApp("legacy").task("imu", HZ(100)).to_dict()
        legacy["schema"] = "nobro-python-app-v1"
        self.assertEqual(NobroApp.from_dict(legacy).to_dict()["schema"], APP_SCHEMA)


if __name__ == "__main__":
    unittest.main()
