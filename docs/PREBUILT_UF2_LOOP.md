# Prebuilt UF2 loop (CircuitPython-class UX)

**UX rung 0 target:** flash a prebuilt UF2 **once**, then iterate by editing *data*
(`app.json` from the block editor) — no toolchain, no rebuild, code-free after first
flash.

## The loop

```
1. Drag-drop prebuilt UF2  →  board enumerates (DFU/COM)
2. Open block editor       →  design app visually
3. Export app.json         →  drop on serial / future UF2 data partition
4. Web console / ReportReader  →  plain-English PASS/FAIL from NOBRO_* reports
```

NobroRTOS already has every piece except the **prebuilt UF2 bundle** and the
**app.json hot-swap transport**:

| Piece | Status |
| --- | --- |
| Block editor → `app.json` | Done (Wave 2 ML block) |
| Web-flasher report console | Done (Wave 1) |
| `nobro_app.py` validator | Done |
| Bootloader-safe UF2 flash | Done (`hw_eval --flash uf2`) |
| Prebuilt "shell" UF2 | **Wave 8** |
| app.json runtime reload | **Wave 8+** (manifest hot-reload or serial drop) |

## Prebuilt shell firmware

The shell UF2 is a known-good firmware image that:

1. Boots through the six-stage chain and emits decodable `NOBRO_*` reports.
2. Exposes a **data slot** for `app.json` (KV store, flash log, or serial ingest).
3. Re-admits modules when `app.json` changes (no reflash).

Build command (once Wave 8 lands):

```bash
python tools/package_prebuilt_uf2.py --profile s140 --out packages/prebuilt/
```

## What the user sees

After the one-time UF2 flash:

- Block editor exports `app.json`.
- User drops the file (serial upload or future mass-storage slot).
- Console shows: "✅ servo mounted, sensor alive" or the first-fault sentence.

This matches the CircuitPython "edit `code.py`, save, it runs" bar — except the
editable artifact is **contract data**, not Python source.

## Gate (planned)

`check_prebuilt_loop.py` will verify:

- `packages/prebuilt/` contains a manifest (profile, hash, version).
- Sample `app.json` from the block editor passes `nobro_app.validate()`.
- Web-flasher parser recognizes the shell's report lines.

See Wave 8 in `REMODELING_PLAN_INTERNAL.md`.
