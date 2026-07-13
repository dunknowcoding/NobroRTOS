#include <stdint.h>

#include "FreeRTOS.h"
#include "queue.h"
#include "task.h"

volatile uint32_t BASELINE_REPORT[4] = { 0, 0, 0, 0 };

/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
volatile uint32_t RUNTIME_BUSY_CYCLES = 0;
volatile uint32_t RUNTIME_PEAK_TASK_STACK_BYTES = 0;
static uint32_t runtime_switch_start;
static uint32_t runtime_non_idle;
static TaskHandle_t runtime_task_handles[5];

static inline uint32_t runtime_cycles(void) {
    return *(volatile uint32_t *) 0xE0001004UL;
}

void nobro_trace_switch_in(void) {
    runtime_non_idle = (xTaskGetCurrentTaskHandle() != xTaskGetIdleTaskHandle());
    runtime_switch_start = runtime_cycles();
}

void nobro_trace_switch_out(void) {
    if (runtime_non_idle != 0U) {
        RUNTIME_BUSY_CYCLES += runtime_cycles() - runtime_switch_start;
    }
}

static void nobro_measure_task_stacks(void) {
    uint32_t peak = 0;
    for (uint32_t index = 0; index < 5U; ++index) {
        uint32_t remaining = uxTaskGetStackHighWaterMark(runtime_task_handles[index]);
        uint32_t used = (128U - remaining) * sizeof(StackType_t);
        if (used > peak) {
            peak = used;
        }
    }
    RUNTIME_PEAK_TASK_STACK_BYTES = peak;
}
#endif
/* BENCH_INSTRUMENTATION_END */

#define GPIO_P0 ((volatile uint32_t *) 0x50000000UL)
#define PIN 15U
#define STACK_WORDS 128U

static StaticQueue_t fusion_queue_buffer;
static StaticQueue_t radio_queue_buffer;
static StaticQueue_t storage_queue_buffer;
static uint8_t fusion_queue_storage[sizeof(uint32_t)];
static uint8_t radio_queue_storage[sizeof(uint32_t)];
static uint8_t storage_queue_storage[sizeof(uint32_t)];
static QueueHandle_t fusion_queue;
static QueueHandle_t radio_queue;
static QueueHandle_t storage_queue;

static StaticTask_t task_buffers[5];
static StackType_t task_stacks[5][STACK_WORDS];
static StaticTask_t idle_task_buffer;
static StackType_t idle_stack[configMINIMAL_STACK_SIZE];

void vApplicationGetIdleTaskMemory(StaticTask_t **task,
                                   StackType_t **stack,
                                   uint32_t *words) {
    *task = &idle_task_buffer;
    *stack = idle_stack;
    *words = configMINIMAL_STACK_SIZE;
}

static void fusion_task(void *argument) {
    (void) argument;
    TickType_t next = xTaskGetTickCount();
    uint32_t fused = 0;
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(10));
        uint32_t samples = ++BASELINE_REPORT[1];
        uint32_t a = samples * 3U + 7U;
        uint32_t b = samples * 5U + 11U;
        fused = fused - (fused >> 3) + ((a ^ b) >> 3);
        if (xQueueSend(fusion_queue, &fused, 0) != pdPASS) {
            BASELINE_REPORT[3]++;
        }
    }
}

static void control_task(void *argument) {
    (void) argument;
    TickType_t next = xTaskGetTickCount();
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(20));
        uint32_t ticks = ++BASELINE_REPORT[0];
        GPIO_P0[(ticks & 1U) ? (0x50CU / 4U) : (0x508U / 4U)] = 1UL << PIN;
        uint32_t command;
        if (xQueueReceive(fusion_queue, &command, 0) == pdPASS) {
            if (xQueueSend(radio_queue, &command, 0) != pdPASS) {
                BASELINE_REPORT[3]++;
            }
            if (xQueueSend(storage_queue, &command, 0) != pdPASS) {
                BASELINE_REPORT[3]++;
            }
        }
    }
}

static void radio_task(void *argument) {
    (void) argument;
    TickType_t next = xTaskGetTickCount();
    uint32_t value;
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(50));
        if (xQueueReceive(radio_queue, &value, 0) == pdPASS) {
            BASELINE_REPORT[2]++;
        }
    }
}

static void storage_task(void *argument) {
    (void) argument;
    TickType_t next = xTaskGetTickCount();
    uint32_t ring[8] = { 0 };
    uint32_t head = 0;
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(100));
        uint32_t value;
        if (xQueueReceive(storage_queue, &value, 0) == pdPASS) {
            ring[head] = value;
            head = (head + 1U) & 7U;
            __asm volatile("" : : "r"(ring) : "memory");
        }
    }
}

static void diagnostics_task(void *argument) {
    (void) argument;
    TickType_t next = xTaskGetTickCount();
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(200));
        __asm volatile("" : : "r"(BASELINE_REPORT[3]) : "memory");
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
        nobro_trace_switch_out();
        nobro_measure_task_stacks();
        nobro_trace_switch_in();
#endif
/* BENCH_INSTRUMENTATION_END */
    }
}

void freertos_complex_start(void) {
    GPIO_P0[(0x700U / 4U) + PIN] = 1U;
    fusion_queue = xQueueCreateStatic(1, sizeof(uint32_t), fusion_queue_storage,
                                      &fusion_queue_buffer);
    radio_queue = xQueueCreateStatic(1, sizeof(uint32_t), radio_queue_storage,
                                     &radio_queue_buffer);
    storage_queue = xQueueCreateStatic(1, sizeof(uint32_t), storage_queue_storage,
                                       &storage_queue_buffer);

/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    runtime_task_handles[0] =
#endif
/* BENCH_INSTRUMENTATION_END */
        xTaskCreateStatic(fusion_task, "fusion", STACK_WORDS, 0, 5,
                          task_stacks[0], &task_buffers[0]);
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    runtime_task_handles[1] =
#endif
/* BENCH_INSTRUMENTATION_END */
        xTaskCreateStatic(control_task, "control", STACK_WORDS, 0, 4,
                          task_stacks[1], &task_buffers[1]);
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    runtime_task_handles[2] =
#endif
/* BENCH_INSTRUMENTATION_END */
        xTaskCreateStatic(radio_task, "radio", STACK_WORDS, 0, 3,
                          task_stacks[2], &task_buffers[2]);
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    runtime_task_handles[3] =
#endif
/* BENCH_INSTRUMENTATION_END */
        xTaskCreateStatic(storage_task, "storage", STACK_WORDS, 0, 2,
                          task_stacks[3], &task_buffers[3]);
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    runtime_task_handles[4] =
#endif
/* BENCH_INSTRUMENTATION_END */
        xTaskCreateStatic(diagnostics_task, "diagnostics", STACK_WORDS, 0, 1,
                          task_stacks[4], &task_buffers[4]);
    vTaskStartScheduler();
    for (;;) {}
}
