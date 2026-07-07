# NobroRTOS Block Editor

Static visual editor that emits `app.json` for `tools/nobro_app.py`.

Run:

```powershell
python -m http.server 8000 -d packages\block-editor
```

Then open `http://localhost:8000`.

The editor is dependency-free and keeps all generated app data inside the
browser session until the user downloads `app.json`.
