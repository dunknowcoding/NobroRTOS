# Python SDK

The Python surface ships as the `nobro-rtos-tools` package (source of truth:
[`bindings/python/`](../../bindings/python/)).

```bash
pip install ./bindings/python            # parsing, contracts, simulators (stdlib-only)
pip install "./bindings/python[serial]"  # + live serial monitoring (NobroNode)
```

After install you have the `nobro-rtos` console command (contract inspection,
report decoding, simulators — `nobro-rtos --help`) and the library:

```python
from nobro_rtos.node import NobroNode, parse_status_line   # live boards + decoding
from nobro_rtos.replay import decode_trace, to_audit       # capability replay audits
from nobro_rtos.nn_export import train_dense, export_model # host-side model training
```

Tutorial: [tier 03](../../tutorials/03-arduino-and-python/README.md).
