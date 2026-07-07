import unittest

from nobro_rtos.thread import (
    LowpanKind,
    ThreadRollup,
    classify_dispatch,
    decode_lowpan,
    decode_thread_record,
    decode_thread_frame,
)


class DispatchTests(unittest.TestCase):
    def test_dispatch_classification(self):
        self.assertEqual(classify_dispatch(0x41), LowpanKind.UNCOMPRESSED_IPV6)
        self.assertEqual(classify_dispatch(0x7A), LowpanKind.IPHC)       # 011xxxxx
        self.assertEqual(classify_dispatch(0xB2), LowpanKind.MESH)       # 10xxxxxx
        self.assertEqual(classify_dispatch(0xC1), LowpanKind.FRAG_FIRST) # 11000xxx
        self.assertEqual(classify_dispatch(0xE1), LowpanKind.FRAG_N)     # 11100xxx
        self.assertEqual(classify_dispatch(0x00), LowpanKind.NALP)


class LowpanTests(unittest.TestCase):
    def test_plain_iphc_is_thread_traffic(self):
        h = decode_lowpan(bytes([0x7A, 0x33, 0x3A, 0x00]))
        self.assertEqual(h.kinds, [LowpanKind.IPHC])
        self.assertTrue(h.is_thread_lowpan)

    def test_mesh_then_iphc(self):
        # mesh 0xB2 = V short, F short, hops=2; orig(2) dest(2); then IPHC
        payload = bytes([0xB2, 0x00, 0x02, 0x00, 0x03, 0x7A, 0x33])
        h = decode_lowpan(payload)
        self.assertEqual(h.kinds, [LowpanKind.MESH, LowpanKind.IPHC])
        self.assertEqual(h.mesh_hops_left, 2)
        self.assertTrue(h.is_thread_lowpan)

    def test_first_fragment_reports_datagram_size(self):
        # frag-first 0xC1 0x50 -> size ((1)<<8)|0x50 = 336; tag(2); then IPHC
        payload = bytes([0xC1, 0x50, 0xAB, 0xCD, 0x7A, 0x33])
        h = decode_lowpan(payload)
        self.assertEqual(h.kinds, [LowpanKind.FRAG_FIRST, LowpanKind.IPHC])
        self.assertEqual(h.frag_datagram_size, 336)
        self.assertTrue(h.is_thread_lowpan)

    def test_non_lowpan_payload_is_not_thread(self):
        h = decode_lowpan(bytes([0x00, 0x11, 0x22]))
        self.assertEqual(h.kinds, [LowpanKind.NALP])
        self.assertFalse(h.is_thread_lowpan)


class ThreadFrameTests(unittest.TestCase):
    def test_captured_frame_decodes_mac_plus_lowpan(self):
        # a real-shaped 802.15.4 data frame (short addrs, PAN compression) carrying an
        # IPHC 6LoWPAN payload - what the CC2530 gateway captures from a NiusThread node
        psdu = bytes([
            0x61, 0x88, 0x3D,        # FCF data+ackreq+pancomp, seq
            0x34, 0x12, 0x02, 0x00,  # dest PAN 0x1234, dest 0x0002
            0x01, 0x00,              # src 0x0001
            0x7A, 0x33, 0x3A, 0x05,  # 6LoWPAN IPHC payload
        ])
        tf = decode_thread_frame(psdu)
        self.assertEqual(tf.mac.dest_pan, 0x1234)
        self.assertTrue(tf.lowpan.is_thread_lowpan)
        rec = tf.to_record()
        self.assertEqual(rec["proto"], "thread")
        self.assertEqual(rec["l3"], "6lowpan")
        self.assertIn("iphc", rec["headers"])
        self.assertTrue(rec["thread"])

    def test_thread_record_returns_none_for_non_lowpan(self):
        psdu = bytes([
            0x61, 0x88, 0x3D,
            0x34, 0x12, 0x02, 0x00,
            0x01, 0x00,
            0x00, 0x11, 0x22,
        ])
        self.assertIsNone(decode_thread_record(psdu))

    def test_thread_rollup_counts_headers(self):
        iphc_psdu = bytes([
            0x61, 0x88, 0x3D,
            0x34, 0x12, 0x02, 0x00,
            0x01, 0x00,
            0x7A, 0x33, 0x3A, 0x05,
        ])
        mesh_psdu = bytes([
            0x61, 0x88, 0x3E,
            0x34, 0x12, 0x02, 0x00,
            0x01, 0x00,
            0xB2, 0x00, 0x02, 0x00, 0x03, 0x7A, 0x33,
        ])
        roll = ThreadRollup()
        roll.ingest(iphc_psdu)
        roll.ingest(mesh_psdu)
        rec = roll.to_record()
        self.assertEqual(rec["frames"], 2)
        self.assertEqual(rec["thread_frames"], 2)
        self.assertEqual(rec["headers"]["iphc"], 2)
        self.assertEqual(rec["headers"]["mesh"], 1)


if __name__ == "__main__":
    unittest.main()
