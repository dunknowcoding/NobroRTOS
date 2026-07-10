# tools/bench — internal lab sketches

Arduino sketches used by the maintainers' hardware-evaluation rigs (driven by tools
like `m220_rfid_eval.py` and the telemetry collectors). They are **not** part of the
NobroRTOS product surface:

- They target whatever modules happen to be wired to the lab bench.
- Credentials/hosts are placeholders (`<YOUR_SSID>` etc.) — copy a sketch and fill in
  your own values locally; never commit real ones.
- Build outputs (`*/build/`) are gitignored.

If you want supported examples, start at `tutorials/`, `packages/arduino/examples/`,
and `docs/GETTING_STARTED.md` instead.
