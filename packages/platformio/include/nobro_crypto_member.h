#ifndef NOBRO_CRYPTO_MEMBER_H
#define NOBRO_CRYPTO_MEMBER_H

#include <stdint.h>

typedef enum {
    NOBRO_CRYPTO_DOWN = 0,
    NOBRO_CRYPTO_READY,
    NOBRO_CRYPTO_SUSPENDED,
    NOBRO_CRYPTO_FAULTED
} nobro_crypto_state_t;

typedef enum {
    NOBRO_CRYPTO_OK = 0,
    NOBRO_CRYPTO_INVALID_CONFIG,
    NOBRO_CRYPTO_NOT_READY,
    NOBRO_CRYPTO_TOO_LARGE,
    NOBRO_CRYPTO_DEADLINE_MISS,
    NOBRO_CRYPTO_UNSUPPORTED,
    NOBRO_CRYPTO_AUTH_FAILED,
    NOBRO_CRYPTO_PROVIDER_ERROR
} nobro_crypto_result_t;

typedef enum {
    NOBRO_CRYPTO_RANDOM = 0,
    NOBRO_CRYPTO_SHA256,
    NOBRO_CRYPTO_AES_GCM_SEAL,
    NOBRO_CRYPTO_AES_GCM_OPEN
} nobro_crypto_operation_t;

typedef struct {
    uint32_t sequence;
    nobro_crypto_operation_t operation;
    uint32_t input_bytes;
    uint32_t submitted_us;
    uint32_t deadline_us;
    uint32_t completed_us;
    uint8_t hardware_accelerated;
} nobro_crypto_receipt_t;

typedef struct {
    uint32_t completed;
    uint32_t rejected;
    uint32_t deadline_misses;
    uint32_t provider_errors;
    uint32_t recoveries;
} nobro_crypto_diagnostics_t;

#endif
