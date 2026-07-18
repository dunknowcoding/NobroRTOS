/*
 * Minimal plain-C task graph. Tier-C supplies startup, admission, and dispatch;
 * the application names each task, its rate, and what it does.
 */
#include "nobro_app.h"

static int32_t imu_step(void) {
    /* Read/publish one sample here. */
    return 0;
}

static int32_t control_step(void) {
    /* Consume the latest sample and update an actuator here. */
    (void)nobro_skipped_releases();
    (void)nobro_last_step_error();
    return 0;
}

static int32_t configure(void) {
    nobro_task_options_t control = NOBRO_TASK_OPTIONS_INIT;
    control.role = NOBRO_TASK_CONTROL;
    control.budget_us = 2000u;

    int32_t result = nobro_task("imu", HZ(100), imu_step);
    if (result < 0) return result;
    result = nobro_task_with("control", HZ(50), control_step, &control);
    if (result < 0) return result;
    result = nobro_wire("imu", "control", 8u);
    if (result < 0) return result;
    return nobro_run();
}

NOBRO_APP(configure)
