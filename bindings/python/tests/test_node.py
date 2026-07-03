import unittest

from nobro_rtos.node import (
    NobroNode,
    parse_status_line,
    parse_telemetry_line,
)


class FakeTransport:
    """Stdlib stand-in for a serial port: scripted RX, captured TX."""

    def __init__(self, lines):
        self._rx = [line.encode() + b"\n" for line in lines]
        self.tx = b""
        self.closed = False

    def readline(self):
        return self._rx.pop(0) if self._rx else b""

    def write(self, data):
        self.tx += data

    def close(self):
        self.closed = True


class StatusLineTests(unittest.TestCase):
    def test_parses_name_and_typed_fields(self):
        report = parse_status_line(
            "NOBRO-USB-SJ backend=NUSJ configured=1 beat=42\r\n"
        )
        self.assertIsNotNone(report)
        self.assertEqual(report.name, "USB-SJ")
        self.assertEqual(report.fields["backend"], "NUSJ")
        self.assertEqual(report.fields["configured"], 1)
        self.assertEqual(report.fields["beat"], 42)

    def test_rejects_non_status_lines(self):
        self.assertIsNone(parse_status_line("hello world"))
        self.assertIsNone(parse_status_line('{"chip": "INA3221"}'))
        self.assertIsNone(parse_status_line("NOBRO-"))

    def test_float_fields_survive(self):
        report = parse_status_line("NOBRO-PWR bus_V=4.96 ok=1")
        self.assertAlmostEqual(report.fields["bus_V"], 4.96)


class TelemetryLineTests(unittest.TestCase):
    def test_parses_jsonl(self):
        sample = parse_telemetry_line('{"chip":"RA4M1","rssi":-40}')
        self.assertEqual(sample["chip"], "RA4M1")
        self.assertEqual(sample["rssi"], -40)

    def test_rejects_garbage(self):
        self.assertIsNone(parse_telemetry_line("NOBRO-C3 all_pass=1"))
        self.assertIsNone(parse_telemetry_line("{broken"))
        self.assertIsNone(parse_telemetry_line("[1, 2, 3]"))


class NodeTests(unittest.TestCase):
    def test_reports_and_telemetry_streams(self):
        fake = FakeTransport(
            [
                "boot noise",
                "NOBRO-C3 arch=riscv32imc subsystems=7 all_pass=1",
                '{"chip":"INA3221","transport":"wifi"}',
                "NOBRO-RP2350 arch=thumbv8m all_pass=1 cores=2",
            ]
        )
        node = NobroNode(transport=fake)
        reports = list(node.reports(seconds=0.5))
        self.assertEqual([r.name for r in reports], ["C3", "RP2350"])
        self.assertEqual(reports[0].fields["all_pass"], 1)

        fake2 = FakeTransport(['{"chip":"INA3221"}', "not json"])
        node2 = NobroNode(transport=fake2)
        samples = list(node2.telemetry(seconds=0.5))
        self.assertEqual(len(samples), 1)
        self.assertEqual(samples[0]["chip"], "INA3221")

    def test_wait_report_filters_by_name(self):
        fake = FakeTransport(
            ["NOBRO-A x=1", "NOBRO-B y=2", "NOBRO-C z=3"]
        )
        node = NobroNode(transport=fake)
        report = node.wait_report("B", seconds=0.5)
        self.assertIsNotNone(report)
        self.assertEqual(report.fields["y"], 2)

    def test_send_and_dfu_write_lines(self):
        fake = FakeTransport([])
        node = NobroNode(transport=fake)
        node.send_line("PING")
        node.request_dfu()
        self.assertEqual(fake.tx, b"PING\nDFU\n")

    def test_context_manager_closes_transport(self):
        fake = FakeTransport([])
        with NobroNode(transport=fake):
            pass
        self.assertTrue(fake.closed)

    def test_requires_port_or_transport(self):
        with self.assertRaises(ValueError):
            NobroNode()


if __name__ == "__main__":
    unittest.main()
