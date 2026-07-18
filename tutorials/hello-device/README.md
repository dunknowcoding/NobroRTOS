# hello-device - one task/wire app

`app.json` is the same strict graph accepted by the validator, Python tooling,
block editor, and native firmware generator:

```bash
python sdk/cli/nobro.py app tutorials/hello-device/app.json
python sdk/cli/nobro.py firmware tutorials/hello-device/app.json --build
```

Tasks use `periodic`, `control`, or `service`; a wire records bounded topology.
Hardware providers and payload transport bind separately, so this source file
does not claim a sensor driver merely because one task is named `imu`.
