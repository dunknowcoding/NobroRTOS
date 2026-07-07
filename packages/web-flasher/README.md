# NobroRTOS Web Flasher

Static browser tool for no-install firmware handoff.

Capabilities:

- drop a `.uf2` or `.bin` image and inspect size/checksum locally
- enter compatible bootloaders through Web Serial with a 1200-baud touch
- send a runtime boot command such as `DFU`
- pair with compatible WebUSB devices and attempt a bulk OUT transfer
- guide UF2 mass-storage workflows without installing a native app

Run:

```powershell
python -m http.server 8000 -d packages\web-flasher
```

Then open `http://localhost:8000`.

Browser support depends on the Web Serial and WebUSB APIs exposed by the user's
browser. The tool performs feature detection and keeps all firmware bytes local
to the browser session.
