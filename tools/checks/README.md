# Checks

Reproducible contributor and hosted-CI gates. Use `run_checks.py` as the
portable local entry point; `ci_matrix.sh` adds the cross-MCU build matrix.

| Directory | Scope |
| --- | --- |
| [`core/`](core/) | Kernel, timing, layout, dependency, and nano-runtime invariants |
| [`platforms/`](platforms/) | Board profiles, capability tiers, firmware generation, and portability |
| [`integrations/`](integrations/) | Arduino, adapters, peripherals, wireless, audio, camera, and ROS |
| [`product/`](product/) | App authoring, editor, web flasher, and UDI surfaces |
| [`release/`](release/) | Public boundary, documentation, packages, versions, and CI integrity |

The category directories are implementation details of the gate runner. Normal
local validation remains:

```console
python tools/checks/run_checks.py
```

Hardware campaigns, competitor comparisons, raw evidence, and local device
identities are private maintainer inputs and are not part of this directory.
