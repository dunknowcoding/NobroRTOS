#include <stdint.h>

#include "FreeRTOS.h"
#include "queue.h"
#include "task.h"

volatile uint32_t BASELINE_REPORT[4] = { 0, 0, 0, 0 };

/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
volatile uint32_t RUNTIME_BUSY_CYCLES = 0;
volatile uint32_t RUNTIME_PEAK_TASK_STACK_BYTES = 0;
volatile uint32_t RUNTIME_LATENCY[7] = { 0, 0, 0, 0, 0, 0, 0 };
volatile uint32_t RUNTIME_IDLE_CYCLES = 0;
volatile uint32_t RUNTIME_IDLE_ENTRIES = 0;
static uint32_t runtime_switch_start;
static uint32_t runtime_non_idle;
static TaskHandle_t runtime_task_handles[5];

static inline uint32_t runtime_cycles(void) {
    return *(volatile uint32_t *) 0xE0001004UL;
}

#define TIMER0 ((volatile uint32_t *) 0x40008000UL)
static void timing_init(void) {
    TIMER0[0x504U / 4U] = 0U;
    TIMER0[0x508U / 4U] = 3U;
    TIMER0[0x510U / 4U] = 4U;
    TIMER0[0] = 1U;
}

static uint32_t timing_micros(void) {
    TIMER0[0x040U / 4U] = 1U;
    return TIMER0[0x540U / 4U];
}

void nobro_trace_switch_in(void) {
    runtime_non_idle = (xTaskGetCurrentTaskHandle() != xTaskGetIdleTaskHandle());
    runtime_switch_start = runtime_cycles();
    if (runtime_non_idle == 0U) {
        RUNTIME_IDLE_ENTRIES++;
    }
}

void nobro_trace_switch_out(void) {
    if (runtime_non_idle != 0U) {
        RUNTIME_BUSY_CYCLES += runtime_cycles() - runtime_switch_start;
    } else {
        RUNTIME_IDLE_CYCLES += runtime_cycles() - runtime_switch_start;
    }
}

static void nobro_record_jitter(uint32_t *last, uint32_t period_us) {
    uint32_t now = timing_micros();
    uint32_t jitter = 0U;
    if (*last != 0U) {
        uint32_t interval = now - *last;
        jitter = (interval > period_us ? interval - period_us : period_us - interval) * 64U;
    }
    *last = now;
    RUNTIME_LATENCY[0]++;
    if (jitter > RUNTIME_LATENCY[1]) RUNTIME_LATENCY[1] = jitter;
    RUNTIME_LATENCY[2] += jitter;
    uint32_t bucket = jitter <= 64U ? 3U : (jitter <= 640U ? 4U : (jitter <= 6400U ? 5U : 6U));
    RUNTIME_LATENCY[bucket]++;
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
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    uint32_t expected = 0;
#endif
/* BENCH_INSTRUMENTATION_END */
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(10));
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
        nobro_record_jitter(&expected, 10000U);
#endif
/* BENCH_INSTRUMENTATION_END */
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
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    uint32_t expected = 0;
#endif
/* BENCH_INSTRUMENTATION_END */
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(20));
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
        nobro_record_jitter(&expected, 20000U);
#endif
/* BENCH_INSTRUMENTATION_END */
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
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    uint32_t expected = 0;
#endif
/* BENCH_INSTRUMENTATION_END */
    for (;;) {
        vTaskDelayUntil(&next, pdMS_TO_TICKS(50));
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
        nobro_record_jitter(&expected, 50000U);
#endif
/* BENCH_INSTRUMENTATION_END */
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
/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
    timing_init();
#endif
/* BENCH_INSTRUMENTATION_END */
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
