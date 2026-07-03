import unittest

from nobro_rtos.nn_export import (
    MODEL_MAGIC,
    dense,
    dequantize_int8,
    evaluate,
    export_model,
    fnv1a,
    quantize_int8,
    train_dense,
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


if __name__ == "__main__":
    unittest.main()
