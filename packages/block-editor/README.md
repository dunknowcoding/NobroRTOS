# NobroRTOS Block Editor

Static visual editor that emits `app.json` for `tools/nobro_app.py`.

Run:

```powershell
python -m http.server 8000 -d packages\block-editor
```

Then open `http://localhost:8000`.

The editor is dependency-free and keeps all generated app data inside the
browser session until the user downloads `app.json`.

## ML blocks

The palette includes trained machine-learning inference blocks (e.g. **Motion NN**).
Adding one contributes an `ai_models[]` entry to `app.json` whose fields match the host
`AiModelContract` (`model_id`, `backend`, `input_bytes_max`, `output_bytes_max`,
`arena_bytes`, `timeout_us`, `stale_after_us`), so the generated app validates with
`tools/nobro_app.py` and drops straight into a contract bundle.

The checked-in model catalog lives in `models.json`. Each card names the public,
checked-in model artifact that implements its contract. To add or update a card,
update that artifact and the catalog together, then validate them with:

```powershell
python tools/check_block_editor.py
```

The editor loads `models.json` at startup (when served over HTTP) and falls back to a
built-in seed otherwise, so it always offers at least the motion classifier.
