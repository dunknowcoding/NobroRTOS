#ifndef NOBRO_SERVO_H
#define NOBRO_SERVO_H

#include <stdint.h>

typedef enum {
    NOBRO_SERVO_DOWN = 0,
    NOBRO_SERVO_READY,
    NOBRO_SERVO_SUSPENDED,
    NOBRO_SERVO_FAULTED
} nobro_servo_state_t;

typedef enum {
    NOBRO_SERVO_OK = 0,
    NOBRO_SERVO_INVALID_CONFIG,
    NOBRO_SERVO_INVALID_COMMAND,
    NOBRO_SERVO_NOT_READY,
    NOBRO_SERVO_DEADLINE_MISS,
    NOBRO_SERVO_TRANSPORT_ERROR
} nobro_servo_result_t;

typedef struct {
    uint32_t sequence;
    uint8_t channel;
    uint32_t pulse_us;
    uint32_t submitted_us;
    uint32_t deadline_us;
    uint32_t completed_us;
} nobro_servo_receipt_t;

#endif
