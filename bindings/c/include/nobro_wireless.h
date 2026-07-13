#ifndef NOBRO_WIRELESS_H
#define NOBRO_WIRELESS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NOBRO_WIRELESS_API_VERSION 0x0100u

typedef enum nobro_wireless_protocol {
    NOBRO_WIRELESS_UNKNOWN = 0,
    NOBRO_WIRELESS_BLE = 1,
    NOBRO_WIRELESS_WIFI = 2,
    NOBRO_WIRELESS_ZIGBEE = 3,
    NOBRO_WIRELESS_THREAD = 4,
    NOBRO_WIRELESS_RFID = 5,
    NOBRO_WIRELESS_LORA = 6,
    NOBRO_WIRELESS_PROPRIETARY = 7
} nobro_wireless_protocol_t;

typedef enum nobro_wireless_state {
    NOBRO_WIRELESS_DOWN = 0,
    NOBRO_WIRELESS_JOINING = 1,
    NOBRO_WIRELESS_UP = 2
} nobro_wireless_state_t;

typedef struct nobro_wireless_diagnostics {
    uint32_t tx_accepted;
    uint32_t tx_rejected;
    uint32_t rx_packets;
    uint32_t read_errors;
    uint32_t recoveries;
    uint32_t recovery_failures;
} nobro_wireless_diagnostics_t;

#ifdef __cplusplus
}
#endif

#endif
