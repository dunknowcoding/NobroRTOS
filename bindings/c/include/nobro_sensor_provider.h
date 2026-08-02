#ifndef NOBRO_SENSOR_PROVIDER_H
#define NOBRO_SENSOR_PROVIDER_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NOBRO_SENSOR_PROVIDER_API_VERSION 0x0100u

typedef enum nobro_sensor_provider_state {
    NOBRO_SENSOR_PROVIDER_DOWN = 0,
    NOBRO_SENSOR_PROVIDER_READY,
    NOBRO_SENSOR_PROVIDER_SUSPENDED,
    NOBRO_SENSOR_PROVIDER_FAULTED
} nobro_sensor_provider_state_t;

typedef enum nobro_sensor_transport {
    NOBRO_SENSOR_TRANSPORT_UNKNOWN = 0,
    NOBRO_SENSOR_TRANSPORT_GPIO_PULSE,
    NOBRO_SENSOR_TRANSPORT_GPIO_DIGITAL,
    NOBRO_SENSOR_TRANSPORT_ANALOG,
    NOBRO_SENSOR_TRANSPORT_UART,
    NOBRO_SENSOR_TRANSPORT_I2C,
    NOBRO_SENSOR_TRANSPORT_RS485,
    NOBRO_SENSOR_TRANSPORT_SPI,
    NOBRO_SENSOR_TRANSPORT_CAN
} nobro_sensor_transport_t;

typedef enum nobro_sensor_sample_status {
    NOBRO_SENSOR_SAMPLE_VALID = 0,
    NOBRO_SENSOR_SAMPLE_TIMEOUT,
    NOBRO_SENSOR_SAMPLE_OUT_OF_RANGE,
    NOBRO_SENSOR_SAMPLE_NOT_READY,
    NOBRO_SENSOR_SAMPLE_DISABLED,
    NOBRO_SENSOR_SAMPLE_TRANSPORT_ERROR
} nobro_sensor_sample_status_t;

typedef struct nobro_ranging_sample {
    uint32_t distance_mm;
    uint16_t signal_permille;
    uint32_t raw;
    uint32_t sequence;
    uint64_t timestamp_us;
    nobro_sensor_sample_status_t status;
} nobro_ranging_sample_t;

typedef struct nobro_presence_sample {
    bool detected;
    uint16_t strength_permille;
    bool rose;
    bool fell;
    uint32_t sequence;
    uint64_t timestamp_us;
    nobro_sensor_sample_status_t status;
} nobro_presence_sample_t;

#ifdef __cplusplus
}
#endif
#endif
