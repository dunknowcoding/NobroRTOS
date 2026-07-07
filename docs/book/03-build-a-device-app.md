# Build A Device App

The declarative app path is the simplest way to learn the system. It starts
with `app.json` and produces a Rust skeleton that mounts the selected device
profiles.

Run the public tutorial app:

```powershell
python tools/nobro_app.py tutorials\hello-device\app.json
```

Generate a skeleton into `_work/`:

```powershell
python tools/nobro_app.py tutorials\hello-device\app.json --gen _work\hello_device.rs
```

The app schema is intentionally small:

- `board`: selects a known board profile.
- `actuators`: names actuator profiles and PWM channels.
- `sensors`: names sensor profiles, buses, and addresses.
- `behaviors`: records the human-readable intent of the example.

The generated skeleton is a starting point, not a hidden framework. Keep the
contract readable, then attach real adapters through SAL/HAL surfaces.
