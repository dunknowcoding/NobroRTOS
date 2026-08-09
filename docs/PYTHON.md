# Python generator guide

The `nobro-rtos-core` Python package turns a small JSON workload into the C and
assembly boundary used by compact byte-addressed targets. It also parses
supported linker reports so a firmware build can fail when its program or data
budget is exceeded.

## Install

```text
python -m pip install https://github.com/dunknowcoding/NobroRTOS/releases/download/v1.0.0/nobro_rtos_core-1.0.0-py3-none-any.whl
```

For a checked-out release, the equivalent command is
`python tools/nobro_core.py`.

## Generate an application

Start from `ports/byte/examples/useful/app.json`:

```json
{
  "schema": "nobro-rtos-core-byte-v1",
  "tick_unit_us": 1000,
  "program_capacity_bytes": 2048,
  "application_reserve_bytes": 1024,
  "mailbox_capacity": 2,
  "idle_hook": true,
  "watchdog_hook": true,
  "tasks": [
    {
      "name": "sensor",
      "period_ticks": 10,
      "dispatch_deadline_ticks": 3,
      "budget_ticks": 1
    }
  ]
}
```

Generate the fixed configuration and receipt:

```text
nobro-core app.json --out generated
```

Unknown fields, invalid identifiers, impossible utilization/response bounds,
ambiguous wrap intervals, and capacity violations fail closed. Output includes
the normalized contract digest, fixed ABI/configuration, task bridge, assembly
symbols, and a machine-readable receipt.

## Python API

```python
from pathlib import Path
from nobro_core import generate

receipt = generate(Path("app.json"), Path("generated"))
print(receipt["kernel_data_bytes"])
```

## Gate a linked image

```text
nobro-core --map firmware.mem --format sdcc-mem \
  --program-limit 1024 --data-limit 32
```

Supported report formats include SDCC MCS-51/STM8, IAR 8051, Keil C51, GNU
size, and MPLAB XC8/XC16 forms. Format selection is explicit; the tool does not
guess a compiler from local filenames.
