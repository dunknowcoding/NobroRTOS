#ifndef NOBRO_PULSE_H
#define NOBRO_PULSE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    NOBRO_PULSE_DOWN = 0,
    NOBRO_PULSE_READY,
    NOBRO_PULSE_BUSY,
    NOBRO_PULSE_SUSPENDED,
    NOBRO_PULSE_FAULTED
} nobro_pulse_state_t;

typedef enum {
    NOBRO_PULSE_OK = 0,
    NOBRO_PULSE_INVALID_CONFIG,
    NOBRO_PULSE_NOT_READY,
    NOBRO_PULSE_TOO_MANY_SYMBOLS,
    NOBRO_PULSE_BACKPRESSURED,
    NOBRO_PULSE_TRANSPORT,
    NOBRO_PULSE_DEADLINE_MISS
} nobro_pulse_error_t;

typedef struct {
    uint32_t frequency_hz;
    uint8_t resolution_bits;
} nobro_pwm_config_t;

typedef struct {
    uint16_t high_ticks;
    uint16_t low_ticks;
} nobro_pulse_symbol_t;

typedef struct {
    uint32_t writes;
    uint32_t symbols;
    uint32_t backpressure_rejections;
    uint32_t oversized_rejections;
    uint32_t transport_errors;
    uint32_t deadline_misses;
    uint32_t recoveries;
} nobro_pulse_diagnostics_t;

#ifdef __cplusplus
}
#endif

#endif
