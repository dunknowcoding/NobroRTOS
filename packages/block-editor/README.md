# NobroRTOS Block Editor

Dependency-free visual editor for the canonical task/wire `app.json` contract.
Its output is accepted unchanged by both validation and native firmware
generation:

```powershell
python -m http.server 8000 -d packages\block-editor
python sdk/cli/nobro.py app app.json
python sdk/cli/nobro.py firmware app.json --build
```

The editor keeps app data in the browser until download. Tasks use the same
`periodic`, `control`, and `service` roles and defaults as the other NobroRTOS
authoring surfaces. A wire describes bounded topology; payload transport and
physical sensor/actuator/provider binding are separate firmware concerns.

`models.json` remains a public catalog of available model artifacts for the
later provider-binding layer. It is not mixed into the scheduling graph or
silently converted into a task.
