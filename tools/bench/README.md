# Bench evaluation sketches (internal)

Arduino sketches under `tools/bench/` support **automated hardware evaluation**
on specific bench setups. They are not part of the public NobroRTOS authoring
surface.

| Sketch | Purpose |
| --- | --- |
| `Es8311AudioLoopback` | ES8311 audio loopback on ESP32-S3 UNO |
| `Es8311SoundEvents` | Sound event detection bench |
| `Es8311VoiceHealth` | Voice health telemetry |
| `Esp32WifiTelemetry` | WiFi telemetry smoke |
| `NanoMpu6050Telemetry` | Nano + MPU6050 I2C telemetry |
| `UnoR4RfidRc522Verify` | UNO R4 + RC522 RFID verify (`m220_rfid_eval.py`) |
| `UnoR4WifiTelemetry` | UNO R4 WiFi telemetry |
| `Vision8Analytics` | Multi-camera analytics bench |

**Rules:**

- Sketches may reference bench-specific wiring; never copy COM ports or env names
  into public docs or committed gate output.
- Build outputs (`tools/bench/*/build/`) are gitignored.
- Eval drivers live alongside the sketches (`tools/m220_rfid_eval.py`, etc.).

For the public hardware eval path, use `tools/nobro_hw_eval.py` and
`docs/HW_QUICKSTART.md`.
