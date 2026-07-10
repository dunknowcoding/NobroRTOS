import unittest

from nobro_rtos.replay import (
    RECORD_SIZE,
    TraceRecord,
    decode_trace,
    encode_record,
    replay,
    to_audit,
)


def rec(seq, module, cap, op, at_us=0, result=0):
    return TraceRecord(seq=seq, at_us=at_us, module=module, capability=cap,
                       op=op, arg0=0, arg1=0, result=result)


class WireFormatTests(unittest.TestCase):
    def test_record_is_28_bytes_and_roundtrips(self):
        self.assertEqual(RECORD_SIZE, 28)
        r = rec(7, 4, 3, 3, at_us=123_456_789_012, result=42)  # sensor bus0 read
        back = decode_trace(encode_record(r))
        self.assertEqual(len(back), 1)
        self.assertEqual(back[0], r)
        self.assertEqual(back[0].at_us, 123_456_789_012)  # u64 survives

    def test_trailing_partial_bytes_ignored(self):
        blob = encode_record(rec(1, 0, 10, 5)) + b"\x00" * 5
        self.assertEqual(len(decode_trace(blob)), 1)

    def test_names_match_kernel_discriminants(self):
        r = rec(0, 8, 11, 5)  # ai / ai_inference / invoke
        d = r.to_dict()
        self.assertEqual((d["module"], d["capability"], d["op"]),
                         ("ai", "ai_inference", "invoke"))
        self.assertEqual(rec(0, 0x80 | 3, 0, 1).module_name, "app3")
        self.assertEqual(rec(0, 0, 0, 6).op_name, "fault")


class ReplayTests(unittest.TestCase):
    def _trace(self):
        return [
            rec(2, 4, 3, 3),          # sensor bus0 read
            rec(0, 4, 3, 1),          # sensor bus0 acquire
            rec(3, 8, 11, 5),         # ai inference invoke
            rec(1, 5, 6, 4),          # actuator servo_pwm write
            rec(4, 4, 3, 6),          # sensor bus0 FAULT
        ]

    def test_replay_orders_by_sequence(self):
        seqs = [r.seq for r in replay(self._trace())]
        self.assertEqual(seqs, [0, 1, 2, 3, 4])

    def test_scope_filters_mirror_kernel_scopes(self):
        t = self._trace()
        self.assertEqual(len(replay(t, module="sensor")), 3)
        self.assertEqual(len(replay(t, capability="bus0")), 3)
        self.assertEqual(len(replay(t, module="sensor", op="fault")), 1)
        self.assertEqual(len(replay(t, module="crypto")), 0)

    def test_audit_counts_and_faults(self):
        audit = to_audit(self._trace())
        self.assertEqual(audit["records"], 5)
        self.assertEqual(audit["faults"], 1)
        sensor_reads = [e for e in audit["by_scope"]
                        if e["module"] == "sensor" and e["op"] == "read"]
        self.assertEqual(sensor_reads[0]["count"], 1)
        self.assertEqual([e["seq"] for e in audit["trace"]], [0, 1, 2, 3, 4])


if __name__ == "__main__":
    unittest.main()
