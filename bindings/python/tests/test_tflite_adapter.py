import unittest

from nobro_rtos.tflite_adapter import TfliteDenseModel


class TfliteAdapterTests(unittest.TestCase):
    """Exercises the pure-Python conversion + inference (no tensorflow needed here;
    the live .tflite import is verified by the _work TF harness)."""

    def _model(self):
        # 2-in, 2-out int8 dense: class 1 iff x0 > x1. Symmetric int8, zero-point -128.
        return TfliteDenseModel(
            input_len=2,
            output_len=2,
            weights=[40, -40, -40, 40],  # class0 favors x0, class1 favors x1
            bias=[0, 0],
            input_scale=1.0 / 127,
            input_zero=-128,
            weight_scale=0.01,
        )

    def test_input_quantization_matches_tflite_convention(self):
        m = self._model()
        q = m.quantize_input([1.0, 0.0])
        self.assertEqual(q[0], 127 - 128 + 128 if False else round(1.0 / m.input_scale) + m.input_zero)
        # 1.0 -> 127 + (-128) = -1 ; 0.0 -> -128
        self.assertEqual(m.quantize_input([1.0])[0], round(1.0 * 127) - 128)
        self.assertEqual(m.quantize_input([0.0])[0], -128)

    def test_infer_classifies_by_the_learned_boundary(self):
        m = self._model()
        self.assertEqual(m.infer([1.0, 0.0]), 0)  # x0 > x1 -> class 0
        self.assertEqual(m.infer([0.0, 1.0]), 1)  # x1 > x0 -> class 1

    def test_manifest_is_device_shaped(self):
        f = self._model().to_manifest()
        self.assertEqual(f["source"], "tflite")
        self.assertEqual(f["input_len"], 2)
        self.assertEqual(f["output_len"], 2)
        self.assertEqual(f["weights_len"], 4)
        self.assertIn("weight_scale_milli", f)
        self.assertIn("input_zero", f)

    def test_infer_is_exact_argmax_under_shared_scale(self):
        # since bias=0 and all outputs share the weight/input scale, the int8 argmax
        # equals the float argmax - the property that makes the import lossless for
        # classification.
        m = self._model()
        for x in ([0.9, 0.1], [0.2, 0.8], [0.5, 0.49]):
            xq = m.quantize_input(x)
            accs = []
            for j in range(2):
                row = m.weights[j * 2:(j + 1) * 2]
                accs.append(sum(w * (xv - m.input_zero) for w, xv in zip(row, xq)))
            self.assertEqual(m.infer(x), accs.index(max(accs)))


if __name__ == "__main__":
    unittest.main()
