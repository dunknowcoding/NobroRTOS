// SPDX-License-Identifier: GPL-3.0-only
#include <NobroRTOSCore.h>

using nobro::core::ADMITTED;
using nobro::core::Scheduler;
using nobro::core::Task;

Scheduler<1> scheduler;

void toggleLed(void *)
{
    digitalWrite(LED_BUILTIN, !digitalRead(LED_BUILTIN));
}

void setup()
{
    pinMode(LED_BUILTIN, OUTPUT);
    static const Task tasks[] = {
        Task::periodic(1, 0, 500000, 500000, 200, toggleLed),
    };
    if (scheduler.begin(tasks, micros()) != ADMITTED) {
        for (;;) {
        }
    }
}

void loop()
{
    scheduler.releaseDue(micros());
    scheduler.runReady();
    yield();
}
