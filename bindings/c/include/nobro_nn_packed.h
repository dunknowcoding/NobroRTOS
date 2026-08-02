#ifndef NOBRO_NN_PACKED_H
#define NOBRO_NN_PACKED_H

/* Heap-free Q4/Q2 dense inference for C99 and Harvard-memory MCUs.
 * Rows start on byte boundaries; low bit lanes are decoded first. */

#include <limits.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(__SDCC_mcs51)
#define NOBRO_NN_REENTRANT __reentrant
#else
#define NOBRO_NN_REENTRANT
#endif

typedef enum {
    NOBRO_NN_Q4 = 4,
    NOBRO_NN_Q2 = 2
} nobro_nn_packed_format_t;

typedef enum {
    NOBRO_NN_OK = 0,
    NOBRO_NN_INVALID_SHAPE = 1,
    NOBRO_NN_INVALID_QUANTIZATION = 2,
    NOBRO_NN_INVALID_PACKED_LENGTH = 3,
    NOBRO_NN_ACCUMULATOR_OVERFLOW = 4,
    NOBRO_NN_UNSUPPORTED = 5,
    NOBRO_NN_PROVIDER_FAILURE = 6
} nobro_nn_status_t;

typedef uint8_t (*nobro_nn_read_u8_fn)(const void *context, size_t index)
    NOBRO_NN_REENTRANT;

typedef struct {
    const void *context;
    size_t length;
    nobro_nn_read_u8_fn read;
} nobro_nn_packed_blob_t;

typedef struct {
    int32_t input_offset;
    int32_t output_offset;
    int32_t multiplier;
    int8_t shift;
    int8_t activation_min;
    int8_t activation_max;
} nobro_nn_packed_quantization_t;

typedef struct {
    uint8_t format_bits;
    uint16_t input_len;
    uint16_t output_len;
    uint32_t logical_weights;
    uint32_t packed_bytes;
} nobro_nn_packed_receipt_t;

static inline uint8_t nobro_nn_read_contiguous(const void *context, size_t index)
    NOBRO_NN_REENTRANT {
    return ((const uint8_t *)context)[index];
}

static inline nobro_nn_packed_blob_t nobro_nn_contiguous_blob(
    const uint8_t *data, size_t length) {
    nobro_nn_packed_blob_t blob;
    blob.context = data;
    blob.length = length;
    blob.read = nobro_nn_read_contiguous;
    return blob;
}

static inline int8_t nobro_nn_sign_extend(uint8_t value, uint8_t bits) {
    uint8_t sign = (uint8_t)(1u << (bits - 1u));
    uint8_t mask = (uint8_t)((1u << bits) - 1u);
    value &= mask;
    return (int8_t)((value ^ sign) - sign);
}

static inline int32_t nobro_nn_arshift_i32(int32_t value, uint8_t bits) {
    if (bits == 0u) {
        return value;
    }
    return value >= 0 ? value >> bits : -1 - ((-1 - value) >> bits);
}

static inline int64_t nobro_nn_arshift_i64(int64_t value, uint8_t bits) {
    if (bits == 0u) {
        return value;
    }
    return value >= 0 ? value >> bits : -1 - ((-1 - value) >> bits);
}

static inline int32_t nobro_nn_requantize(int32_t value, int32_t multiplier,
                                   int8_t shift) {
    uint8_t left = shift > 0 ? (uint8_t)shift : 0u;
    uint8_t right = shift < 0 ? (uint8_t)(-shift) : 0u;
    uint32_t shifted_bits = (uint32_t)value * ((uint32_t)1u << left);
    int32_t shifted = shifted_bits <= (uint32_t)INT32_MAX
                          ? (int32_t)shifted_bits
                          : -1 - (int32_t)(UINT32_MAX - shifted_bits);
    int32_t high = (int32_t)nobro_nn_arshift_i64(
        ((int64_t)1 << 30) + shifted * (int64_t)multiplier, 31u);
    if (right != 0u) {
        int32_t mask = ((int32_t)1 << right) - 1;
        int32_t result = nobro_nn_arshift_i32(high, right);
        int32_t remainder = (int32_t)((int64_t)high -
                                      (int64_t)result * ((int64_t)1 << right));
        int32_t threshold = (mask >> 1) + (result < 0 ? 1 : 0);
        if (remainder > threshold) {
            ++result;
        }
        return result;
    }
    return high;
}

static inline nobro_nn_status_t nobro_nn_packed_dense(
    uint8_t format_bits, const int8_t *input, uint16_t input_len,
    nobro_nn_packed_blob_t weights, const int32_t *bias, uint16_t output_len,
    nobro_nn_packed_quantization_t quantization, int8_t *output,
    nobro_nn_packed_receipt_t *receipt) {
    uint8_t bits = format_bits;
    uint8_t lanes;
    size_t row_bytes;
    size_t expected;
    uint16_t row;
    if (bits != 4u && bits != 2u) {
        return NOBRO_NN_INVALID_SHAPE;
    }
    if (input == NULL || bias == NULL || output == NULL || weights.read == NULL) {
        return NOBRO_NN_INVALID_SHAPE;
    }
    if (input_len == 0u || output_len == 0u) {
        return NOBRO_NN_INVALID_SHAPE;
    }
    if (quantization.input_offset < -127 || quantization.input_offset > 128 ||
        quantization.output_offset < -127 || quantization.output_offset > 128 ||
        quantization.multiplier <= 0 || quantization.shift < -31 ||
        quantization.shift > 30 ||
        quantization.activation_min > quantization.activation_max) {
        return NOBRO_NN_INVALID_QUANTIZATION;
    }
    lanes = (uint8_t)(8u / bits);
    row_bytes = ((size_t)input_len + lanes - 1u) / lanes;
    if (row_bytes != 0u && (size_t)output_len > SIZE_MAX / row_bytes) {
        return NOBRO_NN_INVALID_SHAPE;
    }
    expected = row_bytes * output_len;
    if (weights.length != expected) {
        return NOBRO_NN_INVALID_PACKED_LENGTH;
    }
    for (row = 0u; row < output_len; ++row) {
        int32_t accumulator = bias[row];
        uint16_t column;
        for (column = 0u; column < input_len; ++column) {
            size_t byte_index = (size_t)row * row_bytes + column / lanes;
            uint8_t packed = weights.read(weights.context, byte_index);
            uint8_t lane_shift = (uint8_t)((column % lanes) * bits);
            int8_t weight = nobro_nn_sign_extend((uint8_t)(packed >> lane_shift), bits);
            int32_t adjusted = (int32_t)input[column] + quantization.input_offset;
            int32_t product = adjusted * (int32_t)weight;
            if ((product > 0 && accumulator > INT32_MAX - product) ||
                (product < 0 && accumulator < INT32_MIN - product)) {
                return NOBRO_NN_ACCUMULATOR_OVERFLOW;
            }
            accumulator += product;
        }
        {
            int64_t shifted_output =
                (int64_t)nobro_nn_requantize(accumulator, quantization.multiplier,
                                             quantization.shift) +
                quantization.output_offset;
            if (shifted_output > INT32_MAX || shifted_output < INT32_MIN) {
                return NOBRO_NN_ACCUMULATOR_OVERFLOW;
            }
            accumulator = (int32_t)shifted_output;
        }
        if (accumulator < quantization.activation_min) {
            accumulator = quantization.activation_min;
        } else if (accumulator > quantization.activation_max) {
            accumulator = quantization.activation_max;
        }
        output[row] = (int8_t)accumulator;
    }
    if (receipt != NULL) {
        receipt->format_bits = bits;
        receipt->input_len = input_len;
        receipt->output_len = output_len;
        receipt->logical_weights = (uint32_t)input_len * output_len;
        receipt->packed_bytes = (uint32_t)expected;
    }
    return NOBRO_NN_OK;
}

#ifdef __cplusplus
}
#endif

#undef NOBRO_NN_REENTRANT

#endif
