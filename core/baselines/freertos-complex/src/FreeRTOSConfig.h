#ifndef FREERTOS_CONFIG_H
#define FREERTOS_CONFIG_H

#include <stdint.h>

#define configUSE_PREEMPTION                    1
#define configUSE_TIME_SLICING                  1
#define configUSE_PORT_OPTIMISED_TASK_SELECTION 0
#define configCPU_CLOCK_HZ                      64000000UL
#define configTICK_RATE_HZ                      1000U
#define configMAX_PRIORITIES                    6U
#define configMINIMAL_STACK_SIZE                96U
#define configMAX_TASK_NAME_LEN                 12U
#define configUSE_16_BIT_TICKS                  0
#define configIDLE_SHOULD_YIELD                 1
#define configUSE_IDLE_HOOK                     0
#define configUSE_TICK_HOOK                     0
#define configUSE_TASK_NOTIFICATIONS            0
#define configUSE_MUTEXES                       0
#define configUSE_RECURSIVE_MUTEXES             0
#define configUSE_COUNTING_SEMAPHORES           0
#define configQUEUE_REGISTRY_SIZE               0
#define configUSE_QUEUE_SETS                    0
#define configUSE_TIMERS                        0
#define configUSE_CO_ROUTINES                   0
#define configCHECK_FOR_STACK_OVERFLOW          0
#define configUSE_MALLOC_FAILED_HOOK            0
#define configSUPPORT_STATIC_ALLOCATION         1
#define configSUPPORT_DYNAMIC_ALLOCATION        0
#define configNUM_THREAD_LOCAL_STORAGE_POINTERS 0
#define configUSE_TRACE_FACILITY                0
#define configGENERATE_RUN_TIME_STATS           0
#define configUSE_STATS_FORMATTING_FUNCTIONS    0
#define configENABLE_BACKWARD_COMPATIBILITY     0
#define configKERNEL_INTERRUPT_PRIORITY         255U
#define configMAX_SYSCALL_INTERRUPT_PRIORITY    128U
#define configPRIO_BITS                         3U
#define configENABLE_FPU                        1

/* Route the official Cortex-M port directly into cortex-m-rt's vector names. */
#define vPortSVCHandler                         SVCall
#define xPortPendSVHandler                      PendSV
#define xPortSysTickHandler                     SysTick

#define INCLUDE_vTaskDelay                      1
#define INCLUDE_vTaskDelayUntil                 1
#define INCLUDE_vTaskSuspend                    0
#define INCLUDE_vTaskDelete                     0
#define INCLUDE_xTaskGetSchedulerState          0
#ifdef NOBRO_RAM_RUN
#define INCLUDE_xTaskGetCurrentTaskHandle       1
#define INCLUDE_xTaskGetIdleTaskHandle          1
#define INCLUDE_uxTaskGetStackHighWaterMark     1
#else
#define INCLUDE_xTaskGetCurrentTaskHandle       0
#define INCLUDE_xTaskGetIdleTaskHandle          0
#define INCLUDE_uxTaskGetStackHighWaterMark     0
#endif

/* BENCH_INSTRUMENTATION_BEGIN */
#ifdef NOBRO_RAM_RUN
void nobro_trace_switch_in(void);
void nobro_trace_switch_out(void);
#define traceTASK_SWITCHED_IN()  nobro_trace_switch_in()
#define traceTASK_SWITCHED_OUT() nobro_trace_switch_out()
#endif
/* BENCH_INSTRUMENTATION_END */

#define configASSERT(condition) do { if (!(condition)) { __asm volatile("bkpt #0"); for (;;) {} } } while (0)

#endif
