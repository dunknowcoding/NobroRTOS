# NiusDisplay adapter

This allocation-free adapter mounts NiusDisplay-compatible transports behind
the bounded `nobro-display` contract. It validates geometry, exact payload size,
deadline, lifecycle, recovery, and receipt versioning. Controller and module
inventories stay data-only in NiusDisplay.

The portable adapter is host/target tested. No physical panel claim follows;
that requires an exact board, bus, module, panel run, and restoration receipt.
