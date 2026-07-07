# Diagnostics And Recovery

NobroRTOS exports fixed `NOBRO_*` reports so host tools can inspect system
state without guessing. The important habit is to preserve the first fault:

```text
board profile -> board package -> manifest -> adapter compatibility -> admission -> runtime
```

Runtime recovery is module-scoped. A failed module can be quiesced, cleaned,
retried, restarted, or disabled without resetting unrelated work. Hot reload and
degraded-mode planning follow the same rule: clear stale mailbox records,
alarms, watchdog entries, quota usage, and owner-scoped leases before the module
returns to active state.

For software review, use:

```powershell
python tools/nobro_contract_tool.py check-recovery-matrix
python tools/nobro_contract_tool.py check-runtime-drill
python tools/verify_timing_lease.py
```
