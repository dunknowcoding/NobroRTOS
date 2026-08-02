#ifndef NOBRO_INA_SERIES_H
#define NOBRO_INA_SERIES_H

#include <Arduino.h>
#include <INA_Series_Sensor.h>
#include <limits.h>
#include "nobro_power_monitor.h"

namespace nobro {

template <typename Device>
class InaSeriesAdapter {
public:
    InaSeriesAdapter(Device &device, int sda, int scl, uint32_t hz = 400000)
        : device_(device), sda_(sda), scl_(scl), hz_(hz),
          state_(NOBRO_POWER_MONITOR_DOWN), sequence_(0) {}

    nobro_power_monitor_result_t begin() {
        if (state_ != NOBRO_POWER_MONITOR_DOWN)
            return NOBRO_POWER_MONITOR_INVALID_CONFIG;
        device_.begin(sda_, scl_, hz_);
        state_ = NOBRO_POWER_MONITOR_READY;
        return NOBRO_POWER_MONITOR_OK;
    }

    nobro_power_monitor_result_t sample(nobro_power_channel_sample_t &out,
                                        uint64_t deadline_us = 0) {
        if (state_ != NOBRO_POWER_MONITOR_READY)
            return NOBRO_POWER_MONITOR_NOT_READY;
        const uint32_t started = micros();
        if (deadlineReached(deadline_us, started))
            return NOBRO_POWER_MONITOR_DEADLINE_MISS;
        const float voltage = device_.readBusVoltage();
        const float shunt = device_.readShuntVoltage();
        const float current = device_.readCurrent();
        const float power = device_.readPower();
        if (!finite(voltage) || !finite(shunt) || !finite(current) ||
            !finite(power)) {
            state_ = NOBRO_POWER_MONITOR_FAULTED;
            return NOBRO_POWER_MONITOR_TRANSPORT_ERROR;
        }
        fill(out, 1, voltage, current, power, started);
        out.shunt_uv = scaled(shunt, 1000000.0f);
        return deadlineReached(deadline_us, micros())
                   ? NOBRO_POWER_MONITOR_DEADLINE_MISS
                   : NOBRO_POWER_MONITOR_OK;
    }

    void quiesce() { state_ = NOBRO_POWER_MONITOR_SUSPENDED; }
    nobro_power_monitor_result_t recover() {
        device_.begin(sda_, scl_, hz_);
        state_ = NOBRO_POWER_MONITOR_READY;
        return NOBRO_POWER_MONITOR_OK;
    }
    void release() { state_ = NOBRO_POWER_MONITOR_DOWN; }
    nobro_power_monitor_state_t state() const { return state_; }

protected:
    static bool finite(float value) {
        return value == value && value < 3.4028234e38F && value > -3.4028234e38F;
    }
    // Arduino micros() wraps at 32 bits. Deadlines are therefore valid only
    // within the usual signed half-range, but remain correct across wrap.
    static bool deadlineReached(uint64_t deadline_us, uint32_t now) {
        if (!deadline_us) return false;
        return static_cast<int32_t>(now - static_cast<uint32_t>(deadline_us)) >= 0;
    }
    static int64_t scaled(float value, float scale) {
        const double result = static_cast<double>(value) * scale;
        if (result >= static_cast<double>(INT64_MAX)) return INT64_MAX;
        if (result <= static_cast<double>(INT64_MIN)) return INT64_MIN;
        return static_cast<int64_t>(result);
    }
    void fill(nobro_power_channel_sample_t &out, uint8_t channel,
              float voltage, float current, float power, uint64_t timestamp) {
        out = {};
        out.channel = channel;
        out.bus_uv = scaled(voltage, 1000000.0f);
        out.current_ua = scaled(current, 1000000.0f);
        out.power_uw = scaled(power, 1000000.0f);
        ++sequence_;
        if (!sequence_) ++sequence_;
        out.sequence = sequence_;
        out.timestamp_us = timestamp;
    }

    Device &device_;
    int sda_, scl_;
    uint32_t hz_;
    nobro_power_monitor_state_t state_;
    uint32_t sequence_;
};

// The SPI INA229 uses begin(spi_hz), unlike the I2C bridge family. Keeping a
// separate facade avoids template guesses and makes the selected bus explicit.
template <typename Device>
class InaSeriesSpiAdapter : public InaSeriesAdapter<Device> {
public:
    InaSeriesSpiAdapter(Device &device, uint32_t hz = 10000000)
        : InaSeriesAdapter<Device>(device, 0, 0, hz), spi_hz_(hz) {}

    nobro_power_monitor_result_t begin() {
        if (this->state_ != NOBRO_POWER_MONITOR_DOWN)
            return NOBRO_POWER_MONITOR_INVALID_CONFIG;
        this->device_.begin(spi_hz_);
        this->state_ = NOBRO_POWER_MONITOR_READY;
        return NOBRO_POWER_MONITOR_OK;
    }

    nobro_power_monitor_result_t recover() {
        this->device_.begin(spi_hz_);
        this->state_ = NOBRO_POWER_MONITOR_READY;
        return NOBRO_POWER_MONITOR_OK;
    }

private:
    uint32_t spi_hz_;
};

class Ina3221Adapter : public InaSeriesAdapter<InaBridge3221> {
public:
    Ina3221Adapter(InaBridge3221 &device, int sda, int scl,
                   uint32_t hz = 400000)
        : InaSeriesAdapter<InaBridge3221>(device, sda, scl, hz) {}

    nobro_power_monitor_result_t sampleChannel(
        uint8_t channel, nobro_power_channel_sample_t &out,
        uint64_t deadline_us = 0) {
        if (state_ != NOBRO_POWER_MONITOR_READY)
            return NOBRO_POWER_MONITOR_NOT_READY;
        if (channel < 1 || channel > 3)
            return NOBRO_POWER_MONITOR_INVALID_CHANNEL;
        const uint32_t started = micros();
        if (deadlineReached(deadline_us, started))
            return NOBRO_POWER_MONITOR_DEADLINE_MISS;
        const float voltage = device_.readBusVoltage(channel);
        const float shunt = device_.readShuntVoltage(channel);
        const float current = device_.readCurrent(channel);
        const float power = device_.readPower(channel);
        if (!finite(voltage) || !finite(shunt) || !finite(current) ||
            !finite(power)) {
            state_ = NOBRO_POWER_MONITOR_FAULTED;
            return NOBRO_POWER_MONITOR_TRANSPORT_ERROR;
        }
        fill(out, channel, voltage, current, power, started);
        out.shunt_uv = scaled(shunt, 1000000.0f);
        return deadlineReached(deadline_us, micros())
                   ? NOBRO_POWER_MONITOR_DEADLINE_MISS
                   : NOBRO_POWER_MONITOR_OK;
    }

    nobro_power_monitor_result_t sampleAll(
        nobro_power_channel_sample_t *out, size_t capacity, size_t &written,
        uint64_t deadline_us = 0) {
        written = 0;
        if (!out || capacity < 3) return NOBRO_POWER_MONITOR_INVALID_CONFIG;
        for (uint8_t channel = 1; channel <= 3; ++channel) {
            const nobro_power_monitor_result_t result =
                sampleChannel(channel, out[written], deadline_us);
            if (result != NOBRO_POWER_MONITOR_OK) return result;
            ++written;
        }
        return NOBRO_POWER_MONITOR_OK;
    }
};

}  // namespace nobro
#endif
