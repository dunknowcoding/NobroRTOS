# NobroRTOS Book

This book is the fastest path from first clone to a small, reviewable
NobroRTOS application. It keeps the same contract-first model used by the
kernel: describe the board, declare modules, validate budgets, and only then
bind hardware-specific adapters.

## Chapters

1. [Contracts First](01-contracts-first.md)
2. [Local Validation](02-local-validation.md)
3. [Build A Device App](03-build-a-device-app.md)
4. [AI, Robot, And IoT Modules](04-ai-robot-iot.md)
5. [Diagnostics And Recovery](05-diagnostics-recovery.md)

## Tutorial Gate

Run the tutorial checker from the repository root:

```powershell
python tools/tutorial_runner.py
```

The checker validates the public tutorial app, confirms the generated Rust
skeleton path still works, and runs the bounded timing/lease verifier.
