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

The model catalog lives in `models.json`, which the training pipeline writes:

```powershell
python tools\train_motion_nn.py   # trains the int8 MLP, updates models.json
```

The editor loads `models.json` at startup (when served over HTTP) and falls back to a
built-in seed otherwise, so it always offers at least the motion classifier.
