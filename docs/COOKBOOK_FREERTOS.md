# Cookbook: coming from FreeRTOS

You think in tasks, priorities, queues, semaphores, and software timers. This cookbook maps
those primitives onto NobroRTOS and shows where the mental model changes. It complements the
high-level [PORTING_FROM.md](PORTING_FROM.md).

The one idea to internalize: **FreeRTOS schedules priorities you assign; NobroRTOS schedules
a criticality + deadline contract it can check.** Instead of tuning priorities until timing
"looks right," you *declare* each module's period, jitter budget, memory, and capabilities,
and admission rejects a system that cannot meet them.

## 1. `xTaskCreate` -> a module

A FreeRTOS task is a function with its own stack and a fixed priority. A NobroRTOS module is
a struct with `init()` + `poll(now_us)` and a declared contract; the kernel owns the stack
budget and the schedule.

```c
/* FreeRTOS */
void imu_task(void *arg) {
    for (;;) {
        imu_sample_t s = imu_read();
        xQueueSend(telemetry_q, &s, 0);
        vTaskDelay(pdMS_TO_TICKS(20));
    }
}
xTaskCreate(imu_task, "imu", 512, NULL, 3 /*prio*/, NULL);
```

```rust
// NobroRTOS: the delay is a period; the priority is a criticality; the queue is a mailbox.
ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
    .requires(CapabilitySet::empty().with(Capability::Bus0).with(Capability::SamplePool))
    .memory(MemoryBudget::new(8 * 1024, 1024, 1)) // flash, ram, pool slots - checked
    .deadline(DeadlineContract::new(20_000, 10));  // 20 ms period, 10 us jitter budget
```

## 2. Primitive mapping

| FreeRTOS | NobroRTOS | Note |
| --- | --- | --- |
| `xTaskCreate` | `ModuleSpec` + module `poll()` | stack sizing becomes a declared RAM budget |
| task priority (number) | `Criticality` (BestEffort < User < Driver < System < HardRealtime) | 5 ordered classes, not 32 raw levels |
| `vTaskDelay` / `vTaskDelayUntil` | `DeadlineContract` period, or an `Alarm` you check in `poll` | cadence is declared, not slept |
| `xQueueSend/Receive` | `Mailbox` (bounded) | fixed depth + fixed message size |
| `xSemaphoreTake` (mutex) | a **capability** owned by one module | ownership is exclusive by contract, not by lock discipline |
| counting semaphore | quota / pool slots | admission-checked capacity |
| `xTimerCreate` (software timer) | `Alarm` in the `AlarmQueue` | one-shot / periodic deadline events |
| task watchdog | `Watchdog` + `HealthMonitor` counters | misses drive module-scoped recovery |
| priority inheritance | not needed | shared resources are owned, not locked across tasks |
| heap (`pvPortMalloc`) | static pools, fixed capacity | no hot-path allocation |

## 3. Priorities become criticality + deadline

FreeRTOS gives you a raw priority number and leaves correctness to you. NobroRTOS asks two
questions instead:

- **How critical is it?** `Criticality::HardRealtime` modules *must* carry a
  `DeadlineContract` (period + jitter budget) or admission rejects them. Lower classes
  (`System`, `Driver`, `User`, `BestEffort`) degrade first under pressure - see
  `DegradePlanner`, which drops best-effort work before critical work.
- **Can the system meet it?** Admission sums every module's memory and checks the deadline
  set is schedulable *before* the system runs. A build that can't meet its contract fails
  loudly, not intermittently in the field.

So the porting move is: for each task, pick a criticality class, and for anything real-time,
write down the period and jitter you were implicitly assuming with `vTaskDelayUntil`.

## 4. Queues and semaphores

- **Queue -> `Mailbox`.** Declare depth and message size; sends past capacity fail (or drop
  by policy) instead of blocking unboundedly.
- **Mutex -> capability ownership.** Rather than a runtime lock every task must remember to
  take, a resource (a bus, the sample pool, a radio) is *owned* by exactly one module in the
  manifest. Two modules that both claim `Capability::Bus0` fail admission - the conflict is
  caught at build time, not as a priority-inversion bug at 3am.
- **Counting semaphore -> quota / pool slots.** Capacity is declared and enforced by the
  `QuotaLedger` / `SamplePool`.

## 5. If you have a large legacy C task loop

`nobro_classic` (`core/crates/nobro_classic`) is the compatibility surface for a
`setup()/loop()`-style body. Port the task *logic* into a module's `poll()`, reach hardware
through a SAL adapter (reusing your existing driver code where possible), and let the kernel
provide the schedule and the watchdog. On-MCU authoring in C/C++ rides the C ABI
(`bindings/c/include/nobro_app.h`): your module implements `nobro_app_init` / `nobro_app_poll`
and calls back into host-provided services.

## 6. Recipe summary

1. For each `xTaskCreate`, write a **`ModuleSpec`**: criticality, memory, capabilities, and
   a **deadline** for anything real-time.
2. Turn `vTaskDelay` cadence into a **declared period** or an **alarm**.
3. Turn queues into **bounded mailboxes**; turn mutexes into **capability ownership**.
4. Replace heap with **static pools**.
5. Run admission - it schedules and budget-checks the whole system up front - then read the
   `NOBRO_*` reports to confirm timing and health.
