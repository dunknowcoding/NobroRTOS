# Arduino and PlatformIO use

NobroRTOS Core is packaged as a header-only Arduino library. `Scheduler<N>` owns
a fixed copy of exactly `N` task descriptors, periodic release timestamps, one
priority-to-task map, and one ready bitmap. It allocates no memory and creates
no hidden task stacks.

## Define a workload

```cpp
#include <NobroRTOSCore.h>

using nobro::core::Scheduler;
using nobro::core::Task;

void sample(void *context) {
    // Return before the declared WCET expires.
}

Scheduler<1> scheduler;

void setup() {
    static const Task tasks[] = {
        Task::periodic(1, 0, 1000, 1000, 40, sample),
    };
    const nobro::core::AdmissionResult result = scheduler.begin(tasks, micros());
    if (result != nobro::core::ADMITTED) {
        // Keep the application fail-closed or report the exact result.
    }
}

void loop() {
    scheduler.releaseDue(micros());
    scheduler.runReady();
    yield();
}
```

Priorities are unique values from 0 through 31; lower values run first. A
periodic interval, finish deadline, phase, and WCET must remain inside the
wrap-safe half of the 32-bit microsecond clock. Admission checks aggregate
periodic utilization and apply a conservative fixed-priority,
run-to-completion response-time test with one lower-priority blocking job.

Event tasks use `Task::event(...)` and are released with `markReady(index)` or
`markReadyById(id)`. They have no invented arrival rate, so the application
must separately bound the source's burst/rate and include that interference in
its measured WCET/deadline design. `runReady()` defaults to at most `N`
dispatches per call, preventing a callback that republishes work from trapping
the outer loop indefinitely.

## API at a glance

| API | Purpose |
| --- | --- |
| `Task::periodic(...)` | Declare a periodic callback with priority, period, finish deadline, WCET, optional context, and optional phase |
| `Task::event(...)` | Declare work released by an application event |
| `scheduler.begin(tasks, now)` | Copy and admit the fixed task table |
| `scheduler.releaseDue(now)` | Release every periodic task due at this time while preserving phase |
| `scheduler.markReady(index)` | Release a task by fixed index |
| `scheduler.markReadyById(id)` | Release a task by stable application ID |
| `scheduler.takeNext(index)` | Take the highest-priority ready task without calling it |
| `scheduler.runNext()` | Run one highest-priority ready callback |
| `scheduler.runReady(limit)` | Run a bounded number of ready callbacks |
| `scheduler.nextRelease(now, out)` | Report the next periodic release for a board-owned idle policy |

`begin()` returns an exact `AdmissionResult`. Applications can report
`DUPLICATE_ID`, `DUPLICATE_PRIORITY`, `INVALID_TIMING`,
`UTILIZATION_EXCEEDED`, or `DEADLINE_MISS` instead of entering an uncertain
schedule.

## Interrupt ownership

The public scheduler does not disable interrupts or assume a board-specific
atomic implementation. Call scheduler-mutating methods from one owner. If an
ISR releases an event, publish it through an atomic board flag or protect the
ready-state update with that board's correct critical section before calling
`markReady` from the scheduler owner. This keeps the portable package honest on
8-bit as well as 32-bit boards.

## Install

For Arduino IDE, download the tagged source archive, choose
`Sketch > Include Library > Add .ZIP Library...`, then open
`File > Examples > NobroRTOSCore > BlinkCore`. Library Manager installation
becomes available after the release is accepted into Arduino's index.

For PlatformIO, use the registry package after publication or pin the exact Git
tag:

```ini
[env:example]
platform = atmelavr
board = uno
framework = arduino
lib_deps = https://github.com/dunknowcoding/NobroRTOS.git#v1.0.0
```

`library.properties` and `library.json` intentionally share the Cargo workspace
version. The package metadata gate rejects version, license, header, example,
or archive drift before release.
