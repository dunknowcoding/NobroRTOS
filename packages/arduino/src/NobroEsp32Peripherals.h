#ifndef NOBRO_ESP32_PERIPHERALS_H
#define NOBRO_ESP32_PERIPHERALS_H

#include <stddef.h>
#include <stdint.h>
#include "nobro_adc_dma.h"
#include "nobro_pulse.h"

#if !defined(NOBRO_ESP32_PERIPHERALS_DISABLED)

#if !defined(NOBRO_ESP32_PERIPHERALS_TEST)
#if !defined(ARDUINO_ARCH_ESP32)
#error "NobroEsp32Peripherals.h requires an ESP32 Arduino core"
#endif
#include <Arduino.h>
#include "esp32-hal-adc.h"
#include "esp32-hal-ledc.h"
#include "esp32-hal-rmt.h"
#endif

namespace nobro {

namespace detail {
inline uint32_t timeoutMs(uint32_t max_block_us) {
    return max_block_us / 1000u + (max_block_us % 1000u != 0u ? 1u : 0u);
}

inline bool elapsedOverBudget(uint32_t started, uint32_t max_block_us) {
    return max_block_us != 0u &&
           static_cast<uint32_t>(micros() - started) > max_block_us;
}
}  // namespace detail

// Arduino-ESP32 exposes one process-wide continuous ADC instance. This class
// therefore represents an exclusive provider. The wrapper owns no heap, but
// the vendor core allocates its DMA/result storage; that reservation must be
// measured at an exact board binding and must never be reported as zero merely
// because this object is statically bounded.
template <size_t MaxPins>
class Esp32ContinuousAdc {
public:
    Esp32ContinuousAdc()
        : pin_count_(0), configured_(false), running_(false),
          state_(NOBRO_ADC_DMA_DOWN), diagnostics_{} {}

    nobro_adc_dma_error_t configure(const uint8_t *pins,
                                    const nobro_adc_dma_config_t &config) {
        if (pins == nullptr || config.channels == 0 ||
            config.channels > MaxPins || config.resolution_bits < 9 ||
            config.resolution_bits > 12 ||
            config.conversions_per_channel == 0 ||
            config.sample_rate_hz == 0 || running_) {
            return NOBRO_ADC_DMA_INVALID_CONFIG;
        }
        if (configured_ && !analogContinuousDeinit()) return transportFault();
        for (size_t index = 0; index < config.channels; ++index)
            pins_[index] = pins[index];
        config_ = config;
        pin_count_ = config.channels;
        analogContinuousSetWidth(config.resolution_bits);
        if (!analogContinuous(pins_, pin_count_,
                              config.conversions_per_channel,
                              config.sample_rate_hz, nullptr)) {
            configured_ = false;
            return transportFault();
        }
        configured_ = true;
        state_ = NOBRO_ADC_DMA_READY;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t start() {
        if (!configured_ || state_ != NOBRO_ADC_DMA_READY)
            return NOBRO_ADC_DMA_NOT_READY;
        if (!analogContinuousStart()) return transportFault();
        running_ = true;
        state_ = NOBRO_ADC_DMA_RUNNING;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t readFrame(nobro_adc_sample_t *output,
                                    size_t capacity,
                                    uint32_t max_block_us,
                                    size_t &count) {
        count = 0;
        if (!running_ || state_ != NOBRO_ADC_DMA_RUNNING)
            return NOBRO_ADC_DMA_NOT_READY;
        if (output == nullptr || capacity < pin_count_)
            return NOBRO_ADC_DMA_OUTPUT_TOO_SMALL;
        adc_continuous_result_t *vendor = nullptr;
        const uint32_t started = micros();
        if (!analogContinuousRead(&vendor, detail::timeoutMs(max_block_us)) ||
            vendor == nullptr) {
            ++diagnostics_.deadline_misses;
            state_ = NOBRO_ADC_DMA_FAULTED;
            running_ = false;
            return NOBRO_ADC_DMA_DEADLINE_MISS;
        }
        for (size_t index = 0; index < pin_count_; ++index) {
            if (vendor[index].avg_read_raw < 0 ||
                vendor[index].avg_read_raw > 0xffff ||
                vendor[index].avg_read_mvolts < 0 ||
                vendor[index].avg_read_mvolts > 0xffff) {
                ++diagnostics_.partial_frames;
                state_ = NOBRO_ADC_DMA_FAULTED;
                running_ = false;
                return NOBRO_ADC_DMA_PARTIAL_FRAME;
            }
            output[index].channel = vendor[index].pin;
            output[index].raw =
                static_cast<uint16_t>(vendor[index].avg_read_raw);
            output[index].millivolts =
                static_cast<uint16_t>(vendor[index].avg_read_mvolts);
        }
        count = pin_count_;
        ++diagnostics_.frames;
        diagnostics_.samples += static_cast<uint32_t>(count);
        if (detail::elapsedOverBudget(started, max_block_us)) {
            ++diagnostics_.deadline_misses;
            return NOBRO_ADC_DMA_DEADLINE_MISS;
        }
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t quiesce() {
        if (running_ && !analogContinuousStop()) return transportFault();
        running_ = false;
        if (configured_) state_ = NOBRO_ADC_DMA_SUSPENDED;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t recover() {
        if (!configured_) return NOBRO_ADC_DMA_NOT_READY;
        (void)analogContinuousStop();
        if (!analogContinuousDeinit()) return transportFault();
        analogContinuousSetWidth(config_.resolution_bits);
        if (!analogContinuous(pins_, pin_count_,
                              config_.conversions_per_channel,
                              config_.sample_rate_hz, nullptr) ||
            !analogContinuousStart()) {
            return transportFault();
        }
        running_ = true;
        state_ = NOBRO_ADC_DMA_RUNNING;
        ++diagnostics_.recoveries;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_state_t state() const { return state_; }
    nobro_adc_dma_diagnostics_t diagnostics() const { return diagnostics_; }
    static constexpr size_t staticRamBytes() {
        return sizeof(Esp32ContinuousAdc<MaxPins>);
    }

private:
    nobro_adc_dma_error_t transportFault() {
        ++diagnostics_.transport_errors;
        running_ = false;
        state_ = NOBRO_ADC_DMA_FAULTED;
        return NOBRO_ADC_DMA_TRANSPORT;
    }

    uint8_t pins_[MaxPins == 0 ? 1 : MaxPins];
    size_t pin_count_;
    nobro_adc_dma_config_t config_;
    bool configured_;
    bool running_;
    nobro_adc_dma_state_t state_;
    nobro_adc_dma_diagnostics_t diagnostics_;
};

class Esp32LedcPwm {
public:
    explicit Esp32LedcPwm(uint8_t pin)
        : pin_(pin), configured_(false), state_(NOBRO_PULSE_DOWN),
          diagnostics_{} {}

    nobro_pulse_error_t configure(const nobro_pwm_config_t &config) {
        if (config.frequency_hz == 0 || config.resolution_bits == 0 ||
            config.resolution_bits > 20) {
            return NOBRO_PULSE_INVALID_CONFIG;
        }
        if (configured_) (void)ledcDetach(pin_);
        if (!ledcAttach(pin_, config.frequency_hz, config.resolution_bits))
            return transportFault();
        config_ = config;
        configured_ = true;
        state_ = NOBRO_PULSE_READY;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t setDuty(uint32_t duty) {
        if (!configured_ || state_ != NOBRO_PULSE_READY)
            return NOBRO_PULSE_NOT_READY;
        const uint32_t max_duty =
            config_.resolution_bits >= 31
                ? 0x7fffffffu
                : ((1u << config_.resolution_bits) - 1u);
        if (duty > max_duty) return NOBRO_PULSE_INVALID_CONFIG;
        if (!ledcWrite(pin_, duty)) return transportFault();
        ++diagnostics_.writes;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t quiesce() {
        if (configured_ && !ledcDetach(pin_)) return transportFault();
        if (configured_) state_ = NOBRO_PULSE_SUSPENDED;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t recover() {
        if (!configured_) return NOBRO_PULSE_NOT_READY;
        (void)ledcDetach(pin_);
        if (!ledcAttach(pin_, config_.frequency_hz, config_.resolution_bits))
            return transportFault();
        state_ = NOBRO_PULSE_READY;
        ++diagnostics_.recoveries;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_state_t state() const { return state_; }
    nobro_pulse_diagnostics_t diagnostics() const { return diagnostics_; }
    static constexpr size_t staticRamBytes() { return sizeof(Esp32LedcPwm); }

private:
    nobro_pulse_error_t transportFault() {
        ++diagnostics_.transport_errors;
        state_ = NOBRO_PULSE_FAULTED;
        return NOBRO_PULSE_TRANSPORT;
    }

    uint8_t pin_;
    nobro_pwm_config_t config_;
    bool configured_;
    nobro_pulse_state_t state_;
    nobro_pulse_diagnostics_t diagnostics_;
};

template <size_t MaxSymbols>
class Esp32RmtPulse {
public:
    explicit Esp32RmtPulse(uint8_t pin)
        : pin_(pin), tick_hz_(0), configured_(false),
          state_(NOBRO_PULSE_DOWN), diagnostics_{} {}

    nobro_pulse_error_t configure(uint32_t tick_hz) {
        if (tick_hz == 0 || MaxSymbols == 0)
            return NOBRO_PULSE_INVALID_CONFIG;
        if (configured_) (void)rmtDeinit(pin_);
        if (!rmtInit(pin_, RMT_TX_MODE, RMT_MEM_NUM_BLOCKS_1, tick_hz))
            return transportFault();
        tick_hz_ = tick_hz;
        configured_ = true;
        state_ = NOBRO_PULSE_READY;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t transmit(const nobro_pulse_symbol_t *symbols,
                                 size_t count,
                                 uint32_t max_block_us) {
        if (!configured_ || state_ != NOBRO_PULSE_READY)
            return NOBRO_PULSE_NOT_READY;
        if (symbols == nullptr || count == 0 || count > MaxSymbols) {
            ++diagnostics_.oversized_rejections;
            return NOBRO_PULSE_TOO_MANY_SYMBOLS;
        }
        for (size_t index = 0; index < count; ++index) {
            if ((symbols[index].high_ticks == 0 &&
                 symbols[index].low_ticks == 0) ||
                symbols[index].high_ticks > 0x7fffu ||
                symbols[index].low_ticks > 0x7fffu) {
                ++diagnostics_.oversized_rejections;
                return NOBRO_PULSE_TOO_MANY_SYMBOLS;
            }
            storage_[index].duration0 = symbols[index].high_ticks;
            storage_[index].level0 = 1;
            storage_[index].duration1 = symbols[index].low_ticks;
            storage_[index].level1 = 0;
        }
        state_ = NOBRO_PULSE_BUSY;
        const uint32_t started = micros();
        if (!rmtWrite(pin_, storage_, count, detail::timeoutMs(max_block_us))) {
            ++diagnostics_.deadline_misses;
            state_ = NOBRO_PULSE_FAULTED;
            return NOBRO_PULSE_DEADLINE_MISS;
        }
        state_ = NOBRO_PULSE_READY;
        ++diagnostics_.writes;
        diagnostics_.symbols += static_cast<uint32_t>(count);
        if (detail::elapsedOverBudget(started, max_block_us)) {
            ++diagnostics_.deadline_misses;
            return NOBRO_PULSE_DEADLINE_MISS;
        }
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t quiesce() {
        if (configured_ && !rmtDeinit(pin_)) return transportFault();
        if (configured_) state_ = NOBRO_PULSE_SUSPENDED;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t recover() {
        if (!configured_) return NOBRO_PULSE_NOT_READY;
        if (!rmtDeinit(pin_)) return transportFault();
        if (!rmtInit(pin_, RMT_TX_MODE, RMT_MEM_NUM_BLOCKS_1, tick_hz_))
            return transportFault();
        state_ = NOBRO_PULSE_READY;
        ++diagnostics_.recoveries;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_state_t state() const { return state_; }
    nobro_pulse_diagnostics_t diagnostics() const { return diagnostics_; }
    static constexpr size_t staticRamBytes() {
        return sizeof(Esp32RmtPulse<MaxSymbols>);
    }

private:
    nobro_pulse_error_t transportFault() {
        ++diagnostics_.transport_errors;
        state_ = NOBRO_PULSE_FAULTED;
        return NOBRO_PULSE_TRANSPORT;
    }

    uint8_t pin_;
    uint32_t tick_hz_;
    bool configured_;
    nobro_pulse_state_t state_;
    rmt_data_t storage_[MaxSymbols == 0 ? 1 : MaxSymbols];
    nobro_pulse_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif  // !NOBRO_ESP32_PERIPHERALS_DISABLED
#endif  // NOBRO_ESP32_PERIPHERALS_H
