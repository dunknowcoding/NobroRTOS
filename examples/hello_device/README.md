# hello_device — a NobroRTOS app with no Rust required

`app.json` declares a board, a servo (from the built-in catalog), and a sensor. No code:

```
python3 tools/nobro_app.py examples/hello_device/app.json          # validate + plan
python3 tools/nobro_app.py examples/hello_device/app.json --gen main.rs   # generate Rust
```

Change `"brand": "sg90"` to `"mg996r"`, or `"board"` to `rp2350`, and re-run — the tool
validates against the device + board catalogs and regenerates. This is the beginner path
(config, not code) and the multi-language front-end: any tool that emits this JSON drives
NobroRTOS.
