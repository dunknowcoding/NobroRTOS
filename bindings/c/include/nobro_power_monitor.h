#ifndef NOBRO_POWER_MONITOR_H
#define NOBRO_POWER_MONITOR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NOBRO_POWER_MONITOR_API_VERSION 0x0100u

typedef enum nobro_power_monitor_state {
    NOBRO_POWER_MONITOR_DOWN = 0,
    NOBRO_POWER_MONITOR_READY,
    NOBRO_POWER_MONITOR_SUSPENDED,
    NOBRO_POWER_MONITOR_FAULTED
} nobro_power_monitor_state_t;

typedef enum nobro_power_monitor_result {
    NOBRO_POWER_MONITOR_OK = 0,
    NOBRO_POWER_MONITOR_INVALID_CONFIG,
    NOBRO_POWER_MONITOR_INVALID_CHANNEL,
    NOBRO_POWER_MONITOR_NOT_READY,
    NOBRO_POWER_MONITOR_TIMEOUT,
    NOBRO_POWER_MONITOR_TRANSPORT_ERROR,
    NOBRO_POWER_MONITOR_DEADLINE_MISS
} nobro_power_monitor_result_t;

typedef struct nobro_power_channel_sample {
    uint8_t channel;
    int64_t bus_uv;
    int64_t shunt_uv;
    int64_t current_ua;
    int64_t power_uw;
    uint32_t sequence;
    uint64_t timestamp_us;
} nobro_power_channel_sample_t;

#ifdef __cplusplus
}
#endif
#endif
