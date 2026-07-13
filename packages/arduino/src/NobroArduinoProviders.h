#ifndef NOBRO_ARDUINO_PROVIDERS_H
#define NOBRO_ARDUINO_PROVIDERS_H

#include <Arduino.h>
#if defined(NOBRO_ARDUINO_ENABLE_SPI)
#include <SPI.h>
#endif
#if defined(NOBRO_ARDUINO_ENABLE_I2C)
#include <Wire.h>
#endif

namespace nobro {

/* Thin allocation-free providers over the selected Arduino board package. The board
 * core owns register setup and pin routing; NobroRTOS supplies bounded, uniform calls. */
struct ArduinoClock {
    static uint32_t nowUs() { return micros(); }
    static bool reached(uint32_t now, uint32_t deadline) {
        return static_cast<int32_t>(now - deadline) >= 0;
    }
};

class ArduinoDeadline {
public:
    ArduinoDeadline() : deadline_us_(0), armed_(false) {}

    bool armAfterUs(uint32_t delay_us) {
        if (delay_us == 0 || delay_us > 0x7ffffffful) return false;
        deadline_us_ = ArduinoClock::nowUs() + delay_us;
        armed_ = true;
        return true;
    }
    bool due() {
        if (!armed_ || !ArduinoClock::reached(ArduinoClock::nowUs(), deadline_us_))
            return false;
        armed_ = false;
        return true;
    }
    void cancel() { armed_ = false; }
    bool armed() const { return armed_; }
    uint32_t deadlineUs() const { return deadline_us_; }

private:
    uint32_t deadline_us_;
    bool armed_;
};

class ArduinoAdc {
public:
    explicit ArduinoAdc(uint8_t pin, uint8_t resolution_bits = 10)
        : pin_(pin), resolution_bits_(resolution_bits), begun_(false) {}

    static bool supportsResolution(uint8_t resolution_bits) {
#if defined(ARDUINO_ARCH_AVR)
        // The classic AVR Arduino API always returns a 10-bit logical sample.
        return resolution_bits == 10;
#elif defined(ARDUINO_ARCH_ESP32)
        // Arduino-ESP32 scales the returned value to a requested width of 1..16.
        return resolution_bits >= 1 && resolution_bits <= 16;
#elif defined(ARDUINO_ARCH_RENESAS)
        // These are the values accepted by the Renesas Arduino core.
        return resolution_bits == 8 || resolution_bits == 10 ||
               resolution_bits == 12 || resolution_bits == 14 ||
               resolution_bits == 16;
#elif defined(ARDUINO_ARCH_NRF52)
        // ArduinoNRF clamps to this range; reject instead of silently clamping.
        return resolution_bits >= 1 && resolution_bits <= 14;
#else
        // An unknown core may not implement analogReadResolution at all. Preserve
        // the portable Arduino 10-bit logical API and reject unsupported claims.
        return resolution_bits == 10;
#endif
    }

    bool begin() {
        if (!supportsResolution(resolution_bits_)) return false;
        pinMode(pin_, INPUT);
#if defined(ARDUINO_ARCH_ESP32) || defined(ARDUINO_ARCH_RENESAS) || \
    defined(ARDUINO_ARCH_NRF52)
        analogReadResolution(resolution_bits_);
#endif
        begun_ = true;
        return true;
    }
    bool read(uint16_t &sample) const {
        if (!begun_) return false;
#if defined(ARDUINO_ARCH_ESP32) || defined(ARDUINO_ARCH_RENESAS) || \
    defined(ARDUINO_ARCH_NRF52)
        // These cores expose process-wide ADC resolution state. Reapply this
        // provider's contract immediately before every sample so interleaved
        // ArduinoAdc instances cannot silently reinterpret each other's data.
        analogReadResolution(resolution_bits_);
#endif
        const int value = analogRead(pin_);
        if (value < 0 || static_cast<uint32_t>(value) > maxSample()) return false;
        sample = static_cast<uint16_t>(value);
        return true;
    }
    uint16_t read() const {
        uint16_t sample = 0;
        (void)read(sample);
        return sample;
    }
    bool begun() const { return begun_; }
    uint16_t maxSample() const {
        return resolution_bits_ >= 16 ? 0xffffu
                                      : static_cast<uint16_t>((1ul << resolution_bits_) - 1ul);
    }

private:
    uint8_t pin_;
    uint8_t resolution_bits_;
    bool begun_;
};

class ArduinoPwm {
public:
    explicit ArduinoPwm(uint8_t pin, uint8_t resolution_bits = 8)
        : pin_(pin), resolution_bits_(resolution_bits), begun_(false) {}

    static bool supportsResolution(uint8_t resolution_bits) {
#if defined(ARDUINO_ARCH_AVR)
        // The classic AVR analogWrite contract consumes an 8-bit duty value.
        return resolution_bits == 8;
#elif defined(ARDUINO_ARCH_ESP32)
        // Fourteen bits is supported across the ESP32 targets at the core's
        // default analogWrite frequency; higher widths are target-dependent.
        return resolution_bits >= 1 && resolution_bits <= 14;
#elif defined(ARDUINO_ARCH_RENESAS) || defined(ARDUINO_ARCH_NRF52)
        return resolution_bits >= 1 && resolution_bits <= 16;
#else
        // Do not call a resolution API or advertise a width on an unknown core.
        return resolution_bits == 8;
#endif
    }

    bool begin() {
        if (!supportsResolution(resolution_bits_)) return false;
        pinMode(pin_, OUTPUT);
        begun_ = true;
        return setDuty(0);
    }
    bool setDuty(uint16_t duty) const {
        if (!begun_ || duty > maxDuty()) return false;
#if defined(ARDUINO_ARCH_ESP32)
        // The ESP32 API is pin-scoped, but reapplying here also repairs state
        // changed by code outside this provider between two writes.
        analogWriteResolution(pin_, resolution_bits_);
#elif defined(ARDUINO_ARCH_RENESAS) || defined(ARDUINO_ARCH_NRF52)
        // These cores expose process-wide PWM resolution state. Bind each
        // write to this instance's declared width instead of relying on the
        // most recently begun ArduinoPwm object.
        analogWriteResolution(resolution_bits_);
#endif
        analogWrite(pin_, duty);
        return true;
    }
    bool begun() const { return begun_; }
    uint16_t maxDuty() const {
        return resolution_bits_ >= 16 ? 0xffffu
                                      : static_cast<uint16_t>((1ul << resolution_bits_) - 1ul);
    }

private:
    uint8_t pin_;
    uint8_t resolution_bits_;
    bool begun_;
};

#if defined(NOBRO_ARDUINO_ENABLE_I2C)
enum ArduinoI2cError : uint8_t {
    ARDUINO_I2C_OK = 0,
    ARDUINO_I2C_NOT_BEGUN,
    ARDUINO_I2C_INVALID_ARGUMENT,
    ARDUINO_I2C_SHORT_WRITE,
    ARDUINO_I2C_BUS_ERROR,
    ARDUINO_I2C_SHORT_READ,
};

class ArduinoI2c {
public:
    explicit ArduinoI2c(TwoWire &wire = Wire)
        : wire_(wire), begun_(false), error_(ARDUINO_I2C_NOT_BEGUN), bus_status_(0) {}
    bool begin(uint32_t frequency_hz = 400000ul) {
        if (frequency_hz == 0) return fail(ARDUINO_I2C_INVALID_ARGUMENT);
        wire_.begin();
        wire_.setClock(frequency_hz);
        begun_ = true;
        error_ = ARDUINO_I2C_OK;
        bus_status_ = 0;
        return true;
    }
    bool write(uint8_t address, const uint8_t *bytes, size_t length, bool stop = true) {
        if (!begun_) return fail(ARDUINO_I2C_NOT_BEGUN);
        if (address > 0x7fu || (bytes == nullptr && length != 0))
            return fail(ARDUINO_I2C_INVALID_ARGUMENT);
        wire_.beginTransmission(address);
        const size_t written = length == 0 ? 0 : wire_.write(bytes, length);
        // Always close a begun transaction. In particular, a short staging-buffer
        // write must not leave Wire holding a repeated-start transaction.
        bus_status_ = wire_.endTransmission(stop || written != length);
        if (written != length) return fail(ARDUINO_I2C_SHORT_WRITE);
        if (bus_status_ != 0) return fail(ARDUINO_I2C_BUS_ERROR);
        error_ = ARDUINO_I2C_OK;
        return true;
    }
    size_t read(uint8_t address, uint8_t *bytes, size_t capacity, bool stop = true) {
        if (!begun_) {
            fail(ARDUINO_I2C_NOT_BEGUN);
            return 0;
        }
        if (address > 0x7fu || bytes == nullptr || capacity == 0 || capacity > 255) {
            fail(ARDUINO_I2C_INVALID_ARGUMENT);
            return 0;
        }
        const size_t available = wire_.requestFrom(address, static_cast<uint8_t>(capacity),
                                                   static_cast<uint8_t>(stop));
        size_t count = 0;
        while (count < available && count < capacity && wire_.available()) {
            const int value = wire_.read();
            if (value < 0) break;
            bytes[count++] = static_cast<uint8_t>(value);
        }
        error_ = available == capacity && count == capacity
                     ? ARDUINO_I2C_OK
                     : ARDUINO_I2C_SHORT_READ;
        return count;
    }
    size_t writeRead(uint8_t address, const uint8_t *write_bytes, size_t write_length,
                     uint8_t *read_bytes, size_t read_capacity) {
        if (!begun_) {
            fail(ARDUINO_I2C_NOT_BEGUN);
            return 0;
        }
        if (address > 0x7fu || (write_bytes == nullptr && write_length != 0) ||
            read_bytes == nullptr || read_capacity == 0 || read_capacity > 255) {
            fail(ARDUINO_I2C_INVALID_ARGUMENT);
            return 0;
        }
        if (!write(address, write_bytes, write_length, false)) return 0;
        return read(address, read_bytes, read_capacity, true);
    }
    bool probe(uint8_t address) { return write(address, nullptr, 0, true); }
    ArduinoI2cError lastError() const { return error_; }
    uint8_t lastBusStatus() const { return bus_status_; }

private:
    bool fail(ArduinoI2cError error) {
        error_ = error;
        return false;
    }

    TwoWire &wire_;
    bool begun_;
    ArduinoI2cError error_;
    uint8_t bus_status_;
};
#endif

#if defined(NOBRO_ARDUINO_ENABLE_SPI)
class ArduinoSpi {
public:
    ArduinoSpi(uint8_t chip_select, SPIClass &spi = SPI, uint32_t frequency_hz = 4000000ul)
        : chip_select_(chip_select), spi_(spi), settings_(frequency_hz, MSBFIRST, SPI_MODE0),
          valid_(frequency_hz != 0), begun_(false) {}

    bool begin() {
        if (!valid_) return false;
        if (begun_) return true;
        pinMode(chip_select_, OUTPUT);
        digitalWrite(chip_select_, HIGH);
        spi_.begin();
        begun_ = true;
        return true;
    }
    bool transfer(const uint8_t *write_bytes, uint8_t *read_bytes, size_t length) {
        if (!begun_) return false;
        if ((write_bytes == nullptr || read_bytes == nullptr) && length != 0) return false;
        if (length == 0) return true;
        spi_.beginTransaction(settings_);
        digitalWrite(chip_select_, LOW);
        for (size_t i = 0; i < length; ++i) read_bytes[i] = spi_.transfer(write_bytes[i]);
        digitalWrite(chip_select_, HIGH);
        spi_.endTransaction();
        return true;
    }
    bool begun() const { return begun_; }

private:
    uint8_t chip_select_;
    SPIClass &spi_;
    SPISettings settings_;
    bool valid_;
    bool begun_;
};
#endif

class ArduinoByteIo {
public:
    // Each write call performs exactly one bounded delegation to Stream. This
    // bounds work performed by this wrapper; the selected Arduino core may still
    // buffer, allocate, or block inside its Stream::write implementation.
    enum {
        MAX_WRITE_BYTES_PER_CALL = 64,
        MAX_WRITE_ATTEMPTS_PER_CALL = 1,
    };

    explicit ArduinoByteIo(Stream &stream) : stream_(stream) {}
    size_t readAvailable(uint8_t *bytes, size_t capacity) {
        if (bytes == nullptr) return 0;
        size_t count = 0;
        while (count < capacity && stream_.available()) {
            const int value = stream_.read();
            if (value < 0) break;
            bytes[count++] = static_cast<uint8_t>(value);
        }
        return count;
    }
    // Compatibility name: this is a resumable prefix write, not an unbounded
    // retry loop. The return value is the exact accepted prefix for this call.
    size_t writeAll(const uint8_t *bytes, size_t length) {
        if (bytes == nullptr && length != 0) return 0;
        if (length == 0) return 0;
        const size_t requested =
            length < static_cast<size_t>(MAX_WRITE_BYTES_PER_CALL)
                ? length
                : static_cast<size_t>(MAX_WRITE_BYTES_PER_CALL);
        const size_t written = stream_.write(bytes, requested);
        // A Stream that reports more bytes than requested violates Print's
        // contract. Do not turn that impossible count into a false success.
        return written <= requested ? written : 0;
    }
    void flush() { stream_.flush(); }

private:
    Stream &stream_;
};

}  // namespace nobro

#endif
