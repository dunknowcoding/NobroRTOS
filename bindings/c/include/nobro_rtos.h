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
#define NOBRO_AI_MODEL_REPORT_MAGIC 0x4E424149u
#define NOBRO_ROS_BRIDGE_REPORT_MAGIC 0x4E425253u
#define NOBRO_FNV1A32_OFFSET 0x811C9DC5u
#define NOBRO_FNV1A32_PRIME 0x01000193u

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
    uint8_t stale_ready = state.last_success_age_us <= policy.stale_after_us ? 1u : 0u;
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

static inline int nobro_report_status_is_passing(nobro_report_status_t status) {
    return status == NOBRO_REPORT_STATUS_PASS;
}

#ifdef __cplusplus
}
#endif

#endif /* NOBRO_RTOS_H */
