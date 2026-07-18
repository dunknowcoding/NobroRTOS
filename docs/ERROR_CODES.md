# Error codes

This index is generated from `sdk/error-codes.json`. The code and first
sentence are stable across bindings; details may add the failing task, field,
observed value, or limit. Fix the first reported error, then run the command again.

| Code | Surface | Meaning | First recovery step |
|---|---|---|---|
| `NOBRO-E001` | admission | The workload must contain at least one task. | Add one bounded task before admission. |
| `NOBRO-E002` | admission | The workload exceeds the task capacity. | Remove tasks or select a deliberately larger task capacity. |
| `NOBRO-E003` | admission | Each task identity must be unique. | Give every admitted task a distinct identity. |
| `NOBRO-E004` | admission | A task deadline or period is invalid. | Use a positive period and a deadline no greater than that period. |
| `NOBRO-E005` | admission | Task jitter must be below its deadline. | Reduce the jitter bound or increase the admitted deadline. |
| `NOBRO-E006` | admission | A task execution bound is missing or too large. | Provide a positive execution bound that fits the deadline. |
| `NOBRO-E007` | admission | Task execution plus blocking exceeds its deadline. | Reduce execution or blocking, or increase the admitted deadline. |
| `NOBRO-E008` | admission | The workload utilization exceeds one core. | Reduce execution budgets or task rates. |
| `NOBRO-E009` | admission | Response-time analysis found a missed deadline. | Reduce interference, execution, or blocking for the named task. |
| `NOBRO-E010` | admission | The workload exceeds its flash profile. | Disable optional features or select a measured larger flash profile. |
| `NOBRO-E011` | admission | The workload exceeds its RAM profile. | Reduce static allocations or select a measured larger RAM profile. |
| `NOBRO-E012` | admission | The workload exceeds its sample-pool profile. | Reduce retained samples or select a larger bounded pool. |
| `NOBRO-E013` | admission | Admission arithmetic overflowed. | Reduce the declared bounds and check their units. |
| `NOBRO-E014` | admission | The wake-latency bound exceeds a task deadline. | Reduce the wake bound or increase the affected deadline. |
| `NOBRO-E015` | admission | A task phase must be below its period. | Choose a phase from zero through period minus one. |
| `NOBRO-E016` | admission | An interrupt priority is outside the target range. | Select a priority implemented by the target. |
| `NOBRO-E017` | admission | An interrupt priority is reserved by the platform stack. | Use an application priority not reserved by the selected stack. |
| `NOBRO-E018` | admission | An interrupt timing or stack contract is invalid. | Provide positive bounded timing and stack values. |
| `NOBRO-E019` | admission | An interrupt step requests an unbounded operation. | Move the operation out of interrupt context or use a bounded variant. |
| `NOBRO-E020` | admission | The nested interrupt-stack budget is exceeded. | Reduce nesting or stack use, or select a measured larger bound. |
| `NOBRO-E021` | admission | Interrupt interference exceeds a deadline. | Reduce higher-priority interrupt work or relax the affected deadline. |
| `NOBRO-E030` | project | Workload must contain a non-empty task list. | Add the kernel task and at least one application task. |
| `NOBRO-E031` | project | Every task needs one stable lowercase name. | Use a name matching [a-z][a-z0-9_-]{0,47}. |
| `NOBRO-E032` | project | Task names must be unique. | Rename the duplicate task. |
| `NOBRO-E033` | project | Every workload needs the kernel task. | Add the kernel task to the workload. |
| `NOBRO-E034` | project | Task criticality must be one of the known roles. | Choose a role accepted by the workload schema. |
| `NOBRO-E035` | project | Resource and timing numbers must be non-negative. | Replace negative values with measured non-negative bounds. |
| `NOBRO-E036` | project | A task budget must fit inside its period. | Reduce the budget or increase the period. |
| `NOBRO-E037` | project | Task dependencies must be a short unique list. | Remove duplicate or excessive dependency names. |
| `NOBRO-E038` | project | Task dependencies must name existing tasks. | Correct or add the named dependency task. |
| `NOBRO-E039` | project | The kernel starts first and cannot depend on app tasks. | Remove dependencies from the kernel task. |
| `NOBRO-E040` | project | Each wire must be written as [from, to]. | Provide exactly one source and one destination. |
| `NOBRO-E041` | project | Wire endpoints must name existing tasks. | Correct or add each endpoint task. |
| `NOBRO-E042` | project | Startup dependencies cannot form a cycle. | Remove one dependency edge from the cycle. |
| `NOBRO-E043` | project | Features must be one object of boolean switches. | Use one features object with true or false values. |
| `NOBRO-E044` | project | Feature target is unsupported. | Choose a target present in the feature catalog. |
| `NOBRO-E045` | project | Feature name is unknown for this target. | Choose a feature listed for the selected target. |
| `NOBRO-E046` | project | Feature values must match the catalog. | Use the value type declared by the feature catalog. |
| `NOBRO-E047` | project | Feature is unavailable for this target. | Disable it or choose a target with linked evidence. |
| `NOBRO-E048` | project | Enabled features conflict. | Disable one of the named conflicting features. |
| `NOBRO-E049` | project | Workload schema version is unsupported. | Migrate the document to the current workload schema. |
| `NOBRO-E050` | app | Application graph state does not allow this operation. | Declare tasks and wires before running the graph. |
| `NOBRO-E051` | app | Names must use stable lowercase labels. | Use a name matching [a-z][a-z0-9_-]{0,47}. |
| `NOBRO-E052` | app | Task rate and period must be valid. | Use a positive period no longer than the wrap-safe interval. |
| `NOBRO-E053` | app | Application task capacity is exceeded. | Remove tasks or deliberately select a larger bounded capacity. |
| `NOBRO-E054` | app | Application wire capacity is exceeded. | Use a capacity from 1 through 64 or declare fewer wires. |
| `NOBRO-E055` | app | Wire endpoints must name existing tasks. | Declare both endpoint tasks before the wire. |
| `NOBRO-E056` | app | Task name is already declared. | Rename or remove the duplicate task. |
| `NOBRO-E057` | app | Task timing or resource options are invalid. | Keep phase, deadline, budget, blocking, and memory within their bounds. |
| `NOBRO-E058` | app | Application graph admission failed. | Inspect the retained admission code and reduce the failing bound. |
| `NOBRO-E059` | app | Task callback failed. | Handle the callback error before polling again. |
| `NOBRO-E060` | app | Wire is already declared. | Remove the duplicate source-to-destination wire. |
| `NOBRO-E061` | app | A task cannot wire to itself. | Choose two distinct endpoint tasks. |
| `NOBRO-E062` | app | Task role is unsupported. | Choose periodic, control, or service. |
| `NOBRO-E063` | app | Board or app schema is unsupported. | Choose a supported board and the current app schema. |
| `NOBRO-E064` | app | Application graph needs at least one task. | Declare one bounded task before validation. |
| `NOBRO-E065` | app | Application declaration shape or field type is invalid. | Correct the named field and remove unknown fields. |
