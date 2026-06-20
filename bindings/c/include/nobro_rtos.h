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

NOBRO_STATIC_ASSERT(sizeof(nobro_board_profile_report_t) == 15u * sizeof(uint32_t),
                    "unexpected board profile report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_board_package_report_t) == 20u * sizeof(uint32_t),
                    "unexpected board package report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_manifest_report_t) == 15u * sizeof(uint32_t),
                    "unexpected manifest report size");
NOBRO_STATIC_ASSERT(sizeof(nobro_adapter_compat_report_t) == 14u * sizeof(uint32_t),
                    "unexpected adapter compatibility report size");

static inline uint32_t nobro_report_checksum_words(const uint32_t *words, size_t word_count) {
    uint32_t checksum = 0u;
    size_t index = 0u;
    for (index = 0u; index < word_count; ++index) {
        checksum ^= words[index];
    }
    return checksum;
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

static inline int nobro_report_status_is_passing(nobro_report_status_t status) {
    return status == NOBRO_REPORT_STATUS_PASS;
}

#ifdef __cplusplus
}
#endif

#endif /* NOBRO_RTOS_H */
