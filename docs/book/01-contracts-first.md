# Contracts First

NobroRTOS applications start with explicit contracts instead of hidden global
state. A contract answers four questions before runtime work begins:

- Which board profile and boot layout are selected?
- Which modules exist, and what capabilities do they require or own?
- How much flash, RAM, sample-pool space, and timing budget does each module use?
- What reports should a host tool read when something fails?

The kernel admission path turns those answers into a startup order, quota
ledger, and capability grant table. This makes failures early and explainable:
unknown modules, duplicate capabilities, missing deadlines, and budget overflow
are rejected before driver code runs.

Use this mental model for every app:

```text
board data -> module manifest -> startup graph -> admission -> runtime
```

When porting an existing driver, keep the driver thin and put policy in the
contract. The driver should expose bounded operations through SAL or HAL traits;
the app should own behavior, deadlines, and recovery decisions.
