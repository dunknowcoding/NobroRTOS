# Equivalent authoring fixture

These sources measure only the declarations needed to create three periodic tasks at
5/10/40 ms. Device behavior, channels, drivers, build files, generated Rust, and runtime
resource use are excluded. This narrow scope prevents the line count from being
presented as overall ease-of-use or efficiency.

The Embassy fixture targets the executor/time API shape and the FreeRTOS fixture targets
the task API shape. They are comparison text, not resource binaries; the pinned buildable
resource variants live one directory above. Run `python tools/measure_authoring.py` to
recompute semantic lines and the explicitly declared concept inventory.
