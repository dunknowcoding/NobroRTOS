import unittest

from nobro_rtos.nn_export import (
    MODEL_MAGIC,
    dense,
    dequantize_int8,
    evaluate,
    export_model,
    export_packed_model,
    fnv1a,
    quantize_int8,
    quantize_packed,
    train_dense,
    unpack_packed,
)


class ChecksumParityTests(unittest.TestCase):
    def test_fnv1a_matches_the_rust_constant(self):
        # nobro-ai's Rust test pins the same vector: fnv1a(b"nobro") == 0xA76700F3.
        self.assertEqual(fnv1a(b"nobro"), 0xA76700F3)

    def test_magic_matches_nobro_ai(self):
        self.assertEqual(MODEL_MAGIC, 0x4E424D4C)


class InferenceParityTests(unittest.TestCase):
    def test_dense_matches_the_rust_reference_vector(self):
        # nobro-nn's dense_matches_hand_math test uses the same numbers.
        out = dense([10.0, 20.0], [1.0, 2.0, 3.0, 4.0], [0.5, -1.0])
        self.assertAlmostEqual(out[0], 50.5, places=4)
        self.assertAlmostEqual(out[1], 109.0, places=4)


class TrainingTests(unittest.TestCase):
    def _xor_free_dataset(self):
        # Linearly separable 2-class problem (sign of x0 - x1).
        samples, labels = [], []
        for a in range(-3, 4):
            for b in range(-3, 4):
                if a == b:
                    continue
                samples.append([float(a), float(b)])
                labels.append(0 if a > b else 1)
        return samples, labels

    def test_training_reaches_high_accuracy(self):
        samples, labels = self._xor_free_dataset()
        w, b = train_dense(samples, labels, in_len=2, out_len=2, epochs=150)
        self.assertGreaterEqual(evaluate(samples, labels, w, b), 0.95)

    def test_quantized_model_still_classifies(self):
        samples, labels = self._xor_free_dataset()
        w, b = train_dense(samples, labels, in_len=2, out_len=2, epochs=150)
        model = export_model("sign-net", 1, w, b)
        deq = dequantize_int8(model.weights, model.scale_milli)
        wq, bq = deq[: len(w)], deq[len(w):]
        self.assertGreaterEqual(evaluate(samples, labels, wq, bq), 0.9)

    def test_q4_candidate_and_q2_experimental_paths_are_evaluated(self):
        samples, labels = self._xor_free_dataset()
        weights, bias = train_dense(samples, labels, in_len=2, out_len=2, epochs=150)
        q4 = export_packed_model("sign-net", 1, weights, bias, "q4")
        q2 = export_packed_model("sign-net", 1, weights, bias, "q2")
        self.assertGreaterEqual(
            evaluate(samples, labels, q4.dequantized_weights(), q4.dequantized_bias()),
            0.95,
        )
        # Q2 is exercised end to end but remains experimental; each workload must
        # establish its own accuracy threshold before admission.
        self.assertGreaterEqual(
            evaluate(samples, labels, q2.dequantized_weights(), q2.dequantized_bias()),
            0.75,
        )


class ExportTests(unittest.TestCase):
    def test_quantize_roundtrip_error_is_bounded(self):
        vals = [0.0, 0.5, -0.5, 1.27, -1.27]
        blob, scale_milli = quantize_int8(vals)
        back = dequantize_int8(blob, scale_milli)
        step = scale_milli / 1000.0
        for v, r in zip(vals, back):
            self.assertLessEqual(abs(v - r), step)

    def test_manifest_fields_are_device_shaped(self):
        model = export_model("m", 3, [1.0, -1.0], [0.25], input_len=2)
        f = model.manifest_fields()
        self.assertEqual(f["magic"], MODEL_MAGIC)
        self.assertEqual(f["input_len"], 2)
        self.assertEqual(f["output_len"], 1)
        self.assertEqual(f["weights_len"], 3)  # 2 weights + 1 bias
        self.assertEqual(f["weights_crc"], fnv1a(model.weights))

    def test_q4_and_q2_layout_matches_device_golden_vectors(self):
        values = [-7.0, -1.0, 0.0, 1.0, 7.0, 1.0]
        q4, _, _ = quantize_packed(values, 3, "q4")
        self.assertEqual(q4, bytes([0xF9, 0x00, 0x71, 0x01]))
        self.assertEqual(unpack_packed(q4, 2, 3, "q4"), [-7, -1, 0, 1, 7, 1])

        q2, _, _ = quantize_packed([-1.0, 0.0, 1.0, -1.0, 1.0, 0.0], 3, "q2")
        self.assertEqual(q2, bytes([0x13, 0x07]))
        self.assertEqual(unpack_packed(q2, 2, 3, "q2"), [-1, 0, 1, -1, 1, 0])

    def test_packed_export_has_stable_manifest_and_explicit_maturity(self):
        q4 = export_packed_model("tiny", 2, [1.0, -0.5, 0.25], [0.125], "q4")
        fields = q4.manifest_fields()
        self.assertEqual(fields["layout_version"], 1)
        self.assertEqual(fields["weights_len"], 2)
        self.assertEqual(fields["weights_crc"], fnv1a(q4.packed_weights))
        self.assertEqual(fields["maturity"], "candidate")
        self.assertGreater(fields["input_scale_micros"], 0)
        self.assertGreater(fields["weight_scale_micros"], 0)
        self.assertGreater(fields["output_scale_micros"], 0)
        self.assertGreater(fields["multiplier"], 0)
        q2 = export_packed_model("tiny", 2, [1.0, -0.5, 0.25], [0.125], "q2")
        self.assertEqual(q2.maturity, "experimental")

    def test_packed_quantization_rejects_invalid_or_non_finite_input(self):
        with self.assertRaises(ValueError):
            quantize_packed([], 1, "q4")
        with self.assertRaises(ValueError):
            quantize_packed([float("nan")], 1, "q2")
        with self.assertRaises(ValueError):
            export_packed_model("bad", 1, [1.0, 2.0], [], "q4")

    def test_packed_export_runs_the_same_integer_requantization_contract(self):
        model = export_packed_model(
            "two-way",
            1,
            [1.0, -1.0, -1.0, 1.0],
            [0.0, 0.0],
            "q4",
            input_len=2,
            input_scale=0.25,
        )
        raw, real = model.infer([0.5, -0.5])
        self.assertEqual(raw, [28, -28])
        self.assertAlmostEqual(real[0], 1.0, places=4)
        self.assertAlmostEqual(real[1], -1.0, places=4)
        with self.assertRaises(ValueError):
            model.infer([0.5])


if __name__ == "__main__":
    unittest.main()
