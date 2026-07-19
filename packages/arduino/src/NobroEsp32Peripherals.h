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
#include "esp32-hal-periman.h"
#include "esp32-hal-rmt.h"
#include "esp_adc/adc_cali_scheme.h"
#include "esp_adc/adc_continuous.h"
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
        : pin_count_(0), configured_(false), started_(false), running_(false),
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
        // Arduino-ESP32 widens DMA frames to its cache-line alignment. Reject
        // a shape that would be silently changed so deadlines and averaging
        // counts retain the caller's exact meaning.
        if (alignedConversionsPerChannel(
                config.channels, config.conversions_per_channel) !=
            config.conversions_per_channel) {
            return NOBRO_ADC_DMA_INVALID_CONFIG;
        }
        if (configured_) {
            if (started_ && !analogContinuousStop()) return transportFault();
            started_ = false;
            if (!analogContinuousDeinit()) return transportFault();
            configured_ = false;
        }
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
        started_ = true;
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
        if (started_ && !analogContinuousStop()) return transportFault();
        started_ = false;
        running_ = false;
        if (configured_) state_ = NOBRO_ADC_DMA_SUSPENDED;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t recover() {
        if (!configured_) return NOBRO_ADC_DMA_NOT_READY;
        if (started_ && !analogContinuousStop()) return transportFault();
        started_ = false;
        if (!analogContinuousDeinit()) return transportFault();
        configured_ = false;
        analogContinuousSetWidth(config_.resolution_bits);
        if (!analogContinuous(pins_, pin_count_,
                              config_.conversions_per_channel,
                              config_.sample_rate_hz, nullptr)) {
            return transportFault();
        }
        configured_ = true;
        if (!analogContinuousStart()) return transportFault();
        started_ = true;
        running_ = true;
        state_ = NOBRO_ADC_DMA_RUNNING;
        ++diagnostics_.recoveries;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t release() {
        if (started_ && !analogContinuousStop()) return transportFault();
        started_ = false;
        running_ = false;
        if (configured_ && !analogContinuousDeinit()) return transportFault();
        configured_ = false;
        pin_count_ = 0;
        state_ = NOBRO_ADC_DMA_DOWN;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_state_t state() const { return state_; }
    nobro_adc_dma_diagnostics_t diagnostics() const { return diagnostics_; }
    static uint16_t alignedConversionsPerChannel(uint8_t channels,
                                                 uint16_t requested) {
        if (channels == 0 || requested == 0) return 0;
#if defined(ESP_ARDUINO_DMA_BUF_ALIGN)
        const uint32_t alignment = ESP_ARDUINO_DMA_BUF_ALIGN;
#else
        const uint32_t alignment = 4;
#endif
        const uint32_t bytes_per_conversion_round =
            static_cast<uint32_t>(channels) * 4u;
        uint32_t conversions = requested;
        while ((conversions * bytes_per_conversion_round) % alignment != 0u) {
            ++conversions;
            if (conversions > UINT16_MAX) return 0;
        }
        return static_cast<uint16_t>(conversions);
    }
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
    bool started_;
    bool running_;
    nobro_adc_dma_state_t state_;
    nobro_adc_dma_diagnostics_t diagnostics_;
};

#if !defined(NOBRO_ESP32_PERIPHERALS_TEST) && \
    defined(ESP_ARDUINO_VERSION_MAJOR) && ESP_ARDUINO_VERSION_MAJOR >= 3

// A fixed-capacity alternative to Arduino's analogContinuousRead convenience
// path. ESP-IDF reads directly into this object's aligned storage, avoiding
// one heap allocation/free pair per frame. The ordinary Esp32ContinuousAdc
// remains available when minimum flash is more important than runtime cost.
template <size_t MaxPins, size_t MaxConversionsPerChannel>
class Esp32PersistentContinuousAdc {
public:
    static_assert(MaxPins > 0, "persistent ADC requires at least one pin");
    static_assert(MaxConversionsPerChannel > 0,
                  "persistent ADC requires frame storage");
    static_assert(MaxPins * MaxConversionsPerChannel *
                          SOC_ADC_DIGI_RESULT_BYTES <=
                      4092,
                  "persistent ADC frame exceeds the ESP-IDF limit");

    Esp32PersistentContinuousAdc()
        : handle_(nullptr), calibrations_{}, pin_count_(0),
          frame_bytes_(0), configured_(false), started_(false),
          running_(false), releasing_(false), previous_deinit_(nullptr),
          state_(NOBRO_ADC_DMA_DOWN), diagnostics_{} {}

    ~Esp32PersistentContinuousAdc() { (void)release(); }

    Esp32PersistentContinuousAdc(
        const Esp32PersistentContinuousAdc &) = delete;
    Esp32PersistentContinuousAdc &operator=(
        const Esp32PersistentContinuousAdc &) = delete;
    Esp32PersistentContinuousAdc(
        Esp32PersistentContinuousAdc &&) = delete;
    Esp32PersistentContinuousAdc &operator=(
        Esp32PersistentContinuousAdc &&) = delete;

    nobro_adc_dma_error_t configure(const uint8_t *pins,
                                    const nobro_adc_dma_config_t &config) {
        if (pins == nullptr || config.channels == 0 ||
            config.channels > MaxPins || config.resolution_bits < 9 ||
            config.resolution_bits > 12 ||
            config.conversions_per_channel == 0 ||
            config.conversions_per_channel > MaxConversionsPerChannel ||
            config.sample_rate_hz == 0 || running_ ||
            alignedConversionsPerChannel(
                config.channels, config.conversions_per_channel) !=
                config.conversions_per_channel) {
            return NOBRO_ADC_DMA_INVALID_CONFIG;
        }
        if (configured_ || handle_ != nullptr || pin_count_ != 0) {
            const nobro_adc_dma_error_t released = release();
            if (released != NOBRO_ADC_DMA_OK) return released;
        }

        adc_digi_pattern_config_t patterns[MaxPins] = {};
        for (size_t index = 0; index < config.channels; ++index) {
            adc_unit_t unit = ADC_UNIT_1;
            adc_channel_t channel = ADC_CHANNEL_0;
            if (adc_continuous_io_to_channel(
                    pins[index], &unit, &channel) != ESP_OK ||
                unit != ADC_UNIT_1) {
                return NOBRO_ADC_DMA_INVALID_CONFIG;
            }
            pins_[index] = pins[index];
            channels_[index] = channel;
            patterns[index].atten = ADC_ATTEN_DB_12;
            patterns[index].channel = channel;
            patterns[index].unit = ADC_UNIT_1;
            patterns[index].bit_width =
                static_cast<adc_bitwidth_t>(config.resolution_bits);
        }
        pin_count_ = config.channels;
        config_ = config;
        frame_bytes_ =
            static_cast<size_t>(config.channels) *
            config.conversions_per_channel * SOC_ADC_DIGI_RESULT_BYTES;
        if (frame_bytes_ == 0 || frame_bytes_ > sizeof(frame_buffer_)) {
            pin_count_ = 0;
            return NOBRO_ADC_DMA_INVALID_CONFIG;
        }

        // Release only the requested pins through their current owners before
        // installing this provider's deinitializer. A different process-wide
        // continuous ADC instance will make new_handle fail closed.
        for (size_t index = 0; index < pin_count_; ++index) {
            if (!perimanClearPinBus(pins_[index])) {
                pin_count_ = 0;
                return transportFault();
            }
        }

        const adc_continuous_handle_cfg_t handle_config = {
            .max_store_buf_size =
                static_cast<uint32_t>(frame_bytes_ * 2u),
            .conv_frame_size = static_cast<uint32_t>(frame_bytes_),
        };
        if (adc_continuous_new_handle(&handle_config, &handle_) != ESP_OK) {
            pin_count_ = 0;
            return transportFault();
        }
        const adc_continuous_config_t driver_config = {
            .pattern_num = config.channels,
            .adc_pattern = patterns,
            .sample_freq_hz = config.sample_rate_hz,
            .conv_mode = ADC_CONV_SINGLE_UNIT_1,
#if CONFIG_IDF_TARGET_ESP32 || CONFIG_IDF_TARGET_ESP32S2
            .format = ADC_DIGI_OUTPUT_FORMAT_TYPE1,
#else
            .format = ADC_DIGI_OUTPUT_FORMAT_TYPE2,
#endif
        };
        if (adc_continuous_config(handle_, &driver_config) != ESP_OK ||
            !createCalibrations(config.resolution_bits)) {
            (void)cleanupDriver();
            pin_count_ = 0;
            return transportFault();
        }

        previous_deinit_ =
            perimanGetBusDeinit(ESP32_BUS_TYPE_ADC_CONT);
        if (!perimanSetBusDeinit(
                ESP32_BUS_TYPE_ADC_CONT, &detachBus)) {
            (void)cleanupDriver();
            pin_count_ = 0;
            previous_deinit_ = nullptr;
            return transportFault();
        }
        for (size_t index = 0; index < pin_count_; ++index) {
            if (!perimanSetPinBus(
                    pins_[index], ESP32_BUS_TYPE_ADC_CONT, this,
                    static_cast<int8_t>(ADC_UNIT_1),
                    static_cast<int8_t>(channels_[index]))) {
                (void)release();
                return transportFault();
            }
        }

        configured_ = true;
        state_ = NOBRO_ADC_DMA_READY;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t start() {
        if (!configured_ || handle_ == nullptr ||
            state_ != NOBRO_ADC_DMA_READY) {
            return NOBRO_ADC_DMA_NOT_READY;
        }
        if (adc_continuous_start(handle_) != ESP_OK)
            return transportFault();
        started_ = true;
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

        uint32_t sums[MaxPins] = {};
        uint32_t counts[MaxPins] = {};
        uint32_t bytes_read = 0;
        const uint32_t started = micros();
        const esp_err_t error = adc_continuous_read(
            handle_, frame_buffer_, frame_bytes_, &bytes_read,
            detail::timeoutMs(max_block_us));
        if (error == ESP_ERR_TIMEOUT) {
            ++diagnostics_.deadline_misses;
            running_ = false;
            state_ = NOBRO_ADC_DMA_FAULTED;
            return NOBRO_ADC_DMA_DEADLINE_MISS;
        }
        if (error != ESP_OK) return transportFault();
        if (bytes_read != frame_bytes_) return partialFrame();

        for (size_t offset = 0; offset < bytes_read;
             offset += SOC_ADC_DIGI_RESULT_BYTES) {
            const adc_digi_output_data_t *sample =
                reinterpret_cast<const adc_digi_output_data_t *>(
                    frame_buffer_ + offset);
            const uint32_t channel = sampleChannel(sample);
            const uint32_t value = sampleValue(sample);
            if (value >= (1u << SOC_ADC_DIGI_MAX_BITWIDTH))
                return partialFrame();
            bool matched = false;
            for (size_t index = 0; index < pin_count_; ++index) {
                if (channel ==
                    static_cast<uint32_t>(channels_[index])) {
                    sums[index] += value;
                    ++counts[index];
                    matched = true;
                    break;
                }
            }
            if (!matched) return partialFrame();
        }

        for (size_t index = 0; index < pin_count_; ++index) {
            if (counts[index] != config_.conversions_per_channel)
                return partialFrame();
            const uint32_t average = sums[index] / counts[index];
            int millivolts = 0;
            if (average > UINT16_MAX ||
                adc_cali_raw_to_voltage(
                    calibrations_[index], average, &millivolts) != ESP_OK ||
                millivolts < 0 || millivolts > UINT16_MAX) {
                return transportFault();
            }
            output[index].channel = pins_[index];
            output[index].raw = static_cast<uint16_t>(average);
            output[index].millivolts =
                static_cast<uint16_t>(millivolts);
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
        if (started_) {
            if (adc_continuous_stop(handle_) != ESP_OK)
                return transportFault();
            started_ = false;
        }
        running_ = false;
        if (configured_) state_ = NOBRO_ADC_DMA_SUSPENDED;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t recover() {
        if (!configured_ || pin_count_ == 0)
            return NOBRO_ADC_DMA_NOT_READY;
        const nobro_adc_dma_config_t saved_config = config_;
        const size_t saved_pin_count = pin_count_;
        uint8_t saved_pins[MaxPins] = {};
        for (size_t index = 0; index < saved_pin_count; ++index)
            saved_pins[index] = pins_[index];
        const nobro_adc_dma_error_t released = release();
        if (released != NOBRO_ADC_DMA_OK) return released;
        const nobro_adc_dma_error_t configured =
            configure(saved_pins, saved_config);
        if (configured != NOBRO_ADC_DMA_OK) return configured;
        const nobro_adc_dma_error_t started = start();
        if (started != NOBRO_ADC_DMA_OK) return started;
        ++diagnostics_.recoveries;
        return NOBRO_ADC_DMA_OK;
    }

    nobro_adc_dma_error_t release() {
        bool pass = true;
        releasing_ = true;
        if (started_ && handle_ != nullptr) {
            pass =
                adc_continuous_stop(handle_) == ESP_OK && pass;
        }
        started_ = false;
        running_ = false;
        pass = cleanupDriver() && pass;
        for (size_t index = 0; index < pin_count_; ++index) {
            if (perimanGetPinBus(
                    pins_[index], ESP32_BUS_TYPE_ADC_CONT) == this) {
                pass = perimanClearPinBus(pins_[index]) && pass;
            }
        }
        if (perimanGetBusDeinit(ESP32_BUS_TYPE_ADC_CONT) ==
            &detachBus) {
            if (previous_deinit_ != nullptr) {
                pass = perimanSetBusDeinit(
                           ESP32_BUS_TYPE_ADC_CONT,
                           previous_deinit_) &&
                       pass;
            } else {
                pass =
                    perimanClearBusDeinit(
                        ESP32_BUS_TYPE_ADC_CONT) &&
                    pass;
            }
        }
        previous_deinit_ = nullptr;
        releasing_ = false;
        configured_ = false;
        pin_count_ = 0;
        frame_bytes_ = 0;
        state_ = NOBRO_ADC_DMA_DOWN;
        return pass ? NOBRO_ADC_DMA_OK : NOBRO_ADC_DMA_TRANSPORT;
    }

    nobro_adc_dma_state_t state() const { return state_; }
    nobro_adc_dma_diagnostics_t diagnostics() const {
        return diagnostics_;
    }
    static uint16_t alignedConversionsPerChannel(
        uint8_t channels, uint16_t requested) {
        if (channels == 0 || requested == 0) return 0;
        uint32_t conversions = requested;
        while ((conversions * channels *
                SOC_ADC_DIGI_RESULT_BYTES) %
                   ESP_ARDUINO_DMA_BUF_ALIGN !=
               0u) {
            ++conversions;
            if (conversions > UINT16_MAX) return 0;
        }
        return static_cast<uint16_t>(conversions);
    }
    static constexpr size_t persistentBufferBytes() {
        return sizeof(frame_buffer_);
    }
    static constexpr size_t staticRamBytes() {
        return sizeof(Esp32PersistentContinuousAdc<
                      MaxPins, MaxConversionsPerChannel>);
    }

private:
    static bool detachBus(void *bus) {
        if (bus == nullptr) return false;
        return static_cast<Esp32PersistentContinuousAdc *>(bus)
            ->detachByPeripheralManager();
    }

    bool detachByPeripheralManager() {
        bool pass = true;
        if (started_ && handle_ != nullptr)
            pass = adc_continuous_stop(handle_) == ESP_OK;
        started_ = false;
        running_ = false;
        pass = cleanupDriver() && pass;
        configured_ = false;
        if (!releasing_) state_ = NOBRO_ADC_DMA_FAULTED;
        return pass;
    }

    bool cleanupDriver() {
        bool pass = true;
        if (handle_ != nullptr) {
            pass = adc_continuous_deinit(handle_) == ESP_OK;
            handle_ = nullptr;
        }
        for (size_t index = 0; index < MaxPins; ++index) {
            if (calibrations_[index] == nullptr) continue;
#if ADC_CALI_SCHEME_CURVE_FITTING_SUPPORTED
            pass =
                adc_cali_delete_scheme_curve_fitting(
                    calibrations_[index]) == ESP_OK &&
                pass;
#elif ADC_CALI_SCHEME_LINE_FITTING_SUPPORTED
            pass =
                adc_cali_delete_scheme_line_fitting(
                    calibrations_[index]) == ESP_OK &&
                pass;
#endif
            calibrations_[index] = nullptr;
        }
        return pass;
    }

    bool createCalibrations(uint8_t resolution_bits) {
        for (size_t index = 0; index < pin_count_; ++index) {
#if ADC_CALI_SCHEME_CURVE_FITTING_SUPPORTED
            const adc_cali_curve_fitting_config_t config = {
                .unit_id = ADC_UNIT_1,
                .chan = channels_[index],
                .atten = ADC_ATTEN_DB_12,
                .bitwidth =
                    static_cast<adc_bitwidth_t>(resolution_bits),
            };
            if (adc_cali_create_scheme_curve_fitting(
                    &config, &calibrations_[index]) != ESP_OK) {
                return false;
            }
#elif ADC_CALI_SCHEME_LINE_FITTING_SUPPORTED
            const adc_cali_line_fitting_config_t config = {
                .unit_id = ADC_UNIT_1,
                .atten = ADC_ATTEN_DB_12,
                .bitwidth =
                    static_cast<adc_bitwidth_t>(resolution_bits),
                .default_vref = 0,
            };
            if (adc_cali_create_scheme_line_fitting(
                    &config, &calibrations_[index]) != ESP_OK) {
                return false;
            }
#else
            (void)resolution_bits;
            return false;
#endif
        }
        return true;
    }

    static uint32_t sampleChannel(
        const adc_digi_output_data_t *sample) {
#if CONFIG_IDF_TARGET_ESP32 || CONFIG_IDF_TARGET_ESP32S2
        return sample->type1.channel;
#else
        return sample->type2.channel;
#endif
    }

    static uint32_t sampleValue(
        const adc_digi_output_data_t *sample) {
#if CONFIG_IDF_TARGET_ESP32 || CONFIG_IDF_TARGET_ESP32S2
        return sample->type1.data;
#else
        return sample->type2.data;
#endif
    }

    nobro_adc_dma_error_t partialFrame() {
        ++diagnostics_.partial_frames;
        running_ = false;
        state_ = NOBRO_ADC_DMA_FAULTED;
        return NOBRO_ADC_DMA_PARTIAL_FRAME;
    }

    nobro_adc_dma_error_t transportFault() {
        ++diagnostics_.transport_errors;
        running_ = false;
        state_ = NOBRO_ADC_DMA_FAULTED;
        return NOBRO_ADC_DMA_TRANSPORT;
    }

    adc_continuous_handle_t handle_;
    adc_cali_handle_t calibrations_[MaxPins];
    uint8_t pins_[MaxPins];
    adc_channel_t channels_[MaxPins];
    size_t pin_count_;
    size_t frame_bytes_;
    nobro_adc_dma_config_t config_;
    bool configured_;
    bool started_;
    bool running_;
    bool releasing_;
    peripheral_bus_deinit_cb_t previous_deinit_;
    nobro_adc_dma_state_t state_;
    nobro_adc_dma_diagnostics_t diagnostics_;
    alignas(ESP_ARDUINO_DMA_BUF_ALIGN)
        uint8_t frame_buffer_[
            MaxPins * MaxConversionsPerChannel *
            SOC_ADC_DIGI_RESULT_BYTES];
};

#endif

class Esp32LedcPwm {
public:
    explicit Esp32LedcPwm(uint8_t pin)
        : pin_(pin), configured_(false), attached_(false),
          state_(NOBRO_PULSE_DOWN),
          diagnostics_{} {}

    nobro_pulse_error_t configure(const nobro_pwm_config_t &config) {
        if (config.frequency_hz == 0 || config.resolution_bits == 0 ||
            config.resolution_bits > 20) {
            return NOBRO_PULSE_INVALID_CONFIG;
        }
        if (attached_ && !ledcDetach(pin_)) return transportFault();
        attached_ = false;
        if (!ledcAttach(pin_, config.frequency_hz, config.resolution_bits))
            return transportFault();
        config_ = config;
        configured_ = true;
        attached_ = true;
        state_ = NOBRO_PULSE_READY;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t setDuty(uint32_t duty) {
        if (!configured_ || !attached_ || state_ != NOBRO_PULSE_READY)
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
        if (attached_ && !ledcDetach(pin_)) return transportFault();
        attached_ = false;
        if (configured_) state_ = NOBRO_PULSE_SUSPENDED;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t recover() {
        if (!configured_) return NOBRO_PULSE_NOT_READY;
        if (attached_ && !ledcDetach(pin_)) return transportFault();
        attached_ = false;
        if (!ledcAttach(pin_, config_.frequency_hz, config_.resolution_bits))
            return transportFault();
        attached_ = true;
        state_ = NOBRO_PULSE_READY;
        ++diagnostics_.recoveries;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t release() {
        if (attached_ && !ledcDetach(pin_)) return transportFault();
        attached_ = false;
        configured_ = false;
        state_ = NOBRO_PULSE_DOWN;
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
    bool attached_;
    nobro_pulse_state_t state_;
    nobro_pulse_diagnostics_t diagnostics_;
};

template <size_t MaxSymbols>
class Esp32RmtPulse {
public:
    explicit Esp32RmtPulse(uint8_t pin)
        : pin_(pin), tick_hz_(0), configured_(false), attached_(false),
          state_(NOBRO_PULSE_DOWN), diagnostics_{} {}

    nobro_pulse_error_t configure(uint32_t tick_hz) {
        if (tick_hz == 0 || MaxSymbols == 0)
            return NOBRO_PULSE_INVALID_CONFIG;
        if (attached_ && !rmtDeinit(pin_)) return transportFault();
        attached_ = false;
        if (!rmtInit(pin_, RMT_TX_MODE, RMT_MEM_NUM_BLOCKS_1, tick_hz))
            return transportFault();
        tick_hz_ = tick_hz;
        configured_ = true;
        attached_ = true;
        state_ = NOBRO_PULSE_READY;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t transmit(const nobro_pulse_symbol_t *symbols,
                                 size_t count,
                                 uint32_t max_block_us) {
        if (!configured_ || !attached_ || state_ != NOBRO_PULSE_READY)
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
        if (attached_ && !rmtDeinit(pin_)) return transportFault();
        attached_ = false;
        if (configured_) state_ = NOBRO_PULSE_SUSPENDED;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t recover() {
        if (!configured_) return NOBRO_PULSE_NOT_READY;
        if (attached_ && !rmtDeinit(pin_)) return transportFault();
        attached_ = false;
        if (!rmtInit(pin_, RMT_TX_MODE, RMT_MEM_NUM_BLOCKS_1, tick_hz_))
            return transportFault();
        attached_ = true;
        state_ = NOBRO_PULSE_READY;
        ++diagnostics_.recoveries;
        return NOBRO_PULSE_OK;
    }

    nobro_pulse_error_t release() {
        if (attached_ && !rmtDeinit(pin_)) return transportFault();
        attached_ = false;
        configured_ = false;
        tick_hz_ = 0;
        state_ = NOBRO_PULSE_DOWN;
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
    bool attached_;
    nobro_pulse_state_t state_;
    rmt_data_t storage_[MaxSymbols == 0 ? 1 : MaxSymbols];
    nobro_pulse_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif  // !NOBRO_ESP32_PERIPHERALS_DISABLED
#endif  // NOBRO_ESP32_PERIPHERALS_H
