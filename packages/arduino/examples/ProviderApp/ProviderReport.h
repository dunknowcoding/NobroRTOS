#ifndef NOBRO_PROVIDER_REPORT_H
#define NOBRO_PROVIDER_REPORT_H

// Allocation-free record builder for the ProviderApp example. A record is
// assembled completely before the first byte is exposed to Stream, then an
// exact cursor is retained until every suffix byte has been accepted.
class ProviderReport {
public:
    ProviderReport() : length_(0), cursor_(0) {}

    bool begin(bool deadline_armed, bool adc_ok, bool pwm_ok,
               bool i2c_exercised, bool i2c_ok, bool cycle_ok) {
        if (pending()) return false;
        length_ = 0;
        cursor_ = 0;

        const bool complete =
            append("NOBRO-ARDUINO deadline=") &&
            append(deadline_armed ? "armed" : "error") &&
            append(" adc=") && append(adc_ok ? "sampled" : "error") &&
            append(" pwm=") &&
            append(pwm_ok ? "duty_requested" : "error") &&
            append(" i2c=") &&
            append(!i2c_exercised ? "not_exercised"
                                  : (i2c_ok ? "acknowledged" : "error")) &&
            append(" rfid=compile_only result=") &&
            append(cycle_ok ? "ok" : "error") && append("\r\n");
        if (!complete) {
            length_ = 0;
            cursor_ = 0;
        }
        return complete;
    }

    size_t resume(nobro::ArduinoByteIo &output) {
        const size_t unsent = remaining();
        if (unsent == 0) return 0;
        const size_t written = output.writeAll(
            reinterpret_cast<const uint8_t *>(line_ + cursor_), unsent);
        // ArduinoByteIo never reports more than the requested suffix. Keep the
        // guard local as well so a future sink change cannot advance past EOF.
        if (written <= unsent) cursor_ += written;
        return written;
    }

    bool pending() const { return cursor_ < length_; }
    size_t remaining() const { return length_ - cursor_; }

private:
    enum { CAPACITY = 192 };

    bool append(const char *text) {
        size_t text_length = 0;
        while (text[text_length] != '\0') ++text_length;
        if (text_length > static_cast<size_t>(CAPACITY) - length_) return false;
        for (size_t i = 0; i < text_length; ++i) line_[length_ + i] = text[i];
        length_ += text_length;
        return true;
    }

    char line_[CAPACITY];
    size_t length_;
    size_t cursor_;
};

#endif
