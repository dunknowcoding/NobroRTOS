/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef NOBRO_CORE_H
#define NOBRO_CORE_H

/*
 * NobroRTOS Core byte-machine contract.
 *
 * This API deliberately uses no function parameters or return values.  Calls
 * exchange data through one fixed byte layout, so C and assembly applications
 * do not inherit a compiler-specific register calling convention.
 */

#include <limits.h>

#if CHAR_BIT != 8
#error "nobro_core requires an eight-bit addressable byte"
#endif

#ifndef NOBRO_CORE_TASK_COUNT
#error "generate nobro_core_config.h before compiling the Core backend"
#endif

#ifndef NOBRO_CORE_MAILBOX_CAPACITY
#define NOBRO_CORE_MAILBOX_CAPACITY 0
#endif

#ifndef NOBRO_CORE_ENABLE_IDLE_HOOK
#define NOBRO_CORE_ENABLE_IDLE_HOOK 0
#endif

#ifndef NOBRO_CORE_ENABLE_WATCHDOG_HOOK
#define NOBRO_CORE_ENABLE_WATCHDOG_HOOK 0
#endif

#if NOBRO_CORE_TASK_COUNT < 1 || NOBRO_CORE_TASK_COUNT > 8
#error "NOBRO_CORE_TASK_COUNT must be in 1..8"
#endif

#if NOBRO_CORE_MAILBOX_CAPACITY < 0 || NOBRO_CORE_MAILBOX_CAPACITY > 8
#error "NOBRO_CORE_MAILBOX_CAPACITY must be in 0..8"
#endif

#if defined(__SDCC_mcs51)
#define NOBRO_CORE_CODE __code
#define NOBRO_CORE_DATA __data
#elif defined(__C51__)
#define NOBRO_CORE_CODE code
#define NOBRO_CORE_DATA data
#elif defined(__ICC8051__)
#define NOBRO_CORE_CODE __code
#define NOBRO_CORE_DATA __data
#elif defined(__AVR__)
#include <avr/pgmspace.h>
#define NOBRO_CORE_CODE PROGMEM
#define NOBRO_CORE_DATA
#define NOBRO_CORE_READ_BYTE(address) pgm_read_byte(address)
#else
#define NOBRO_CORE_CODE
#define NOBRO_CORE_DATA
#endif

#ifndef NOBRO_CORE_READ_BYTE
#define NOBRO_CORE_READ_BYTE(address) (*(address))
#endif

typedef unsigned char nobro_core_u8;
typedef nobro_core_u8 nobro_core_tick;

typedef char nobro_core_u8_must_be_one_byte[
    (sizeof(nobro_core_u8) == 1) ? 1 : -1];

enum nobro_core_fault {
    NOBRO_CORE_FAULT_NONE = 0,
    NOBRO_CORE_FAULT_CONFIG = 1,
    NOBRO_CORE_FAULT_DISPATCH_LATE = 2,
    NOBRO_CORE_FAULT_RELEASE_SKIPPED = 4,
    NOBRO_CORE_FAULT_MAILBOX_FULL = 8
};

enum nobro_core_result {
    NOBRO_CORE_RESULT_EMPTY = 0,
    NOBRO_CORE_RESULT_OK = 1,
    NOBRO_CORE_RESULT_REJECTED = 2
};

struct nobro_core_abi_block {
    volatile nobro_core_u8 now;
    volatile nobro_core_u8 sleep_ticks;
    volatile nobro_core_u8 active_task;
    volatile nobro_core_u8 message;
    volatile nobro_core_u8 result;
    volatile nobro_core_u8 faults;
};

struct nobro_core_task_spec {
    nobro_core_u8 period_ticks;
    nobro_core_u8 dispatch_deadline_ticks;
};

typedef char nobro_core_abi_must_be_six_bytes[
    (sizeof(struct nobro_core_abi_block) == 6) ? 1 : -1];
typedef char nobro_core_task_spec_must_be_two_bytes[
    (sizeof(struct nobro_core_task_spec) == 2) ? 1 : -1];

extern NOBRO_CORE_DATA struct nobro_core_abi_block nobro_core_abi;
extern NOBRO_CORE_CODE const struct nobro_core_task_spec
    nobro_core_tasks[NOBRO_CORE_TASK_COUNT];

/* Public ABI entry points. Inputs and outputs live in nobro_core_abi. */
void nobro_core_reset(void);
void nobro_core_dispatch(void);
void nobro_core_post(void);
void nobro_core_take(void);
void nobro_core_recover(void);

/* Generated application router. It reads nobro_core_abi.active_task. */
void nobro_core_step(void);

#if NOBRO_CORE_ENABLE_IDLE_HOOK
void nobro_core_idle(void);
#endif

#if NOBRO_CORE_ENABLE_WATCHDOG_HOOK
void nobro_core_watchdog(void);
#endif

#endif
