#ifndef NOBRO_DI_FINDERS_H
#define NOBRO_DI_FINDERS_H

#include <Arduino.h>
#include <DiFinders.h>
#include "nobro_sensor_provider.h"

namespace nobro {

inline nobro_sensor_sample_status_t sensorStatus(DiFinders::SensorStatus status) {
    switch (status) {
        case DiFinders::SensorStatus::Ok: return NOBRO_SENSOR_SAMPLE_VALID;
        case DiFinders::SensorStatus::Timeout: return NOBRO_SENSOR_SAMPLE_TIMEOUT;
        case DiFinders::SensorStatus::OutOfRange: return NOBRO_SENSOR_SAMPLE_OUT_OF_RANGE;
        case DiFinders::SensorStatus::NotReady: return NOBRO_SENSOR_SAMPLE_NOT_READY;
        case DiFinders::SensorStatus::Disabled: return NOBRO_SENSOR_SAMPLE_DISABLED;
        default: return NOBRO_SENSOR_SAMPLE_TRANSPORT_ERROR;
    }
}

template <typename Sensor>
class DiFinderRangingAdapter {
public:
    typedef bool (*MountFn)(Sensor &);
    DiFinderRangingAdapter(Sensor &sensor, nobro_sensor_transport_t transport,
                           MountFn mount = nullptr)
        : sensor_(sensor), transport_(transport), mount_(mount),
          state_(NOBRO_SENSOR_PROVIDER_DOWN), sequence_(0) {}

    bool begin() {
        if (state_ != NOBRO_SENSOR_PROVIDER_DOWN) return false;
        if (!mount_ || !mount_(sensor_)) {
            state_ = NOBRO_SENSOR_PROVIDER_FAULTED;
            return false;
        }
        state_ = NOBRO_SENSOR_PROVIDER_READY;
        return true;
    }

    // Explicitly adopt a sensor which the application already initialized.
    bool adoptReady() {
        if (state_ != NOBRO_SENSOR_PROVIDER_DOWN) return false;
        state_ = NOBRO_SENSOR_PROVIDER_READY;
        return true;
    }

    bool sample(nobro_ranging_sample_t &out, uint64_t deadline_us = 0) {
        if (state_ != NOBRO_SENSOR_PROVIDER_READY) return false;
        const uint32_t started = micros();
        if (deadlineReached(deadline_us, started)) return false;
        const DiFinders::RangeReading source = sensor_.read();
        out = {};
        out.distance_mm = source.distanceMm;
        out.raw = source.rawValue;
        out.sequence = nextSequence();
        out.timestamp_us = static_cast<uint64_t>(source.timestampMs) * 1000ULL;
        out.status = sensorStatus(source.status);
        if (deadlineReached(deadline_us, micros())) return false;
        if (out.status == NOBRO_SENSOR_SAMPLE_TRANSPORT_ERROR)
            state_ = NOBRO_SENSOR_PROVIDER_FAULTED;
        return out.status == NOBRO_SENSOR_SAMPLE_VALID;
    }

    void quiesce() { state_ = NOBRO_SENSOR_PROVIDER_SUSPENDED; }
    bool recover() {
        if (!mount_ || !mount_(sensor_)) {
            state_ = NOBRO_SENSOR_PROVIDER_FAULTED;
            return false;
        }
        state_ = NOBRO_SENSOR_PROVIDER_READY;
        return true;
    }
    void release() { state_ = NOBRO_SENSOR_PROVIDER_DOWN; }
    nobro_sensor_provider_state_t state() const { return state_; }
    nobro_sensor_transport_t transport() const { return transport_; }

private:
    static bool deadlineReached(uint64_t deadline_us, uint32_t now) {
        if (!deadline_us) return false;
        return static_cast<int32_t>(now - static_cast<uint32_t>(deadline_us)) >= 0;
    }
    uint32_t nextSequence() { if (++sequence_ == 0) ++sequence_; return sequence_; }
    Sensor &sensor_;
    nobro_sensor_transport_t transport_;
    MountFn mount_;
    nobro_sensor_provider_state_t state_;
    uint32_t sequence_;
};

inline void presenceFields(const DiFinders::ProximityReading &source,
                           nobro_presence_sample_t &out) {
    out.detected = source.detected();
    out.strength_permille = source.strengthPermille;
    out.timestamp_us = static_cast<uint64_t>(source.timestampMs) * 1000ULL;
    out.status = sensorStatus(source.status);
}

inline void presenceFields(const DiFinders::MotionReading &source,
                           nobro_presence_sample_t &out) {
    out.detected = source.detected();
    out.rose = source.rose;
    out.fell = source.fell;
    out.timestamp_us = static_cast<uint64_t>(source.timestampMs) * 1000ULL;
    out.status = sensorStatus(source.status);
}

template <typename Sensor>
class DiFinderPresenceAdapter {
public:
    typedef bool (*MountFn)(Sensor &);
    DiFinderPresenceAdapter(Sensor &sensor, nobro_sensor_transport_t transport,
                            MountFn mount = nullptr)
        : sensor_(sensor), transport_(transport), mount_(mount),
          state_(NOBRO_SENSOR_PROVIDER_DOWN), sequence_(0) {}
    bool begin() {
        if (state_ != NOBRO_SENSOR_PROVIDER_DOWN) return false;
        if (!mount_ || !mount_(sensor_)) {
            state_ = NOBRO_SENSOR_PROVIDER_FAULTED; return false;
        }
        state_ = NOBRO_SENSOR_PROVIDER_READY; return true;
    }
    bool adoptReady() {
        if (state_ != NOBRO_SENSOR_PROVIDER_DOWN) return false;
        state_ = NOBRO_SENSOR_PROVIDER_READY; return true;
    }
    bool sample(nobro_presence_sample_t &out, uint64_t deadline_us = 0) {
        if (state_ != NOBRO_SENSOR_PROVIDER_READY) return false;
        const uint32_t started = micros();
        if (deadlineReached(deadline_us, started)) return false;
        const auto source = sensor_.read();
        out = {};
        presenceFields(source, out);
        out.sequence = nextSequence();
        if (deadlineReached(deadline_us, micros())) return false;
        if (out.status == NOBRO_SENSOR_SAMPLE_TRANSPORT_ERROR)
            state_ = NOBRO_SENSOR_PROVIDER_FAULTED;
        return out.status == NOBRO_SENSOR_SAMPLE_VALID;
    }
    void quiesce() { state_ = NOBRO_SENSOR_PROVIDER_SUSPENDED; }
    bool recover() {
        if (!mount_ || !mount_(sensor_)) {
            state_ = NOBRO_SENSOR_PROVIDER_FAULTED; return false;
        }
        state_ = NOBRO_SENSOR_PROVIDER_READY; return true;
    }
    void release() { state_ = NOBRO_SENSOR_PROVIDER_DOWN; }
    nobro_sensor_provider_state_t state() const { return state_; }
    nobro_sensor_transport_t transport() const { return transport_; }
private:
    static bool deadlineReached(uint64_t deadline_us, uint32_t now) {
        if (!deadline_us) return false;
        return static_cast<int32_t>(now - static_cast<uint32_t>(deadline_us)) >= 0;
    }
    uint32_t nextSequence() { if (++sequence_ == 0) ++sequence_; return sequence_; }
    Sensor &sensor_;
    nobro_sensor_transport_t transport_;
    MountFn mount_;
    nobro_sensor_provider_state_t state_;
    uint32_t sequence_;
};

}  // namespace nobro
#endif
