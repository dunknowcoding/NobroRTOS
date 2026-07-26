# CLI tools

These are public SDK command implementations. The stable user surface is:

```console
python sdk/cli/nobro.py <command>
```

| Directory | Purpose | Dispatcher commands |
| --- | --- | --- |
| [`project/`](project/) | App validation, project generation, adapters, and native firmware authoring | `app`, `project`, `adapter`, `firmware` |
| [`firmware/`](firmware/) | Image deployment and signing | `flash`, `sign` |
| [`analysis/`](analysis/) | Admission, capacity, static budgets, diagnostics, and timing/lease verification | `admit`, `shrink`, `budget`, `verify-timing` |
| [`interop/`](interop/) | DeviceTree and bounded ROS import | `import-dts`, `ros-msg` |
| [`learning/`](learning/) | Contract inspection and tutorial validation | `contract`, `tutorials` |

Implementation paths may move within these categories. Scripts, documentation,
CI, and packaged SDKs should invoke the dispatcher so users have one predictable
command surface.
