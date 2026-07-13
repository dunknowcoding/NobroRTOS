#ifndef NOBRO_ARDUINO_PROVIDERS_H
#define NOBRO_ARDUINO_PROVIDERS_H

#include <Arduino.h>
#include <SPI.h>
#include <Wire.h>

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
        : pin_(pin), resolution_bits_(resolution_bits) {}

    void begin() {
        pinMode(pin_, INPUT);
#if !defined(ARDUINO_ARCH_AVR)
        analogReadResolution(resolution_bits_);
#endif
    }
    uint16_t read() const { return static_cast<uint16_t>(analogRead(pin_)); }
    uint16_t maxSample() const {
        return resolution_bits_ >= 16 ? 0xffffu
                                      : static_cast<uint16_t>((1ul << resolution_bits_) - 1ul);
    }

private:
    uint8_t pin_;
    uint8_t resolution_bits_;
};

class ArduinoPwm {
public:
    explicit ArduinoPwm(uint8_t pin, uint8_t resolution_bits = 8)
        : pin_(pin), resolution_bits_(resolution_bits) {}

    void begin() {
        pinMode(pin_, OUTPUT);
#if defined(ARDUINO_ARCH_ESP32)
        analogWriteResolution(pin_, resolution_bits_);
#elif !defined(ARDUINO_ARCH_AVR)
        analogWriteResolution(resolution_bits_);
#endif
        setDuty(0);
    }
    void setDuty(uint16_t duty) const { analogWrite(pin_, duty > maxDuty() ? maxDuty() : duty); }
    uint16_t maxDuty() const {
        return resolution_bits_ >= 16 ? 0xffffu
                                      : static_cast<uint16_t>((1ul << resolution_bits_) - 1ul);
    }

private:
    uint8_t pin_;
    uint8_t resolution_bits_;
};

class ArduinoI2c {
public:
    explicit ArduinoI2c(TwoWire &wire = Wire) : wire_(wire) {}
    void begin(uint32_t frequency_hz = 400000ul) {
        wire_.begin();
        wire_.setClock(frequency_hz);
    }
    bool write(uint8_t address, const uint8_t *bytes, size_t length, bool stop = true) {
        if (bytes == nullptr && length != 0) return false;
        wire_.beginTransmission(address);
        if (length != 0 && wire_.write(bytes, length) != length) return false;
        return wire_.endTransmission(stop) == 0;
    }
    size_t read(uint8_t address, uint8_t *bytes, size_t capacity, bool stop = true) {
        if (bytes == nullptr || capacity == 0 || capacity > 255) return 0;
        const size_t available = wire_.requestFrom(address, static_cast<uint8_t>(capacity),
                                                   static_cast<uint8_t>(stop));
        size_t count = 0;
        while (count < available && count < capacity && wire_.available())
            bytes[count++] = static_cast<uint8_t>(wire_.read());
        return count;
    }
    size_t writeRead(uint8_t address, const uint8_t *write_bytes, size_t write_length,
                     uint8_t *read_bytes, size_t read_capacity) {
        if (!write(address, write_bytes, write_length, false)) return 0;
        return read(address, read_bytes, read_capacity, true);
    }

private:
    TwoWire &wire_;
};

class ArduinoSpi {
public:
    ArduinoSpi(uint8_t chip_select, SPIClass &spi = SPI, uint32_t frequency_hz = 4000000ul)
        : chip_select_(chip_select), spi_(spi), settings_(frequency_hz, MSBFIRST, SPI_MODE0) {}

    void begin() {
        pinMode(chip_select_, OUTPUT);
        digitalWrite(chip_select_, HIGH);
        spi_.begin();
    }
    bool transfer(const uint8_t *write_bytes, uint8_t *read_bytes, size_t length) {
        if ((write_bytes == nullptr || read_bytes == nullptr) && length != 0) return false;
        spi_.beginTransaction(settings_);
        digitalWrite(chip_select_, LOW);
        for (size_t i = 0; i < length; ++i) read_bytes[i] = spi_.transfer(write_bytes[i]);
        digitalWrite(chip_select_, HIGH);
        spi_.endTransaction();
        return true;
    }

private:
    uint8_t chip_select_;
    SPIClass &spi_;
    SPISettings settings_;
};

class ArduinoByteIo {
public:
    explicit ArduinoByteIo(Stream &stream) : stream_(stream) {}
    size_t readAvailable(uint8_t *bytes, size_t capacity) {
        if (bytes == nullptr) return 0;
        size_t count = 0;
        while (count < capacity && stream_.available())
            bytes[count++] = static_cast<uint8_t>(stream_.read());
        return count;
    }
    size_t writeAll(const uint8_t *bytes, size_t length) {
        return bytes == nullptr && length != 0 ? 0 : stream_.write(bytes, length);
    }
    void flush() { stream_.flush(); }

private:
    Stream &stream_;
};

}  // namespace nobro

#endif
