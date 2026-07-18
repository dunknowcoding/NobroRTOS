# Python SDK

The Python surface ships as the `nobro-rtos` distribution (source of truth:
[`bindings/python/`](../../bindings/python/)).

```bash
pip install nobro_rtos           # dependency-free core: contracts, reports, simulators
pip install "nobro_rtos[serial]" # + live serial monitoring (pyserial)
pip install "nobro_rtos[tflite]" # + large TensorFlow .tflite importer
```

Python package names normalize `_` and `-`, so `pip install nobro_rtos`
resolves the `nobro-rtos` distribution and imports as `nobro_rtos`.
After install you have the `nobro-rtos` console command (contract inspection,
report decoding, simulators - `nobro-rtos --help`) and the library:

```python
from nobro_rtos.node import NobroNode, parse_status_line   # live boards + decoding
from nobro_rtos.replay import decode_trace, to_audit       # capability replay audits
from nobro_rtos.nn_export import train_dense, export_model # host-side model training
```

Tutorial: [tier 03](../../tutorials/03-arduino-and-python/README.md).
