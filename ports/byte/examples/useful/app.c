/* SPDX-License-Identifier: GPL-3.0-only */
#include "nobro_core_config.h"

volatile nobro_core_u8 nobro_example_result;
static nobro_core_u8 sample;

void sensor_step(void)
{
    nobro_core_abi.message = ++sample;
    nobro_core_post();
}

void control_step(void)
{
    nobro_core_take();
    if (nobro_core_abi.result == NOBRO_CORE_RESULT_OK)
        nobro_example_result = nobro_core_abi.message;
}

void nobro_app_idle(void)
{
    nobro_core_u8 now = nobro_core_abi.now;
    nobro_core_u8 sleep = nobro_core_abi.sleep_ticks;
    nobro_core_abi.now = (nobro_core_u8)(now + sleep);
}

void nobro_app_watchdog(void)
{
    /* The exact board binding feeds or checks its hardware watchdog here. */
}

int main(void)
{
    nobro_core_u8 cycle;
    nobro_core_abi.now = 0u;
    nobro_core_reset();
    for (cycle = 0u; cycle < 20u; ++cycle)
        nobro_core_dispatch();

    /* Prove bounded rejection and deterministic recovery before stopping. */
    nobro_core_abi.message = 0xa1u;
    nobro_core_post();
    nobro_core_abi.message = 0xa2u;
    nobro_core_post();
    nobro_core_abi.message = 0xa3u;
    nobro_core_post();
    if ((nobro_core_abi.faults & NOBRO_CORE_FAULT_MAILBOX_FULL) != 0u)
        nobro_core_recover();
    for (;;) {
        /* A board binding owns sleep or reset after application completion. */
    }
}
