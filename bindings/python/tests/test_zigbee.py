import unittest

from nobro_rtos.zigbee import (
    AddrMode,
    FrameType,
    GatewayRollup,
    ZigbeeDecodeError,
    decode_mac_frame,
)


class DecodeTests(unittest.TestCase):
    def test_ack_frame_is_fcf_plus_seq(self):
        # FCF 0x0002 (type=ACK), seq=0x2A
        f = decode_mac_frame(bytes([0x02, 0x00, 0x2A]))
        self.assertEqual(f.frame_type, FrameType.ACK)
        self.assertEqual(f.seq, 0x2A)
        self.assertIsNone(f.dest_addr)

    def test_beacon_request_command(self):
        # FCF 0x0803: MAC command, dest addr mode = short; dest PAN/addr = 0xFFFF;
        # command id 0x07 = beacon request.
        psdu = bytes([0x03, 0x08, 0x11, 0xFF, 0xFF, 0xFF, 0xFF, 0x07])
        f = decode_mac_frame(psdu)
        self.assertEqual(f.frame_type, FrameType.MAC_COMMAND)
        self.assertEqual(f.dest_pan, 0xFFFF)
        self.assertEqual(f.dest_addr, 0xFFFF)
        self.assertEqual(f.command, "beacon_request")
        self.assertEqual(f.seq, 0x11)

    def test_data_frame_short_addrs_with_pan_compression(self):
        # FCF 0x8861: data(1), ack_req(bit5), pan_comp(bit6), dest short, src short.
        # dest PAN 0x1234, dest 0x0002, src (compressed PAN) 0x0001, payload 2 bytes.
        psdu = bytes([
            0x61, 0x88, 0x3D,        # FCF, seq
            0x34, 0x12, 0x02, 0x00,  # dest PAN 0x1234, dest 0x0002
            0x01, 0x00,              # src 0x0001 (PAN compressed)
            0xAB, 0xCD,              # payload
        ])
        f = decode_mac_frame(psdu)
        self.assertEqual(f.frame_type, FrameType.DATA)
        self.assertTrue(f.ack_request)
        self.assertTrue(f.pan_id_compression)
        self.assertEqual(f.dest_pan, 0x1234)
        self.assertEqual(f.dest_addr, 0x0002)
        self.assertEqual(f.src_pan, 0x1234)  # reused via compression
        self.assertEqual(f.src_addr, 0x0001)
        self.assertEqual(f.payload_len, 2)

    def test_extended_source_address(self):
        # FCF 0xC001: type=1 data, dest mode none, src mode extended (bits14-15=11).
        psdu = bytes([0x01, 0xC0, 0x05, 0x99, 0x00,  # FCF, seq, src PAN 0x0099
                      1, 2, 3, 4, 5, 6, 7, 8])       # src ext addr (8 bytes LE)
        f = decode_mac_frame(psdu)
        self.assertEqual(f.frame_type, FrameType.DATA)
        self.assertEqual(f.src_addr, 0x0807060504030201)

    def test_fcs_is_stripped_when_requested(self):
        f = decode_mac_frame(bytes([0x02, 0x00, 0x2A, 0xDE, 0xAD]), has_fcs=True)
        self.assertEqual(f.frame_type, FrameType.ACK)
        self.assertEqual(f.raw_len, 3)

    def test_truncated_frame_raises(self):
        with self.assertRaises(ZigbeeDecodeError):
            decode_mac_frame(bytes([0x61, 0x88]))  # FCF but no seq/addrs

    def test_record_schema_is_collector_friendly(self):
        f = decode_mac_frame(bytes([0x03, 0x08, 0x11, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]))
        rec = f.to_record()
        self.assertEqual(rec["proto"], "802.15.4")
        self.assertEqual(rec["type"], "mac_command")
        self.assertEqual(rec["dest_pan"], "0xFFFF")
        self.assertEqual(rec["command"], "beacon_request")


class RollupTests(unittest.TestCase):
    def test_rollup_counts_by_type(self):
        roll = GatewayRollup()
        roll.ingest(decode_mac_frame(bytes([0x02, 0x00, 0x01])))          # ack
        roll.ingest(decode_mac_frame(bytes([0x02, 0x00, 0x02])))          # ack
        roll.ingest(decode_mac_frame(
            bytes([0x03, 0x08, 0x11, 0xFF, 0xFF, 0xFF, 0xFF, 0x07])))     # cmd
        rec = roll.to_record()
        self.assertEqual(rec["frames"], 3)
        self.assertEqual(rec["by_type"]["ack"], 2)
        self.assertEqual(rec["by_type"]["mac_command"], 1)


if __name__ == "__main__":
    unittest.main()
