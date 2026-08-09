/* SPDX-License-Identifier: GPL-3.0-only */
#include "nobro_core_config.h"

NOBRO_CORE_DATA struct nobro_core_abi_block nobro_core_abi;
static NOBRO_CORE_DATA nobro_core_u8
    nobro_core_release[NOBRO_CORE_TASK_COUNT];

#if NOBRO_CORE_MAILBOX_CAPACITY > 0
static NOBRO_CORE_DATA nobro_core_u8
    nobro_core_mailbox[NOBRO_CORE_MAILBOX_CAPACITY];
static NOBRO_CORE_DATA nobro_core_u8 nobro_core_mailbox_head;
static NOBRO_CORE_DATA nobro_core_u8 nobro_core_mailbox_length;
#endif

static nobro_core_u8 nobro_core_due(nobro_core_u8 now, nobro_core_u8 release)
{
    return (nobro_core_u8)((nobro_core_u8)(now - release) < 128u);
}

static nobro_core_u8 nobro_core_valid_spec(
    nobro_core_u8 period, nobro_core_u8 deadline)
{
    return (nobro_core_u8)(period != 0u && period < 128u &&
        deadline != 0u && deadline <= period);
}

void nobro_core_reset(void)
{
    nobro_core_u8 task;
    for (task = 0u; task < NOBRO_CORE_TASK_COUNT; ++task)
        nobro_core_release[task] = nobro_core_abi.now;
#if NOBRO_CORE_MAILBOX_CAPACITY > 0
    nobro_core_mailbox_head = 0u;
    nobro_core_mailbox_length = 0u;
#endif
    nobro_core_abi.sleep_ticks = 0u;
    nobro_core_abi.active_task = 0xffu;
    nobro_core_abi.result = NOBRO_CORE_RESULT_OK;
    nobro_core_abi.faults = NOBRO_CORE_FAULT_NONE;
}

void nobro_core_dispatch(void)
{
    nobro_core_u8 task;
    nobro_core_u8 now;
    nobro_core_u8 next_sleep = 127u;

    nobro_core_abi.active_task = 0xffu;
    for (task = 0u; task < NOBRO_CORE_TASK_COUNT; ++task) {
        nobro_core_u8 period = NOBRO_CORE_READ_BYTE(
            &nobro_core_tasks[task].period_ticks);
        nobro_core_u8 deadline = NOBRO_CORE_READ_BYTE(
            &nobro_core_tasks[task].dispatch_deadline_ticks);
        nobro_core_u8 release = nobro_core_release[task];

        if (!nobro_core_valid_spec(period, deadline)) {
            nobro_core_abi.faults |= NOBRO_CORE_FAULT_CONFIG;
            continue;
        }
        /* A preceding run-to-completion step may have consumed timer ticks. */
        now = nobro_core_abi.now;
        if (nobro_core_due(now, release)) {
            nobro_core_u8 lateness = (nobro_core_u8)(now - release);
            if (lateness >= deadline)
                nobro_core_abi.faults |= NOBRO_CORE_FAULT_DISPATCH_LATE;
            nobro_core_abi.active_task = task;
            nobro_core_step();
            /* Re-sample before deciding whether another release was skipped. */
            now = nobro_core_abi.now;
            release = (nobro_core_u8)(release + period);
            if (nobro_core_due(now, release)) {
                nobro_core_abi.faults |= NOBRO_CORE_FAULT_RELEASE_SKIPPED;
                release = (nobro_core_u8)(now + period);
            }
            nobro_core_release[task] = release;
        }
    }

    /* Compute idle time from the final tick, not the pre-dispatch snapshot. */
    now = nobro_core_abi.now;
    for (task = 0u; task < NOBRO_CORE_TASK_COUNT; ++task) {
        nobro_core_u8 delta = (nobro_core_u8)(nobro_core_release[task] - now);
        if (nobro_core_due(now, nobro_core_release[task]))
            delta = 0u;
        if (delta < next_sleep)
            next_sleep = delta;
    }
    if ((nobro_core_abi.faults & NOBRO_CORE_FAULT_CONFIG) != 0u)
        next_sleep = 0u;
    nobro_core_abi.active_task = 0xffu;
    nobro_core_abi.sleep_ticks = next_sleep;
#if NOBRO_CORE_ENABLE_WATCHDOG_HOOK
    nobro_core_watchdog();
#endif
#if NOBRO_CORE_ENABLE_IDLE_HOOK
    if (next_sleep != 0u)
        nobro_core_idle();
#endif
}

void nobro_core_post(void)
{
#if NOBRO_CORE_MAILBOX_CAPACITY > 0
    nobro_core_u8 tail;
    if (nobro_core_mailbox_length >= NOBRO_CORE_MAILBOX_CAPACITY) {
        nobro_core_abi.faults |= NOBRO_CORE_FAULT_MAILBOX_FULL;
        nobro_core_abi.result = NOBRO_CORE_RESULT_REJECTED;
        return;
    }
    tail = (nobro_core_u8)(nobro_core_mailbox_head +
        nobro_core_mailbox_length);
    if (tail >= NOBRO_CORE_MAILBOX_CAPACITY)
        tail = (nobro_core_u8)(tail - NOBRO_CORE_MAILBOX_CAPACITY);
    nobro_core_mailbox[tail] = nobro_core_abi.message;
    ++nobro_core_mailbox_length;
    nobro_core_abi.result = NOBRO_CORE_RESULT_OK;
#else
    nobro_core_abi.faults |= NOBRO_CORE_FAULT_CONFIG;
    nobro_core_abi.result = NOBRO_CORE_RESULT_REJECTED;
#endif
}

void nobro_core_take(void)
{
#if NOBRO_CORE_MAILBOX_CAPACITY > 0
    if (nobro_core_mailbox_length == 0u) {
        nobro_core_abi.result = NOBRO_CORE_RESULT_EMPTY;
        return;
    }
    nobro_core_abi.message = nobro_core_mailbox[nobro_core_mailbox_head];
    ++nobro_core_mailbox_head;
    if (nobro_core_mailbox_head >= NOBRO_CORE_MAILBOX_CAPACITY)
        nobro_core_mailbox_head = 0u;
    --nobro_core_mailbox_length;
    nobro_core_abi.result = NOBRO_CORE_RESULT_OK;
#else
    nobro_core_abi.result = NOBRO_CORE_RESULT_EMPTY;
#endif
}

void nobro_core_recover(void)
{
    nobro_core_reset();
}
