# 02 - Build with Blocks

Design the scheduling graph without learning language syntax first.

## 1. Open the editor

Open [`packages/block-editor/index.html`](../../packages/block-editor/index.html).
It runs locally in the browser and does not upload the graph.

## 2. Declare tasks and wires

Add `periodic`, `control`, or `service` tasks, set their periods, then connect
them with bounded wires. Export `app.json`.

## 3. Validate the exact firmware input

```bash
python sdk/cli/nobro.py app path/to/app.json
```

The validator uses the same strict task/wire document as Python and native
firmware generation. Try the checked-in example:

```bash
python sdk/cli/nobro.py app tutorials/hello-device/app.json
```

Then change a wire endpoint to `missing`; validation must fail with that task
name instead of silently dropping the edge.

## 4. Build native firmware

```bash
python sdk/cli/nobro.py firmware path/to/app.json --build
```

The graph covers scheduling and topology. Physical sensors, actuators, model
artifacts, and payload transport are selected in the provider layer rather than
being guessed from task names.

Next: [03 - Arduino & Python](../03-arduino-and-python/README.md).
