#ifndef NOBRO_ROBO_SERVO_H
#define NOBRO_ROBO_SERVO_H

#include <RoboServo.h>
#include "nobro_servo.h"

namespace nobro {

// One caller-owned RoboServo instance behind the portable Nobro lifecycle.
// RoboServo owns the board-specific PWM implementation; Nobro adds bounded
// validation, deadline admission, explicit suspension, recovery, and receipts.
class RoboServoAdapter {
public:
    explicit RoboServoAdapter(RoboServo &servo)
        : servo_(servo), min_us_(0), max_us_(0), last_us_(0), sequence_(1),
          state_(NOBRO_SERVO_DOWN) {}

    nobro_servo_result_t begin(int pin, int min_us = 500, int max_us = 2500,
                               int frequency_hz = 50) {
        if (state_ != NOBRO_SERVO_DOWN || min_us <= 0 || min_us > max_us ||
            frequency_hz < ROBOSERVO_MIN_FREQUENCY ||
            frequency_hz > ROBOSERVO_MAX_FREQUENCY) {
            return NOBRO_SERVO_INVALID_CONFIG;
        }
        if (servo_.attach(pin, min_us, max_us, SERVO_TYPE_180, frequency_hz) ==
            ROBOSERVO_INVALID_SERVO) {
            state_ = NOBRO_SERVO_FAULTED;
            return NOBRO_SERVO_TRANSPORT_ERROR;
        }
        min_us_ = static_cast<uint32_t>(min_us);
        max_us_ = static_cast<uint32_t>(max_us);
        last_us_ = static_cast<uint32_t>((min_us + max_us) / 2);
        servo_.writeMicroseconds(static_cast<int>(last_us_));
        state_ = NOBRO_SERVO_READY;
        return NOBRO_SERVO_OK;
    }

    nobro_servo_result_t command(uint8_t channel, uint32_t pulse_us,
                                 uint32_t now_us, uint32_t deadline_us,
                                 nobro_servo_receipt_t *receipt) {
        if (state_ != NOBRO_SERVO_READY) return NOBRO_SERVO_NOT_READY;
        if (channel != 0 || pulse_us < min_us_ || pulse_us > max_us_ ||
            receipt == nullptr) {
            return NOBRO_SERVO_INVALID_COMMAND;
        }
        if (static_cast<int32_t>(now_us - deadline_us) > 0) {
            return NOBRO_SERVO_DEADLINE_MISS;
        }
        servo_.writeMicroseconds(static_cast<int>(pulse_us));
        if (!servo_.attached() || servo_.readMicroseconds() != static_cast<int>(pulse_us)) {
            state_ = NOBRO_SERVO_FAULTED;
            return NOBRO_SERVO_TRANSPORT_ERROR;
        }
        last_us_ = pulse_us;
        receipt->sequence = sequence_;
        receipt->channel = channel;
        receipt->pulse_us = pulse_us;
        receipt->submitted_us = now_us;
        receipt->deadline_us = deadline_us;
        receipt->completed_us = micros();
        sequence_ = sequence_ == UINT32_MAX ? 1U : sequence_ + 1U;
        return NOBRO_SERVO_OK;
    }

    void quiesce() {
        if (state_ == NOBRO_SERVO_READY) {
            servo_.release();
            state_ = NOBRO_SERVO_SUSPENDED;
        }
    }

    nobro_servo_result_t recover() {
        if (state_ != NOBRO_SERVO_SUSPENDED || !servo_.attached()) {
            return NOBRO_SERVO_NOT_READY;
        }
        servo_.writeMicroseconds(static_cast<int>(last_us_));
        if (servo_.readMicroseconds() != static_cast<int>(last_us_)) {
            state_ = NOBRO_SERVO_FAULTED;
            return NOBRO_SERVO_TRANSPORT_ERROR;
        }
        state_ = NOBRO_SERVO_READY;
        return NOBRO_SERVO_OK;
    }

    void release() {
        servo_.detach();
        min_us_ = max_us_ = last_us_ = 0;
        state_ = NOBRO_SERVO_DOWN;
    }

    nobro_servo_state_t state() const { return state_; }
    uint32_t lastPulseUs() const { return last_us_; }

private:
    RoboServo &servo_;
    uint32_t min_us_;
    uint32_t max_us_;
    uint32_t last_us_;
    uint32_t sequence_;
    nobro_servo_state_t state_;
};

}  // namespace nobro

#endif
