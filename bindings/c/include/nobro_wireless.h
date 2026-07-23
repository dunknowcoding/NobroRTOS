#ifndef NOBRO_WIRELESS_H
#define NOBRO_WIRELESS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NOBRO_WIRELESS_API_VERSION 0x0103u

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

/* Adaptive traffic is an optional policy above a concrete link. Fixed storage
 * remains the default; heap storage is valid only when the application enables
 * and prices it explicitly. */
typedef enum nobro_wireless_storage_mode {
    NOBRO_WIRELESS_STORAGE_FIXED = 0,
    NOBRO_WIRELESS_STORAGE_CALLER_POOL = 1,
    NOBRO_WIRELESS_STORAGE_HEAP = 2
} nobro_wireless_storage_mode_t;

typedef enum nobro_wireless_pacing {
    NOBRO_WIRELESS_PACING_FIXED = 0,
    NOBRO_WIRELESS_PACING_ADAPTIVE = 1
} nobro_wireless_pacing_t;

typedef struct nobro_wireless_retry_policy {
    uint8_t max_attempts;
    uint64_t initial_backoff_us;
    uint64_t max_backoff_us;
} nobro_wireless_retry_policy_t;

typedef struct nobro_wireless_recovery_state {
    uint8_t failed_attempts;
    uint64_t next_attempt_us;
} nobro_wireless_recovery_state_t;

typedef struct nobro_wireless_adaptive_policy {
    nobro_wireless_storage_mode_t storage_mode;
    uint16_t queue_slots;
    uint16_t payload_bytes_per_slot;
    nobro_wireless_retry_policy_t retry;
    uint64_t batch_window_us;
    uint16_t max_batch_messages;
} nobro_wireless_adaptive_policy_t;

typedef struct nobro_wireless_message_contract {
    uint64_t offered_at_us;
    uint64_t deadline_us;
    uint64_t expires_at_us;
    uint8_t priority;
    bool batchable;
} nobro_wireless_message_contract_t;

typedef struct nobro_wireless_adaptive_diagnostics {
    uint32_t offered_messages;
    uint64_t offered_bytes;
    uint32_t delivered_messages;
    uint64_t delivered_bytes;
    uint32_t deadline_misses;
    uint32_t expired_messages;
    uint32_t cancelled_messages;
    uint32_t backpressure_rejections;
    uint32_t retry_attempts;
    uint32_t retry_exhaustions;
    uint32_t link_down_deferrals;
    uint32_t window_deferrals;
    uint32_t backend_rejections;
    uint32_t completion_wakes;
    uint32_t radio_wake_batches;
    uint64_t latency_sum_us;
    uint64_t latency_max_us;
} nobro_wireless_adaptive_diagnostics_t;

/* Result returned by a concrete C/C++ transport callback. Link/window
 * deferrals do not spend the backend retry budget. */
typedef enum nobro_wireless_send_result {
    NOBRO_WIRELESS_SEND_OK = 0,
    NOBRO_WIRELESS_SEND_LINK_DOWN = 1,
    NOBRO_WIRELESS_SEND_WINDOW_EXHAUSTED = 2,
    NOBRO_WIRELESS_SEND_BACKEND_REJECTED = 3,
    NOBRO_WIRELESS_SEND_PAYLOAD_TOO_LARGE = 4,
    NOBRO_WIRELESS_SEND_DEADLINE_ELAPSED = 5
} nobro_wireless_send_result_t;

static inline nobro_wireless_adaptive_policy_t nobro_wireless_responsive_policy(
    uint16_t queue_slots,
    uint16_t payload_bytes_per_slot) {
    nobro_wireless_adaptive_policy_t policy = {
        NOBRO_WIRELESS_STORAGE_FIXED,
        queue_slots,
        payload_bytes_per_slot,
        {3u, 1000u, 100000u},
        0u,
        1u
    };
    return policy;
}

static inline nobro_wireless_adaptive_policy_t nobro_wireless_low_energy_policy(
    uint16_t queue_slots,
    uint16_t payload_bytes_per_slot,
    uint64_t batch_window_us) {
    nobro_wireless_adaptive_policy_t policy = {
        NOBRO_WIRELESS_STORAGE_FIXED,
        queue_slots,
        payload_bytes_per_slot,
        {4u, 10000u, 1000000u},
        batch_window_us,
        8u
    };
    return policy;
}

static inline nobro_wireless_message_contract_t nobro_wireless_best_effort_message(
    uint64_t offered_at_us,
    uint64_t expires_at_us) {
    nobro_wireless_message_contract_t contract = {
        offered_at_us,
        expires_at_us,
        expires_at_us,
        0u,
        true
    };
    return contract;
}

static inline nobro_wireless_message_contract_t nobro_wireless_urgent_message(
    uint64_t offered_at_us,
    uint64_t deadline_us) {
    nobro_wireless_message_contract_t contract = {
        offered_at_us,
        deadline_us,
        deadline_us,
        UINT8_MAX,
        false
    };
    return contract;
}

static inline bool nobro_wireless_retry_policy_valid(
    const nobro_wireless_retry_policy_t *policy) {
    return policy != NULL
        && policy->max_attempts != 0u
        && policy->initial_backoff_us <= policy->max_backoff_us
        && (policy->max_attempts == 1u || policy->initial_backoff_us != 0u);
}

static inline bool nobro_wireless_policy_valid(
    const nobro_wireless_adaptive_policy_t *policy) {
    return policy != NULL
        && policy->storage_mode <= NOBRO_WIRELESS_STORAGE_HEAP
        && policy->queue_slots != 0u
        && policy->payload_bytes_per_slot != 0u
        && nobro_wireless_retry_policy_valid(&policy->retry)
        && policy->max_batch_messages != 0u;
}

static inline bool nobro_wireless_message_valid(
    const nobro_wireless_message_contract_t *contract) {
    return contract != NULL
        && contract->offered_at_us <= contract->deadline_us
        && contract->deadline_us <= contract->expires_at_us;
}

static inline void nobro_wireless_recovery_reset(
    nobro_wireless_recovery_state_t *state) {
    if (state != NULL) {
        state->failed_attempts = 0u;
        state->next_attempt_us = 0u;
    }
}

static inline bool nobro_wireless_recovery_ready(
    const nobro_wireless_recovery_state_t *state,
    uint64_t now_us) {
    return state != NULL && now_us >= state->next_attempt_us;
}

/* Record one failed lifecycle attempt. A true result admits another attempt
 * at next_attempt_us; false means the explicit attempt budget is exhausted. */
static inline bool nobro_wireless_recovery_failed(
    nobro_wireless_recovery_state_t *state,
    const nobro_wireless_retry_policy_t *policy,
    uint64_t now_us) {
    if (state == NULL || !nobro_wireless_retry_policy_valid(policy)) {
        return false;
    }
    if (state->failed_attempts != UINT8_MAX) {
        state->failed_attempts++;
    }
    if (state->failed_attempts >= policy->max_attempts) {
        return false;
    }
    uint64_t delay = policy->initial_backoff_us;
    for (uint8_t shift = 1u; shift < state->failed_attempts; ++shift) {
        if (delay >= policy->max_backoff_us || delay > UINT64_MAX / 2u) {
            delay = policy->max_backoff_us;
            break;
        }
        delay *= 2u;
    }
    if (delay > policy->max_backoff_us) {
        delay = policy->max_backoff_us;
    }
    state->next_attempt_us = UINT64_MAX - now_us < delay
        ? UINT64_MAX : now_us + delay;
    return true;
}

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

#define NOBRO_BLE_GATT_VALUE_MAX 20u

typedef enum nobro_ble_event_kind {
    NOBRO_BLE_CONNECTED = 1,
    NOBRO_BLE_DISCONNECTED = 2,
    NOBRO_BLE_GATT_READ = 3,
    NOBRO_BLE_GATT_WRITE = 4,
    NOBRO_BLE_NOTIFICATION_COMPLETE = 5
} nobro_ble_event_kind_t;

/* One caller-owned, fixed-capacity BLE event. Arduino facades use logical
 * connection/attribute ids; vendor-specific handles never escape this ABI. */
typedef struct nobro_ble_event {
    nobro_ble_event_kind_t kind;
    uint16_t connection_id;
    uint16_t attribute_handle;
    uint8_t value[NOBRO_BLE_GATT_VALUE_MAX];
    uint8_t value_len;
} nobro_ble_event_t;

typedef struct nobro_ble_stack_diagnostics {
    uint32_t advertisements;
    uint32_t advertisement_stops;
    uint32_t events;
    uint32_t gatt_responses;
    uint32_t deadline_misses;
    uint32_t recoveries;
    uint32_t transport_faults;
} nobro_ble_stack_diagnostics_t;

#ifdef __cplusplus
}
#endif

#endif
