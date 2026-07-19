#ifndef NOBRO_WIRELESS_H
#define NOBRO_WIRELESS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NOBRO_WIRELESS_API_VERSION 0x0101u

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

typedef enum nobro_stack_family {
    NOBRO_STACK_FAMILY_WIFI = 1,
    NOBRO_STACK_FAMILY_BLE = 2,
    NOBRO_STACK_FAMILY_THREAD = 3
} nobro_stack_family_t;

typedef enum nobro_stack_state {
    NOBRO_STACK_DOWN = 0,
    NOBRO_STACK_STARTING = 1,
    NOBRO_STACK_READY = 2,
    NOBRO_STACK_QUIESCED = 3,
    NOBRO_STACK_FAULTED = 4
} nobro_stack_state_t;

typedef enum nobro_stack_result {
    NOBRO_STACK_OK = 0,
    NOBRO_STACK_INVALID_CONFIG = 1,
    NOBRO_STACK_INVALID_IDENTITY = 2,
    NOBRO_STACK_NOT_READY = 3,
    NOBRO_STACK_BUSY = 4,
    NOBRO_STACK_DEADLINE_ELAPSED = 5,
    NOBRO_STACK_QUEUE_FULL = 6,
    NOBRO_STACK_BACKEND_FAULT = 7,
    NOBRO_STACK_ASSOCIATION_REJECTED = 8
} nobro_stack_result_t;

typedef struct nobro_stack_identity {
    const char *backend_id;
    nobro_stack_family_t family;
    uint16_t mtu;
    uint16_t rx_queue_slots;
    uint16_t tx_queue_slots;
    uint16_t service_slots;
    uint16_t characteristic_slots;
} nobro_stack_identity_t;

/* Runtime-only borrowed association material. Never persist this structure in
 * board metadata, reports, or diagnostics. */
typedef struct nobro_wifi_credentials {
    const uint8_t *ssid;
    size_t ssid_len;
    const uint8_t *secret;
    size_t secret_len;
} nobro_wifi_credentials_t;

typedef struct nobro_wifi_network {
    uint8_t ssid[32];
    uint8_t ssid_len;
    uint8_t channel;
    int8_t rssi_dbm;
    bool secured;
} nobro_wifi_network_t;

typedef struct nobro_wifi_stack_diagnostics {
    uint32_t scans;
    uint32_t scan_results;
    uint32_t truncated_scan_results;
    uint32_t join_attempts;
    uint32_t join_failures;
    uint32_t deadline_misses;
    uint32_t leaves;
    uint32_t recoveries;
    uint32_t transport_faults;
} nobro_wifi_stack_diagnostics_t;

#ifdef __cplusplus
}
#endif

#endif
