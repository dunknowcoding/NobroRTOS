# 03 — Arduino & Python 🐍

*Your first real programming language contact — with high-level APIs that stay small.*

Two independent paths; do either or both. In each, you'll work with NobroRTOS's
core idea — self-verifying reports — using beginner-grade APIs.

---

## Path A: Arduino IDE (no Rust anywhere)

### What you need

| Thing | Where |
| --- | --- |
| Arduino IDE 2.x | [arduino.cc/en/software](https://www.arduino.cc/en/software) |
| Any Arduino-capable board (Uno, Nano, UNO R4, ESP32…) | whatever you have |
| The NobroRTOS Arduino library | `sdk/` build or `_work/NobroRTOS-arduino.zip` via `python tools/package_arduino.py --zip` |

### Steps

1. Arduino IDE → **Sketch → Include Library → Add .ZIP Library…** → pick the zip.
2. **File → Examples → NobroRTOS → ReportReader** — open it and read: it builds a
   report exactly like device firmware, seals its checksum, then decodes it.
3. Upload, open the Serial Monitor at **115200**:

```
magic      = 0x4E425254  (runtime report)
all_pass   = 1
checksum   = 0x4E425258  (valid)
VERDICT    : PASS
```

4. Now break it: flip one number in `demo_report[]` and re-upload. The checksum
   check catches the corruption — **that's the whole philosophy in one sketch.**

## Path B: Python

### What you need

| Thing | Where |
| --- | --- |
| Python 3.10+ | [python.org](https://www.python.org/downloads/) |
| The `nobro-rtos` distribution | `pip install nobro_rtos` (add `[serial]` for live boards or `[tflite]` for the large TensorFlow importer) |

### Steps

The beginner API is three lines:

```python
from nobro_rtos.node import parse_status_line

report = parse_status_line("NOBRO-C3 arch=riscv32imc subsystems=7 all_pass=1")
print(report.name, report.fields)          # C3 {'arch': 'riscv32imc', ...}
```

Have a live NobroRTOS board on a serial port? Watch it talk:

```python
from nobro_rtos.node import NobroNode

with NobroNode("YOUR_PORT_HERE") as node:
    for report in node.reports(seconds=5):
        print("PASS" if report.fields.get("all_pass") == 1 else "FAIL", report.name)
```

No board? The package ships full simulators — run `nobro-rtos --help` and try
`nobro-rtos sample-report`.

## ✔ Verify

- [ ] Arduino: `VERDICT: PASS` on the Serial Monitor, and your sabotage got caught
- [ ] Python: `parse_status_line` returns a decoded report; `nobro-rtos --help` lists commands

Next: [04 — Your First Module](../04-your-first-module/README.md) →
