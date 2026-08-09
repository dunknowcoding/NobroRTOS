/* SPDX-License-Identifier: GPL-3.0-only */
#include "nobro_core_config.h"

volatile nobro_core_u8 nobro_example_result;

void control_step(void)
{
    ++nobro_example_result;
}

int main(void)
{
    nobro_core_u8 cycle;
    nobro_core_abi.now = 0u;
    nobro_core_reset();
    for (cycle = 0u; cycle < 8u; ++cycle) {
        nobro_core_u8 now;
        nobro_core_u8 sleep;
        nobro_core_dispatch();
        now = nobro_core_abi.now;
        sleep = nobro_core_abi.sleep_ticks;
        nobro_core_abi.now = (nobro_core_u8)(now + sleep);
    }
    for (;;) {
        /* A board binding owns sleep or reset after application completion. */
    }
}
