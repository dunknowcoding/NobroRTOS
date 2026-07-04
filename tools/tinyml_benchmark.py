#!/usr/bin/env python3
"""TinyML benchmark: NobroRTOS int8 vs TFLite vs float, on a fixed public set (M147).

Trains one dense classifier on scikit-learn's 8x8 digits set (public, deterministic),
then compares three deployment paths on the SAME held-out data:
  - float32 Keras (the accuracy ceiling),
  - TFLite full-int8 (the industry-standard MCU baseline),
  - NobroRTOS int8 (our own dense_int8 kernel, model imported via tflite_adapter).
Reports accuracy, model size, and how often our kernel agrees with TFLite - the numbers
that show our own int8 path is competitive with the vendor baseline, not just plausible.

Requires tensorflow + scikit-learn (a bench tool, run by hand):
  python3 tools/tinyml_benchmark.py
"""
import os
import sys
from pathlib import Path

os.environ.setdefault("TF_CPP_MIN_LOG_LEVEL", "3")
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "bindings" / "python"))


def main() -> int:
    import numpy as np
    import tensorflow as tf
    from sklearn.datasets import load_digits
    from sklearn.model_selection import train_test_split

    from nobro_rtos.tflite_adapter import load_tflite_dense

    work = Path("_work")
    work.mkdir(exist_ok=True)
    tflite_path = work / "bench_digits_int8.tflite"

    d = load_digits()
    X = (d.data / 16.0).astype(np.float32)
    y = d.target.astype(np.int32)
    Xtr, Xte, ytr, yte = train_test_split(X, y, test_size=0.25, random_state=7)

    model = tf.keras.Sequential([tf.keras.layers.Input((64,)), tf.keras.layers.Dense(10)])
    model.compile(optimizer="adam",
                  loss=tf.keras.losses.SparseCategoricalCrossentropy(from_logits=True),
                  metrics=["accuracy"])
    model.fit(Xtr, ytr, epochs=30, verbose=0)

    # float accuracy
    float_acc = float((np.argmax(model.predict(Xte, verbose=0), axis=1) == yte).mean())

    # convert to full-int8 tflite
    def rep():
        for i in range(200):
            yield [Xtr[i:i + 1]]

    conv = tf.lite.TFLiteConverter.from_keras_model(model)
    conv.optimizations = [tf.lite.Optimize.DEFAULT]
    conv.representative_dataset = rep
    conv.target_spec.supported_ops = [tf.lite.OpsSet.TFLITE_BUILTINS_INT8]
    conv.inference_input_type = tf.int8
    conv.inference_output_type = tf.int8
    tfl = conv.convert()
    tflite_path.write_bytes(tfl)

    interp = tf.lite.Interpreter(model_path=str(tflite_path))
    interp.allocate_tensors()
    inp = interp.get_input_details()[0]
    out = interp.get_output_details()[0]
    isc, izp = inp["quantization"]

    # our int8 model via the adapter (weights + bias, one int8 blob)
    ours = load_tflite_dense(str(tflite_path))
    ours_bytes = len(ours.weights) + 4 * len(ours.bias)  # int8 weights + int32 bias

    tfl_hits = ours_hits = agree = 0
    n = len(Xte)
    for i in range(n):
        x = Xte[i]
        xq = np.round(x / isc + izp).clip(-128, 127).astype(np.int8).reshape(1, 64)
        interp.set_tensor(inp["index"], xq)
        interp.invoke()
        tfl_pred = int(np.argmax(interp.get_tensor(out["index"])[0]))
        our_pred = ours.infer(x.tolist())
        tfl_hits += tfl_pred == yte[i]
        ours_hits += our_pred == yte[i]
        agree += our_pred == tfl_pred

    print(f"dataset: sklearn digits 8x8, {n} held-out samples, dense 64->10")
    print(f"{'path':22} {'accuracy':>10} {'model size':>12}")
    print(f"{'float32 (Keras)':22} {float_acc*100:9.1f}% {'n/a':>12}")
    print(f"{'TFLite int8':22} {tfl_hits/n*100:9.1f}% {len(tfl):11d}B")
    print(f"{'NobroRTOS int8':22} {ours_hits/n*100:9.1f}% {ours_bytes:11d}B")
    print(f"NobroRTOS vs TFLite agreement: {agree}/{n} ({agree/n*100:.1f}%)")
    ok = agree >= 0.97 * n and ours_hits >= 0.85 * n
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
