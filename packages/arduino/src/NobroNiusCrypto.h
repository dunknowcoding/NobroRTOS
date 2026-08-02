#ifndef NOBRO_NIUS_CRYPTO_H
#define NOBRO_NIUS_CRYPTO_H

#include <NiusCrypto.h>
#include "nobro_crypto_member.h"

namespace nobro {

class NiusCryptoAdapter {
public:
    explicit NiusCryptoAdapter(ncrypto::CryptoEngine &engine)
        : engine_(engine), prefer_(ncrypto::CryptoEngine::Prefer::Auto),
          max_input_bytes_(0), sequence_(1), state_(NOBRO_CRYPTO_DOWN),
          diagnostics_{} {}

    nobro_crypto_result_t begin(
        uint32_t max_input_bytes,
        ncrypto::CryptoEngine::Prefer prefer = ncrypto::CryptoEngine::Prefer::Auto) {
        if (state_ != NOBRO_CRYPTO_DOWN || max_input_bytes == 0) {
            return NOBRO_CRYPTO_INVALID_CONFIG;
        }
        prefer_ = prefer;
        max_input_bytes_ = max_input_bytes;
        if (!engine_.begin(prefer_)) {
            state_ = NOBRO_CRYPTO_FAULTED;
            increment(diagnostics_.provider_errors);
            return NOBRO_CRYPTO_PROVIDER_ERROR;
        }
        state_ = NOBRO_CRYPTO_READY;
        return NOBRO_CRYPTO_OK;
    }

    nobro_crypto_result_t random(uint8_t *output, size_t bytes, uint32_t now_us,
                                 uint32_t deadline_us,
                                 nobro_crypto_receipt_t *receipt) {
        if (!admit(bytes, now_us, deadline_us, receipt)) return admission_result_;
        return finish(engine_.random(output, bytes), NOBRO_CRYPTO_RANDOM, bytes,
                      now_us, deadline_us, receipt);
    }

    nobro_crypto_result_t sha256(const uint8_t *input, size_t bytes,
                                 uint8_t output[NIUS_SHA256_BYTES], uint32_t now_us,
                                 uint32_t deadline_us,
                                 nobro_crypto_receipt_t *receipt) {
        if (!admit(bytes, now_us, deadline_us, receipt)) return admission_result_;
        return finish(engine_.sha256(input, bytes, output), NOBRO_CRYPTO_SHA256,
                      bytes, now_us, deadline_us, receipt);
    }

    nobro_crypto_result_t aesGcmSeal(
        const uint8_t key[NIUS_AES128_KEY], const uint8_t iv[NIUS_GCM_IV],
        const uint8_t *aad, size_t aad_bytes, const uint8_t *input,
        uint8_t *output, size_t bytes, uint8_t tag[NIUS_GCM_TAG],
        uint32_t now_us, uint32_t deadline_us, nobro_crypto_receipt_t *receipt) {
        if (aad_bytes > max_input_bytes_ || !admit(bytes, now_us, deadline_us, receipt))
            return aad_bytes > max_input_bytes_ ? reject(NOBRO_CRYPTO_TOO_LARGE)
                                                : admission_result_;
        return finish(engine_.aesGcmEncrypt(key, iv, aad, aad_bytes, input, output,
                                            bytes, tag),
                      NOBRO_CRYPTO_AES_GCM_SEAL, bytes, now_us, deadline_us, receipt);
    }

    nobro_crypto_result_t aesGcmOpen(
        const uint8_t key[NIUS_AES128_KEY], const uint8_t iv[NIUS_GCM_IV],
        const uint8_t *aad, size_t aad_bytes, const uint8_t *input,
        uint8_t *output, size_t bytes, const uint8_t tag[NIUS_GCM_TAG],
        uint32_t now_us, uint32_t deadline_us, nobro_crypto_receipt_t *receipt) {
        if (aad_bytes > max_input_bytes_ || !admit(bytes, now_us, deadline_us, receipt))
            return aad_bytes > max_input_bytes_ ? reject(NOBRO_CRYPTO_TOO_LARGE)
                                                : admission_result_;
        return finish(engine_.aesGcmDecrypt(key, iv, aad, aad_bytes, input, output,
                                            bytes, tag),
                      NOBRO_CRYPTO_AES_GCM_OPEN, bytes, now_us, deadline_us, receipt);
    }

    void quiesce() {
        if (state_ == NOBRO_CRYPTO_READY || state_ == NOBRO_CRYPTO_FAULTED) {
            engine_.end();
            state_ = NOBRO_CRYPTO_SUSPENDED;
        }
    }

    nobro_crypto_result_t recover() {
        if (state_ != NOBRO_CRYPTO_SUSPENDED) return NOBRO_CRYPTO_NOT_READY;
        if (!engine_.begin(prefer_)) {
            state_ = NOBRO_CRYPTO_FAULTED;
            increment(diagnostics_.provider_errors);
            return NOBRO_CRYPTO_PROVIDER_ERROR;
        }
        state_ = NOBRO_CRYPTO_READY;
        increment(diagnostics_.recoveries);
        return NOBRO_CRYPTO_OK;
    }

    void release() {
        engine_.end();
        max_input_bytes_ = 0;
        state_ = NOBRO_CRYPTO_DOWN;
    }

    nobro_crypto_state_t state() const { return state_; }
    nobro_crypto_diagnostics_t diagnostics() const { return diagnostics_; }
    const char *backendId() const { return engine_.backendName(); }

private:
    bool admit(size_t bytes, uint32_t now_us, uint32_t deadline_us,
               nobro_crypto_receipt_t *receipt) {
        if (state_ != NOBRO_CRYPTO_READY || receipt == nullptr) {
            admission_result_ = reject(NOBRO_CRYPTO_NOT_READY);
            return false;
        }
        if (bytes == 0 || bytes > max_input_bytes_) {
            admission_result_ = reject(NOBRO_CRYPTO_TOO_LARGE);
            return false;
        }
        if (static_cast<int32_t>(now_us - deadline_us) > 0) {
            increment(diagnostics_.deadline_misses);
            admission_result_ = reject(NOBRO_CRYPTO_DEADLINE_MISS);
            return false;
        }
        admission_result_ = NOBRO_CRYPTO_OK;
        return true;
    }

    nobro_crypto_result_t finish(ncrypto::CryptoStatus status,
                                 nobro_crypto_operation_t operation, size_t bytes,
                                 uint32_t submitted_us, uint32_t deadline_us,
                                 nobro_crypto_receipt_t *receipt) {
        const uint32_t completed_us = micros();
        const nobro_crypto_result_t mapped = map(status);
        if (mapped != NOBRO_CRYPTO_OK) {
            if (mapped == NOBRO_CRYPTO_PROVIDER_ERROR) {
                state_ = NOBRO_CRYPTO_FAULTED;
                increment(diagnostics_.provider_errors);
            }
            return reject(mapped);
        }
        if (static_cast<int32_t>(completed_us - deadline_us) > 0) {
            increment(diagnostics_.deadline_misses);
            return reject(NOBRO_CRYPTO_DEADLINE_MISS);
        }
        receipt->sequence = sequence_;
        receipt->operation = operation;
        receipt->input_bytes = static_cast<uint32_t>(bytes);
        receipt->submitted_us = submitted_us;
        receipt->deadline_us = deadline_us;
        receipt->completed_us = completed_us ? completed_us : 1U;
        receipt->hardware_accelerated = engine_.isHardwareAccelerated() ? 1U : 0U;
        sequence_ = sequence_ == UINT32_MAX ? 1U : sequence_ + 1U;
        increment(diagnostics_.completed);
        return NOBRO_CRYPTO_OK;
    }

    static nobro_crypto_result_t map(ncrypto::CryptoStatus status) {
        if (status == ncrypto::CryptoStatus::Ok) return NOBRO_CRYPTO_OK;
        if (status == ncrypto::CryptoStatus::Unsupported) return NOBRO_CRYPTO_UNSUPPORTED;
        if (status == ncrypto::CryptoStatus::AuthFailed) return NOBRO_CRYPTO_AUTH_FAILED;
        if (status == ncrypto::CryptoStatus::BadParam) return NOBRO_CRYPTO_INVALID_CONFIG;
        return NOBRO_CRYPTO_PROVIDER_ERROR;
    }

    nobro_crypto_result_t reject(nobro_crypto_result_t result) {
        increment(diagnostics_.rejected);
        return result;
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) ++value;
    }

    ncrypto::CryptoEngine &engine_;
    ncrypto::CryptoEngine::Prefer prefer_;
    uint32_t max_input_bytes_;
    uint32_t sequence_;
    nobro_crypto_state_t state_;
    nobro_crypto_diagnostics_t diagnostics_;
    nobro_crypto_result_t admission_result_ = NOBRO_CRYPTO_OK;
};

}  // namespace nobro

#endif
