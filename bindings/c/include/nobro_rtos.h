/*
 * NobroRTOS public C ABI helpers.
 *
 * This header is intentionally dependency-light: it mirrors fixed host-readable
 * report layouts and inline decoding helpers so C firmware, host tools, and
 * test harnesses can inspect NobroRTOS reports without linking Rust code.
 */

#ifndef NOBRO_RTOS_H
#define NOBRO_RTOS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define NOBRO_STATIC_ASSERT(cond, msg) _Static_assert((cond), msg)
#else
#define NOBRO_STATIC_ASSERT_JOIN_(a, b) a##b
#define NOBRO_STATIC_ASSERT_JOIN(a, b) NOBRO_STATIC_ASSERT_JOIN_(a, b)
#define NOBRO_STATIC_ASSERT(cond, msg) \
    typedef char NOBRO_STATIC_ASSERT_JOIN(nobro_static_assert_, __LINE__)[(cond) ? 1 : -1]
#endif

#define NOBRO_REPORT_VERSION 1u
#define NOBRO_BOARD_PROFILE_REPORT_MAGIC 0x4E424250u
#define NOBRO_BOARD_PACKAGE_REPORT_MAGIC 0x4E42424Bu
#define NOBRO_MANIFEST_REPORT_MAGIC 0x4E424D46u
#define NOBRO_ADAPTER_COMPAT_REPORT_MAGIC 0x4E424143u
#define NOBRO_ADMISSION_REPORT_MAGIC 0x4E424144u
#define NOBRO_RUNTIME_REPORT_MAGIC 0x4E425254u
#define NOBRO_HEALTH_REPORT_MAGIC 0x4E42484Cu
#define NOBRO_EVENT_LOG_REPORT_MAGIC 0x4E42454Cu
#define NOBRO_MODULE_RUNTIME_REPORT_MAGIC 0x4E424D52u
#define NOBRO_DEGRADE_APPLICATION_REPORT_MAGIC 0x4E424447u
#define NOBRO_AI_MODEL_REPORT_MAGIC 0x4E424149u
#define NOBRO_ROS_BRIDGE_REPORT_MAGIC 0x4E425253u
#define NOBRO_FNV1A32_OFFSET 0x811C9DC5u
#define NOBRO_FNV1A32_PRIME 0x01000193u

#define NOBRO_AI_PREFLIGHT_MODEL_ID_MISMATCH (1u << 0)
#define NOBRO_AI_PREFLIGHT_INPUT_TOO_LARGE (1u << 1)
#define NOBRO_AI_PREFLIGHT_OUTPUT_TOO_SMALL (1u << 2)
#define NOBRO_AI_PREFLIGHT_RAM_EXCEEDED (1u << 3)
#define NOBRO_AI_PREFLIGHT_ROUTE_UNAVAILABLE (1u << 4)
#define NOBRO_AI_PREFLIGHT_DEGRADED_FALLBACK (1u << 5)
#define NOBRO_AI_PREFLIGHT_STALE_SNAPSHOT (1u << 6)
#define NOBRO_AI_PREFLIGHT_STALE_TOO_OLD (1u << 7)
#define NOBRO_AI_PREFLIGHT_ENDPOINT_CIRCUIT_OPEN (1u << 8)

#define NOBRO_ROS_PREFLIGHT_PAYLOAD_TOO_LARGE (1u << 0)
#define NOBRO_ROS_PREFLIGHT_RESPONSE_TOO_SMALL (1u << 1)
#define NOBRO_ROS_PREFLIGHT_TIMEOUT_EXCEEDED (1u << 2)
#define NOBRO_ROS_PREFLIGHT_QUEUE_DEPTH_ZERO (1u << 3)

typedef enum nobro_report_status {
    NOBRO_REPORT_STATUS_MISSING = 1,
    NOBRO_REPORT_STATUS_IN_PROGRESS = 2,
    NOBRO_REPORT_STATUS_PASS = 3,
    NOBRO_REPORT_STATUS_FAIL = 4,
    NOBRO_REPORT_STATUS_CORRUPT = 5,
} nobro_report_status_t;

typedef enum nobro_boot_stage {
    NOBRO_BOOT_STAGE_BOARD_PROFILE = 1,
    NOBRO_BOOT_STAGE_BOARD_PACKAGE = 2,
    NOBRO_BOOT_STAGE_MANIFEST = 3,
    NOBRO_BOOT_STAGE_ADAPTER_COMPATIBILITY = 4,
    NOBRO_BOOT_STAGE_ADMISSION = 5,
    NOBRO_BOOT_STAGE_RUNTIME = 6,
} nobro_boot_stage_t;

typedef enum nobro_ai_backend_kind {
    NOBRO_AI_BACKEND_ON_DEVICE = 1,
    NOBRO_AI_BACKEND_REMOTE_API = 2,
    NOBRO_AI_BACKEND_EDGE_SIDECAR = 3,
    NOBRO_AI_BACKEND_HYBRID = 4,
} nobro_ai_backend_kind_t;

typedef enum nobro_ai_route_preference {
    NOBRO_AI_ROUTE_LOCAL_ONLY = 1,
    NOBRO_AI_ROUTE_PREFER_LOCAL = 2,
    NOBRO_AI_ROUTE_PREFER_REMOTE = 3,
    NOBRO_AI_ROUTE_HYBRID_FALLBACK = 4,
} nobro_ai_route_preference_t;

typedef enum nobro_ai_route_target {
    NOBRO_AI_TARGET_ON_DEVICE = 1,
    NOBRO_AI_TARGET_REMOTE_API = 2,
    NOBRO_AI_TARGET_EDGE_SIDECAR = 3,
    NOBRO_AI_TARGET_STALE_SNAPSHOT = 4,
    NOBRO_AI_TARGET_DEGRADED_FALLBACK = 5,
    NOBRO_AI_TARGET_UNAVAILABLE = 6,
} nobro_ai_route_target_t;

typedef enum nobro_ros_bridge_transport {
    NOBRO_ROS_TRANSPORT_SERIAL = 1,
    NOBRO_ROS_TRANSPORT_UDP = 2,
    NOBRO_ROS_TRANSPORT_RADIO = 3,
    NOBRO_ROS_TRANSPORT_SHARED_MEMORY = 4,
    NOBRO_ROS_TRANSPORT_CUSTOM = 255,
} nobro_ros_bridge_transport_t;

typedef struct nobro_ai_model_contract {
    uint32_t model_id;
    uint8_t backend;
    uint16_t input_bytes_max;
    uint16_t output_bytes_max;
    uint32_t arena_bytes;
    uint32_t timeout_us;
    uint32_t stale_after_us;
} nobro_ai_model_contract_t;

typedef struct nobro_ai_runtime_state {
    uint8_t local_ready;
    uint8_t endpoint_ready;
    uint32_t last_success_age_us;
    uint8_t consecutive_endpoint_failures;
} nobro_ai_runtime_state_t;

typedef struct nobro_ai_route_policy {
    uint8_t preference;
    uint32_t stale_after_us;
    uint8_t endpoint_failure_limit;
} nobro_ai_route_policy_t;

typedef struct nobro_ai_route_decision {
    uint8_t target;
    uint8_t endpoint_circuit_open;
    uint8_t uses_stale_snapshot;
} nobro_ai_route_decision_t;

typedef struct nobro_ai_invocation_limits {
    uint32_t output_capacity_bytes;
    uint32_t scratch_bytes;
    uint32_t available_ram_bytes;
    uint32_t budget_us;
    uint32_t max_stale_us;
    uint8_t allow_stale_snapshot;
    uint8_t allow_degraded_fallback;
    uint8_t allow_unavailable;
    uint8_t allow_endpoint_circuit_open;
} nobro_ai_invocation_limits_t;

typedef struct nobro_ai_invocation_preflight {
    nobro_ai_route_decision_t route;
    uint32_t required_ram_bytes;
    uint32_t available_ram_bytes;
    uint32_t error_bits;
} nobro_ai_invocation_preflight_t;

typedef struct nobro_ros_topic_contract {
    uint32_t name_hash;
    uint32_t message_type_hash;
    uint8_t depth;
    uint16_t max_message_bytes;
} nobro_ros_topic_contract_t;

typedef struct nobro_ros_service_contract {
    uint32_t name_hash;
    uint16_t request_bytes_max;
    uint16_t response_bytes_max;
    uint32_t timeout_us;
} nobro_ros_service_contract_t;

typedef struct nobro_ros_action_contract {
    uint32_t name_hash;
    uint16_t goal_bytes_max;
    uint16_t feedback_bytes_max;
    uint16_t result_bytes_max;
    uint32_t timeout_us;
} nobro_ros_action_contract_t;

typedef struct nobro_ros_parameter_contract {
    uint32_t name_hash;
    uint16_t value_bytes_max;
} nobro_ros_parameter_contract_t;

typedef struct nobro_ros_bridge_preflight {
    uint32_t required_buffer_bytes;
    uint32_t error_bits;
} nobro_ros_bridge_preflight_t;

typedef struct nobro_ros_bridge_contract {
    uint8_t transport;
    uint32_t bridge_id_hash;
    uint8_t topic_count;
    uint8_t service_count;
    uint8_t action_count;
    uint8_t parameter_count;
    uint32_t total_buffer_bytes;
    uint32_t max_timeout_us;
} nobro_ros_bridge_contract_t;

typedef struct nobro_board_profile_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t platform_hash;
    uint32_t board_hash;
    uint32_t app_flash_start;
    uint32_t flash_budget_bytes;
    uint32_t ram_budget_bytes;
    uint32_t sample_pool_slots;
    uint32_t max_modules;
    uint32_t servo_pin;
    uint32_t servo_center_us;
    uint32_t led_pin;
    uint32_t mvk_trigger_pin;
    uint32_t checksum;
} nobro_board_profile_report_t;

typedef struct nobro_board_package_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t valid;
    uint32_t platform_hash;
    uint32_t board_hash;
    uint32_t boot_layout;
    uint32_t app_flash_start;
    uint32_t app_flash_len_bytes;
    uint32_t ram_start;
    uint32_t ram_len_bytes;
    uint32_t flash_budget_bytes;
    uint32_t ram_budget_bytes;
    uint32_t sample_pool_slots;
    uint32_t max_modules;
    uint32_t led_pin;
    uint32_t servo_pin;
    uint32_t mvk_trigger_pin;
    uint32_t error_code;
    uint32_t checksum;
} nobro_board_package_report_t;

typedef struct nobro_manifest_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t valid;
    uint32_t module_count;
    uint32_t fingerprint;
    uint32_t required_bits;
    uint32_t owned_bits;
    uint32_t flash_used_bytes;
    uint32_t ram_used_bytes;
    uint32_t pool_used_slots;
    uint32_t error_code;
    uint32_t error_module_tag;
    uint32_t error_capability_bits;
    uint32_t checksum;
} nobro_manifest_report_t;

typedef struct nobro_adapter_compat_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t compatible;
    uint32_t adapter_count;
    uint32_t required_bits;
    uint32_t owned_bits;
    uint32_t flash_used_bytes;
    uint32_t ram_used_bytes;
    uint32_t pool_used_slots;
    uint32_t error_code;
    uint32_t error_module_tag;
    uint32_t error_capability_bits;
    uint32_t checksum;
} nobro_adapter_compat_report_t;

typedef struct nobro_ai_model_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t backend;
    uint32_t model_id;
    uint32_t input_bytes_max;
    uint32_t output_bytes_max;
    uint32_t arena_bytes;
    uint32_t timeout_us;
    uint32_t route_preference;
    uint32_t stale_after_us;
    uint32_t endpoint_failure_limit;
    uint32_t checksum;
} nobro_ai_model_report_t;

typedef struct nobro_ros_bridge_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t transport;
    uint32_t bridge_id_hash;
    uint32_t topic_count;
    uint32_t service_count;
    uint32_t action_count;
    uint32_t parameter_count;
    uint32_t total_buffer_bytes;
    uint32_t max_timeout_us;
    uint32_t checksum;
} nobro_ros_bridge_report_t;

typedef struct nobro_admission_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t admitted;
    uint32_t module_count;
    uint32_t startup_len;
    uint32_t flash_used_bytes;
    uint32_t flash_limit_bytes;
    uint32_t ram_used_bytes;
    uint32_t ram_limit_bytes;
    uint32_t pool_used_slots;
    uint32_t pool_limit_slots;
    uint32_t error_code;
    uint32_t checksum;
} nobro_admission_report_t;

typedef struct nobro_runtime_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t state;
    uint32_t module_count;
    uint32_t mailbox_len;
    uint32_t mailbox_dropped;
    uint32_t alarm_len;
    uint32_t next_alarm_due_us_lo;
    uint32_t next_alarm_due_us_hi;
    uint32_t kv_len;
    uint32_t kv_writes;
    uint32_t kv_deletes;
    uint32_t quota_flash_used_bytes;
    uint32_t quota_ram_used_bytes;
    uint32_t quota_pool_used_slots;
    uint32_t event_count;
    uint32_t dropped_events;
    uint32_t checksum;
} nobro_runtime_report_t;

typedef struct nobro_health_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t module_tag;
    uint32_t total_errors;
    uint32_t consecutive_errors;
    uint32_t last_error;
    uint32_t last_action;
    uint32_t event_count;
    uint32_t dropped_events;
    uint32_t error_events;
    uint32_t fatal_events;
    uint32_t last_seen_us_lo;
    uint32_t last_seen_us_hi;
    uint32_t checksum;
} nobro_health_report_t;

typedef struct nobro_event_log_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t event_count;
    uint32_t capacity;
    uint32_t dropped_events;
    uint32_t latest_seq;
    uint32_t latest_at_us_lo;
    uint32_t latest_at_us_hi;
    uint32_t latest_module_tag;
    uint32_t latest_severity;
    uint32_t latest_kind;
    uint32_t latest_payload_kind;
    uint32_t latest_payload0;
    uint32_t latest_payload1;
    uint32_t checksum;
} nobro_event_log_report_t;

typedef struct nobro_module_runtime_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t module_count;
    uint32_t capacity;
    uint32_t active_count;
    uint32_t suspended_count;
    uint32_t faulted_count;
    uint32_t recovering_count;
    uint32_t disabled_count;
    uint32_t latest_module_tag;
    uint32_t latest_state;
    uint32_t latest_fault_count;
    uint32_t latest_recovery_count;
    uint32_t latest_change_us_lo;
    uint32_t latest_change_us_hi;
    uint32_t checksum;
} nobro_module_runtime_report_t;

typedef struct nobro_degrade_application_report {
    uint32_t magic;
    uint32_t version;
    uint32_t completed;
    uint32_t requested_count;
    uint32_t disabled_count;
    uint32_t already_disabled_count;
    uint32_t reason;
    uint32_t applied_at_us_lo;
    uint32_t applied_at_us_hi;
    uint32_t checksum;
} nobro_degrade_application_report_t;

NOBRO_STATIC_ASSERT(sizeof(nobro_board_profile_report_t) == 15u * sizeof(uint32_t),
                    "unexpected board profile report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_board_package_report_t) == 20u * sizeof(uint32_t),
                    "unexpected board package report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_manifest_report_t) == 15u * sizeof(uint32_t),
                    "unexpected manifest report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_adapter_compat_report_t) == 14u * sizeof(uint32_t),
                    "unexpected adapter compatibility report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_ai_model_report_t) == 13u * sizeof(uint32_t),
                    "unexpected AI model report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_ros_bridge_report_t) == 12u * sizeof(uint32_t),
                    "unexpected ROS bridge report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_admission_report_t) == 14u * sizeof(uint32_t),
                    "unexpected admission report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_runtime_report_t) == 19u * sizeof(uint32_t),
                    "unexpected runtime report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_health_report_t) == 15u * sizeof(uint32_t),
                    "unexpected health report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_event_log_report_t) == 16u * sizeof(uint32_t),
                    "unexpected event log report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_module_runtime_report_t) == 17u * sizeof(uint32_t),
                    "unexpected module runtime report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_degrade_application_report_t) == 10u * sizeof(uint32_t),
                    "unexpected degrade application report size");

static inline uint32_t nobro_report_checksum_words(const uint32_t *words, size_t word_count) {
    uint32_t checksum = 0u;
    size_t index = 0u;
    for (index = 0u; index < word_count; ++index) {
        checksum ^= words[index];
    }
    return checksum;
}

static inline uint32_t nobro_stable_hash32_bytes(const uint8_t *bytes, size_t byte_count) {
    uint32_t result = NOBRO_FNV1A32_OFFSET;
    size_t index = 0u;
    for (index = 0u; index < byte_count; ++index) {
        result ^= bytes[index];
        result *= NOBRO_FNV1A32_PRIME;
    }
    return result;
}

static inline uint32_t nobro_stable_hash32_cstr(const char *value) {
    uint32_t result = NOBRO_FNV1A32_OFFSET;
    while (*value != '\0') {
        result ^= (uint8_t)(*value);
        result *= NOBRO_FNV1A32_PRIME;
        ++value;
    }
    return result;
}

static inline uint32_t nobro_ros_topic_buffer_bytes(nobro_ros_topic_contract_t topic) {
    return (uint32_t)topic.depth * (uint32_t)topic.max_message_bytes;
}

static inline uint32_t nobro_ros_service_buffer_bytes(nobro_ros_service_contract_t service) {
    return (uint32_t)service.request_bytes_max + (uint32_t)service.response_bytes_max;
}

static inline uint32_t nobro_ros_action_buffer_bytes(nobro_ros_action_contract_t action) {
    return (uint32_t)action.goal_bytes_max
        + (uint32_t)action.feedback_bytes_max
        + (uint32_t)action.result_bytes_max;
}

static inline uint8_t nobro_ros_preflight_passing(
    nobro_ros_bridge_preflight_t preflight
) {
    return preflight.error_bits == 0u ? 1u : 0u;
}

static inline uint8_t nobro_ros_preflight_has_error(
    nobro_ros_bridge_preflight_t preflight,
    uint32_t error
) {
    return (preflight.error_bits & error) != 0u ? 1u : 0u;
}

static inline nobro_ros_bridge_preflight_t nobro_ros_topic_preflight(
    nobro_ros_topic_contract_t topic,
    uint16_t payload_bytes
) {
    uint32_t error_bits = 0u;
    nobro_ros_bridge_preflight_t preflight;

    if (topic.depth == 0u) {
        error_bits |= NOBRO_ROS_PREFLIGHT_QUEUE_DEPTH_ZERO;
    }
    if (payload_bytes > topic.max_message_bytes) {
        error_bits |= NOBRO_ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }

    preflight.required_buffer_bytes = nobro_ros_topic_buffer_bytes(topic);
    preflight.error_bits = error_bits;
    return preflight;
}

static inline nobro_ros_bridge_preflight_t nobro_ros_service_preflight(
    nobro_ros_service_contract_t service,
    uint16_t request_bytes,
    uint16_t response_capacity_bytes,
    uint32_t budget_us
) {
    uint32_t error_bits = 0u;
    nobro_ros_bridge_preflight_t preflight;

    if (request_bytes > service.request_bytes_max) {
        error_bits |= NOBRO_ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }
    if (response_capacity_bytes < service.response_bytes_max) {
        error_bits |= NOBRO_ROS_PREFLIGHT_RESPONSE_TOO_SMALL;
    }
    if (service.timeout_us > budget_us) {
        error_bits |= NOBRO_ROS_PREFLIGHT_TIMEOUT_EXCEEDED;
    }

    preflight.required_buffer_bytes = nobro_ros_service_buffer_bytes(service);
    preflight.error_bits = error_bits;
    return preflight;
}

static inline nobro_ros_bridge_preflight_t nobro_ros_action_preflight(
    nobro_ros_action_contract_t action,
    uint16_t goal_bytes,
    uint16_t feedback_capacity_bytes,
    uint16_t result_capacity_bytes,
    uint32_t budget_us
) {
    uint32_t error_bits = 0u;
    nobro_ros_bridge_preflight_t preflight;

    if (goal_bytes > action.goal_bytes_max) {
        error_bits |= NOBRO_ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }
    if (feedback_capacity_bytes < action.feedback_bytes_max
        || result_capacity_bytes < action.result_bytes_max) {
        error_bits |= NOBRO_ROS_PREFLIGHT_RESPONSE_TOO_SMALL;
    }
    if (action.timeout_us > budget_us) {
        error_bits |= NOBRO_ROS_PREFLIGHT_TIMEOUT_EXCEEDED;
    }

    preflight.required_buffer_bytes = nobro_ros_action_buffer_bytes(action);
    preflight.error_bits = error_bits;
    return preflight;
}

static inline nobro_ros_bridge_preflight_t nobro_ros_parameter_preflight(
    nobro_ros_parameter_contract_t parameter,
    uint16_t value_bytes
) {
    uint32_t error_bits = 0u;
    nobro_ros_bridge_preflight_t preflight;

    if (value_bytes > parameter.value_bytes_max) {
        error_bits |= NOBRO_ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }

    preflight.required_buffer_bytes = (uint32_t)parameter.value_bytes_max;
    preflight.error_bits = error_bits;
    return preflight;
}

static inline uint32_t nobro_ai_effective_stale_after_us(
    nobro_ai_route_policy_t policy,
    nobro_ai_model_contract_t contract
) {
    if (policy.stale_after_us == 0u) {
        return contract.stale_after_us;
    }
    if (contract.stale_after_us == 0u || policy.stale_after_us < contract.stale_after_us) {
        return policy.stale_after_us;
    }
    return contract.stale_after_us;
}

static inline uint32_t nobro_saturating_add_u32(uint32_t left, uint32_t right) {
    uint32_t result = left + right;
    return result < left ? UINT32_MAX : result;
}

static inline nobro_ai_route_decision_t nobro_ai_route_decide(
    nobro_ai_route_policy_t policy,
    nobro_ai_model_contract_t contract,
    nobro_ai_runtime_state_t state,
    uint32_t budget_us
) {
    uint8_t failure_limit = policy.endpoint_failure_limit == 0u
        ? 1u
        : policy.endpoint_failure_limit;
    uint8_t endpoint_circuit_open =
        state.consecutive_endpoint_failures >= failure_limit ? 1u : 0u;
    uint32_t stale_after_us = nobro_ai_effective_stale_after_us(policy, contract);
    uint8_t stale_ready = state.last_success_age_us <= stale_after_us ? 1u : 0u;
    uint8_t fits_budget = contract.timeout_us <= budget_us ? 1u : 0u;
    nobro_ai_route_decision_t decision = {
        NOBRO_AI_TARGET_DEGRADED_FALLBACK,
        endpoint_circuit_open,
        0u
    };

    if (!fits_budget) {
        if (stale_ready) {
            decision.target = NOBRO_AI_TARGET_STALE_SNAPSHOT;
            decision.uses_stale_snapshot = 1u;
        } else if (policy.preference == NOBRO_AI_ROUTE_LOCAL_ONLY) {
            decision.target = NOBRO_AI_TARGET_UNAVAILABLE;
        }
        return decision;
    }

    if (contract.backend == NOBRO_AI_BACKEND_ON_DEVICE && state.local_ready != 0u) {
        decision.target = NOBRO_AI_TARGET_ON_DEVICE;
        return decision;
    }
    if (contract.backend == NOBRO_AI_BACKEND_REMOTE_API
        && policy.preference != NOBRO_AI_ROUTE_LOCAL_ONLY
        && state.endpoint_ready != 0u
        && endpoint_circuit_open == 0u) {
        decision.target = NOBRO_AI_TARGET_REMOTE_API;
        return decision;
    }
    if (contract.backend == NOBRO_AI_BACKEND_EDGE_SIDECAR
        && policy.preference != NOBRO_AI_ROUTE_LOCAL_ONLY
        && state.endpoint_ready != 0u
        && endpoint_circuit_open == 0u) {
        decision.target = NOBRO_AI_TARGET_EDGE_SIDECAR;
        return decision;
    }
    if (contract.backend == NOBRO_AI_BACKEND_HYBRID) {
        if ((policy.preference == NOBRO_AI_ROUTE_LOCAL_ONLY
                || policy.preference == NOBRO_AI_ROUTE_PREFER_LOCAL)
            && state.local_ready != 0u) {
            decision.target = NOBRO_AI_TARGET_ON_DEVICE;
            return decision;
        }
        if (policy.preference != NOBRO_AI_ROUTE_LOCAL_ONLY
            && state.endpoint_ready != 0u
            && endpoint_circuit_open == 0u) {
            decision.target = NOBRO_AI_TARGET_REMOTE_API;
            return decision;
        }
        if (state.local_ready != 0u) {
            decision.target = NOBRO_AI_TARGET_ON_DEVICE;
            return decision;
        }
    }

    if (stale_ready) {
        decision.target = NOBRO_AI_TARGET_STALE_SNAPSHOT;
        decision.uses_stale_snapshot = 1u;
    } else if (policy.preference == NOBRO_AI_ROUTE_LOCAL_ONLY) {
        decision.target = NOBRO_AI_TARGET_UNAVAILABLE;
    }
    return decision;
}

static inline uint8_t nobro_ai_preflight_passing(
    nobro_ai_invocation_preflight_t preflight
) {
    return preflight.error_bits == 0u ? 1u : 0u;
}

static inline uint8_t nobro_ai_preflight_has_error(
    nobro_ai_invocation_preflight_t preflight,
    uint32_t error
) {
    return (preflight.error_bits & error) != 0u ? 1u : 0u;
}

static inline nobro_ai_invocation_preflight_t nobro_ai_invocation_preflight(
    nobro_ai_route_policy_t policy,
    nobro_ai_model_contract_t contract,
    nobro_ai_runtime_state_t state,
    uint32_t model_id,
    uint32_t input_bytes,
    nobro_ai_invocation_limits_t limits
) {
    nobro_ai_route_decision_t route =
        nobro_ai_route_decide(policy, contract, state, limits.budget_us);
    uint32_t local_arena_bytes =
        (contract.backend == NOBRO_AI_BACKEND_ON_DEVICE
         || contract.backend == NOBRO_AI_BACKEND_HYBRID)
            ? contract.arena_bytes
            : 0u;
    uint32_t required_ram_bytes = nobro_saturating_add_u32(
        nobro_saturating_add_u32(
            nobro_saturating_add_u32(input_bytes, limits.output_capacity_bytes),
            limits.scratch_bytes
        ),
        local_arena_bytes
    );
    uint32_t error_bits = 0u;
    nobro_ai_invocation_preflight_t preflight;

    if (model_id != contract.model_id) {
        error_bits |= NOBRO_AI_PREFLIGHT_MODEL_ID_MISMATCH;
    }
    if (input_bytes > (uint32_t)contract.input_bytes_max) {
        error_bits |= NOBRO_AI_PREFLIGHT_INPUT_TOO_LARGE;
    }
    if (limits.output_capacity_bytes < (uint32_t)contract.output_bytes_max) {
        error_bits |= NOBRO_AI_PREFLIGHT_OUTPUT_TOO_SMALL;
    }
    if (required_ram_bytes > limits.available_ram_bytes) {
        error_bits |= NOBRO_AI_PREFLIGHT_RAM_EXCEEDED;
    }
    if (route.target == NOBRO_AI_TARGET_UNAVAILABLE && limits.allow_unavailable == 0u) {
        error_bits |= NOBRO_AI_PREFLIGHT_ROUTE_UNAVAILABLE;
    }
    if (route.target == NOBRO_AI_TARGET_DEGRADED_FALLBACK
        && limits.allow_degraded_fallback == 0u) {
        error_bits |= NOBRO_AI_PREFLIGHT_DEGRADED_FALLBACK;
    }
    if (route.uses_stale_snapshot != 0u && limits.allow_stale_snapshot == 0u) {
        error_bits |= NOBRO_AI_PREFLIGHT_STALE_SNAPSHOT;
    }
    if (route.uses_stale_snapshot != 0u
        && limits.max_stale_us > 0u
        && state.last_success_age_us > limits.max_stale_us) {
        error_bits |= NOBRO_AI_PREFLIGHT_STALE_TOO_OLD;
    }
    if (route.endpoint_circuit_open != 0u && limits.allow_endpoint_circuit_open == 0u) {
        error_bits |= NOBRO_AI_PREFLIGHT_ENDPOINT_CIRCUIT_OPEN;
    }

    preflight.route = route;
    preflight.required_ram_bytes = required_ram_bytes;
    preflight.available_ram_bytes = limits.available_ram_bytes;
    preflight.error_bits = error_bits;
    return preflight;
}

static inline nobro_report_status_t nobro_report_status_from_checksum(
    uint32_t expected_magic,
    uint32_t magic,
    uint32_t version,
    uint32_t completed,
    uint32_t ok,
    int has_ok_field,
    uint32_t checksum,
    uint32_t computed_checksum
) {
    if (magic == 0u && version == 0u && checksum == 0u) {
        return NOBRO_REPORT_STATUS_MISSING;
    }
    if (magic != expected_magic || version != NOBRO_REPORT_VERSION) {
        return NOBRO_REPORT_STATUS_CORRUPT;
    }
    if (completed == 0u) {
        return NOBRO_REPORT_STATUS_IN_PROGRESS;
    }
    if (checksum != computed_checksum) {
        return NOBRO_REPORT_STATUS_CORRUPT;
    }
    if (has_ok_field != 0 && ok == 0u) {
        return NOBRO_REPORT_STATUS_FAIL;
    }
    return NOBRO_REPORT_STATUS_PASS;
}

static inline uint32_t nobro_board_profile_report_checksum(
    const nobro_board_profile_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->platform_hash
        ^ report->board_hash ^ report->app_flash_start ^ report->flash_budget_bytes
        ^ report->ram_budget_bytes ^ report->sample_pool_slots ^ report->max_modules
        ^ report->servo_pin ^ report->servo_center_us ^ report->led_pin
        ^ report->mvk_trigger_pin;
}

static inline nobro_report_status_t nobro_board_profile_report_status(
    const nobro_board_profile_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_BOARD_PROFILE_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_board_profile_report_checksum(report)
    );
}

static inline uint32_t nobro_board_package_report_checksum(
    const nobro_board_package_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->valid
        ^ report->platform_hash ^ report->board_hash ^ report->boot_layout
        ^ report->app_flash_start ^ report->app_flash_len_bytes ^ report->ram_start
        ^ report->ram_len_bytes ^ report->flash_budget_bytes ^ report->ram_budget_bytes
        ^ report->sample_pool_slots ^ report->max_modules ^ report->led_pin
        ^ report->servo_pin ^ report->mvk_trigger_pin ^ report->error_code;
}

static inline nobro_report_status_t nobro_board_package_report_status(
    const nobro_board_package_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_BOARD_PACKAGE_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        report->valid,
        1,
        report->checksum,
        nobro_board_package_report_checksum(report)
    );
}

static inline uint32_t nobro_manifest_report_checksum(const nobro_manifest_report_t *report) {
    return report->magic ^ report->version ^ report->completed ^ report->valid
        ^ report->module_count ^ report->fingerprint ^ report->required_bits
        ^ report->owned_bits ^ report->flash_used_bytes ^ report->ram_used_bytes
        ^ report->pool_used_slots ^ report->error_code ^ report->error_module_tag
        ^ report->error_capability_bits;
}

static inline nobro_report_status_t nobro_manifest_report_status(
    const nobro_manifest_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_MANIFEST_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        report->valid,
        1,
        report->checksum,
        nobro_manifest_report_checksum(report)
    );
}

static inline uint32_t nobro_adapter_compat_report_checksum(
    const nobro_adapter_compat_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->compatible
        ^ report->adapter_count ^ report->required_bits ^ report->owned_bits
        ^ report->flash_used_bytes ^ report->ram_used_bytes ^ report->pool_used_slots
        ^ report->error_code ^ report->error_module_tag ^ report->error_capability_bits;
}

static inline nobro_report_status_t nobro_adapter_compat_report_status(
    const nobro_adapter_compat_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_ADAPTER_COMPAT_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        report->compatible,
        1,
        report->checksum,
        nobro_adapter_compat_report_checksum(report)
    );
}

static inline uint32_t nobro_ai_model_report_checksum(
    const nobro_ai_model_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->backend
        ^ report->model_id ^ report->input_bytes_max ^ report->output_bytes_max
        ^ report->arena_bytes ^ report->timeout_us ^ report->route_preference
        ^ report->stale_after_us ^ report->endpoint_failure_limit;
}

static inline nobro_report_status_t nobro_ai_model_report_status(
    const nobro_ai_model_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_AI_MODEL_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_ai_model_report_checksum(report)
    );
}

static inline uint32_t nobro_ros_bridge_report_checksum(
    const nobro_ros_bridge_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->transport
        ^ report->bridge_id_hash ^ report->topic_count ^ report->service_count
        ^ report->action_count ^ report->parameter_count ^ report->total_buffer_bytes
        ^ report->max_timeout_us;
}

static inline nobro_report_status_t nobro_ros_bridge_report_status(
    const nobro_ros_bridge_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_ROS_BRIDGE_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_ros_bridge_report_checksum(report)
    );
}

static inline uint32_t nobro_admission_report_checksum(
    const nobro_admission_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->admitted
        ^ report->module_count ^ report->startup_len ^ report->flash_used_bytes
        ^ report->flash_limit_bytes ^ report->ram_used_bytes ^ report->ram_limit_bytes
        ^ report->pool_used_slots ^ report->pool_limit_slots ^ report->error_code;
}

static inline nobro_report_status_t nobro_admission_report_status(
    const nobro_admission_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_ADMISSION_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        report->admitted,
        1,
        report->checksum,
        nobro_admission_report_checksum(report)
    );
}

static inline uint32_t nobro_runtime_report_checksum(
    const nobro_runtime_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->state
        ^ report->module_count ^ report->mailbox_len ^ report->mailbox_dropped
        ^ report->alarm_len ^ report->next_alarm_due_us_lo
        ^ report->next_alarm_due_us_hi ^ report->kv_len ^ report->kv_writes
        ^ report->kv_deletes ^ report->quota_flash_used_bytes
        ^ report->quota_ram_used_bytes ^ report->quota_pool_used_slots
        ^ report->event_count ^ report->dropped_events;
}

static inline nobro_report_status_t nobro_runtime_report_status(
    const nobro_runtime_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_RUNTIME_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_runtime_report_checksum(report)
    );
}

static inline uint32_t nobro_health_report_checksum(
    const nobro_health_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->module_tag
        ^ report->total_errors ^ report->consecutive_errors ^ report->last_error
        ^ report->last_action ^ report->event_count ^ report->dropped_events
        ^ report->error_events ^ report->fatal_events ^ report->last_seen_us_lo
        ^ report->last_seen_us_hi;
}

static inline nobro_report_status_t nobro_health_report_status(
    const nobro_health_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_HEALTH_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_health_report_checksum(report)
    );
}

static inline uint32_t nobro_event_log_report_checksum(
    const nobro_event_log_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->event_count
        ^ report->capacity ^ report->dropped_events ^ report->latest_seq
        ^ report->latest_at_us_lo ^ report->latest_at_us_hi
        ^ report->latest_module_tag ^ report->latest_severity ^ report->latest_kind
        ^ report->latest_payload_kind ^ report->latest_payload0 ^ report->latest_payload1;
}

static inline nobro_report_status_t nobro_event_log_report_status(
    const nobro_event_log_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_EVENT_LOG_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_event_log_report_checksum(report)
    );
}

static inline uint32_t nobro_module_runtime_report_checksum(
    const nobro_module_runtime_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->module_count
        ^ report->capacity ^ report->active_count ^ report->suspended_count
        ^ report->faulted_count ^ report->recovering_count ^ report->disabled_count
        ^ report->latest_module_tag ^ report->latest_state ^ report->latest_fault_count
        ^ report->latest_recovery_count ^ report->latest_change_us_lo
        ^ report->latest_change_us_hi;
}

static inline nobro_report_status_t nobro_module_runtime_report_status(
    const nobro_module_runtime_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_MODULE_RUNTIME_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_module_runtime_report_checksum(report)
    );
}

static inline uint32_t nobro_degrade_application_report_checksum(
    const nobro_degrade_application_report_t *report
) {
    return report->magic ^ report->version ^ report->completed ^ report->requested_count
        ^ report->disabled_count ^ report->already_disabled_count ^ report->reason
        ^ report->applied_at_us_lo ^ report->applied_at_us_hi;
}

static inline nobro_report_status_t nobro_degrade_application_report_status(
    const nobro_degrade_application_report_t *report
) {
    return nobro_report_status_from_checksum(
        NOBRO_DEGRADE_APPLICATION_REPORT_MAGIC,
        report->magic,
        report->version,
        report->completed,
        1u,
        0,
        report->checksum,
        nobro_degrade_application_report_checksum(report)
    );
}

static inline int nobro_report_status_is_passing(nobro_report_status_t status) {
    return status == NOBRO_REPORT_STATUS_PASS;
}

#ifdef __cplusplus
}
#endif

#endif /* NOBRO_RTOS_H */
