#ifndef NOBRO_AVR_NANO_H
#define NOBRO_AVR_NANO_H

#if !defined(ARDUINO_ARCH_AVR) || !defined(__AVR_ATmega328P__)
#error "NobroAvrNano.h requires an Arduino ATmega328P board package"
#endif

#include <Arduino.h>
#include <EEPROM.h>
#include <HardwareSerial.h>
#include <SPI.h>
#include <Wire.h>
#include <avr/interrupt.h>
#include <avr/sleep.h>
#include <avr/wdt.h>

#include "NobroRTOS.h"

namespace nobro {

struct AvrNanoCapabilities {
    enum : uint8_t {
        MAX_TASKS = 4,
        MAX_WIRES = 4,
        MAX_I2C_TRANSFER = 32,
        MAX_UART_TRANSFER = 64,
    };
    enum : uint32_t {
        APP_FLASH_LIMIT = 16ul * 1024ul,
        APP_RAM_LIMIT = 1024ul,
    };
    static bool nativeUsb() { return false; }
    static bool multicore() { return false; }
    static bool memoryIsolation() { return false; }
    static bool boundedDma() { return false; }
};

typedef NobroApp<AvrNanoCapabilities::MAX_TASKS,
                 AvrNanoCapabilities::MAX_WIRES> AvrNanoApp;

struct AvrNanoClock {
    static uint32_t nowUs() { return micros(); }
    static bool reached(uint32_t now, uint32_t deadline) {
        return static_cast<int32_t>(now - deadline) >= 0;
    }
};

class AvrNanoDeadline {
public:
    AvrNanoDeadline() : deadline_us_(0), armed_(false) {}

    bool armAfterUs(uint32_t delay_us) {
        if (delay_us == 0 || delay_us > 0x7ffffffful) return false;
        deadline_us_ = AvrNanoClock::nowUs() + delay_us;
        armed_ = true;
        return true;
    }
    bool due() {
        if (!armed_ || !AvrNanoClock::reached(AvrNanoClock::nowUs(), deadline_us_))
            return false;
        armed_ = false;
        return true;
    }
    void cancel() { armed_ = false; }
    bool armed() const { return armed_; }

private:
    uint32_t deadline_us_;
    bool armed_;
};

class AvrNanoGpio {
public:
    explicit AvrNanoGpio(uint8_t pin) : pin_(pin), begun_(false), output_(false) {}

    static bool supportsPin(uint8_t pin) { return pin < NUM_DIGITAL_PINS; }
    bool begin(uint8_t mode) {
        if (!supportsPin(pin_) ||
            (mode != INPUT && mode != INPUT_PULLUP && mode != OUTPUT))
            return false;
        pinMode(pin_, mode);
        output_ = mode == OUTPUT;
        begun_ = true;
        return true;
    }
    bool write(bool high) {
        if (!begun_ || !output_) return false;
        digitalWrite(pin_, high ? HIGH : LOW);
        return true;
    }
    bool read(bool &high) const {
        if (!begun_) return false;
        high = digitalRead(pin_) == HIGH;
        return true;
    }

private:
    uint8_t pin_;
    bool begun_;
    bool output_;
};

class AvrNanoInterrupt {
public:
    explicit AvrNanoInterrupt(uint8_t pin) : pin_(pin), attached_(false) {}

    static bool supportsPin(uint8_t pin) {
        return pin < NUM_DIGITAL_PINS && digitalPinToInterrupt(pin) != NOT_AN_INTERRUPT;
    }
    bool attach(void (*callback)(), int mode) {
        if (!supportsPin(pin_) || callback == 0 ||
            (mode != LOW && mode != CHANGE && mode != RISING && mode != FALLING))
            return false;
        attachInterrupt(digitalPinToInterrupt(pin_), callback, mode);
        attached_ = true;
        return true;
    }
    void detach() {
        if (supportsPin(pin_)) detachInterrupt(digitalPinToInterrupt(pin_));
        attached_ = false;
    }
    bool attached() const { return attached_; }

private:
    uint8_t pin_;
    bool attached_;
};

class AvrNanoEvent {
public:
    AvrNanoEvent() : pending_(false) {}
    void trigger() { pending_ = true; }
    bool take() {
        const uint8_t saved = SREG;
        cli();
        const bool pending = pending_;
        pending_ = false;
        SREG = saved;
        return pending;
    }

private:
    volatile bool pending_;
};

class AvrNanoPulse {
public:
    explicit AvrNanoPulse(uint8_t pin) : pin_(pin), begun_(false) {}
    bool begin() {
        if (pin_ >= NUM_DIGITAL_PINS) return false;
        pinMode(pin_, INPUT);
        begun_ = true;
        return true;
    }
    bool readHighUs(uint32_t timeout_us, uint32_t &width_us) const {
        if (!begun_ || timeout_us == 0) return false;
        const unsigned long width = pulseInLong(pin_, HIGH, timeout_us);
        if (width == 0) return false;
        width_us = width;
        return true;
    }

private:
    uint8_t pin_;
    bool begun_;
};

class AvrNanoWatchdog {
public:
    AvrNanoWatchdog() : armed_(false) {}

    bool beginInterrupt(uint8_t timeout_code) {
        if (armed_ || timeout_code > WDTO_8S) return false;
        const uint8_t saved = SREG;
        cli();
        wdt_reset();
        MCUSR &= static_cast<uint8_t>(~_BV(WDRF));
        WDTCSR = _BV(WDCE) | _BV(WDE);
        WDTCSR = _BV(WDIE) | (timeout_code & 0x07) |
                  ((timeout_code & 0x08) << 2);
        SREG = saved;
        expiredFlag() = false;
        armed_ = true;
        return true;
    }
    void feed() { if (armed_) wdt_reset(); }
    bool takeExpired() {
        const uint8_t saved = SREG;
        cli();
        const bool expired = expiredFlag();
        expiredFlag() = false;
        SREG = saved;
        return expired;
    }
    void stop() {
        if (!armed_) return;
        const uint8_t saved = SREG;
        cli();
        wdt_reset();
        MCUSR &= static_cast<uint8_t>(~_BV(WDRF));
        WDTCSR = _BV(WDCE) | _BV(WDE);
        WDTCSR = 0;
        SREG = saved;
        armed_ = false;
    }
    static void onInterrupt() { expiredFlag() = true; }

private:
    static volatile bool &expiredFlag() {
        static volatile bool expired = false;
        return expired;
    }
    bool armed_;
};

class AvrNanoLease {
public:
    enum : uint8_t { RESOURCE_COUNT = 16 };
    static bool acquire(uint8_t resource, uint8_t owner) {
        if (resource >= RESOURCE_COUNT || owner == 0) return false;
        const uint8_t saved = SREG;
        cli();
        uint8_t *owners = ownerTable();
        const bool available = owners[resource] == 0;
        if (available) owners[resource] = owner;
        SREG = saved;
        return available;
    }
    static bool release(uint8_t resource, uint8_t owner) {
        if (resource >= RESOURCE_COUNT || owner == 0) return false;
        const uint8_t saved = SREG;
        cli();
        uint8_t *owners = ownerTable();
        const bool held = owners[resource] == owner;
        if (held) owners[resource] = 0;
        SREG = saved;
        return held;
    }

private:
    static uint8_t *ownerTable() {
        static uint8_t owners[RESOURCE_COUNT] = {};
        return owners;
    }
};

class AvrNanoEepromStorage {
public:
    static uint16_t capacity() { return E2END + 1u; }
    static bool read(uint16_t offset, uint8_t *bytes, uint16_t length) {
        if ((bytes == 0 && length != 0) || offset > capacity() ||
            length > capacity() - offset)
            return false;
        for (uint16_t i = 0; i < length; ++i) bytes[i] = EEPROM.read(offset + i);
        return true;
    }
    static bool write(uint16_t offset, const uint8_t *bytes, uint16_t length) {
        if ((bytes == 0 && length != 0) || offset > capacity() ||
            length > capacity() - offset)
            return false;
        for (uint16_t i = 0; i < length; ++i) EEPROM.update(offset + i, bytes[i]);
        return true;
    }
};

class AvrNanoPwm {
public:
    explicit AvrNanoPwm(uint8_t pin) : pin_(pin), begun_(false) {}

    static bool supportsPin(uint8_t pin) {
        return pin == 3 || pin == 5 || pin == 6 ||
               pin == 9 || pin == 10 || pin == 11;
    }
    bool begin() {
        if (!supportsPin(pin_)) return false;
        pinMode(pin_, OUTPUT);
        begun_ = true;
        analogWrite(pin_, 0);
        return true;
    }
    bool setDuty(uint8_t duty) {
        if (!begun_) return false;
        analogWrite(pin_, duty);
        return true;
    }

private:
    uint8_t pin_;
    bool begun_;
};

class AvrNanoAdc {
public:
    explicit AvrNanoAdc(uint8_t pin) : pin_(pin), begun_(false) {}

    static bool supportsPin(uint8_t pin) { return pin >= A0 && pin <= A7; }
    bool begin() {
        if (!supportsPin(pin_)) return false;
        pinMode(pin_, INPUT);
        begun_ = true;
        return true;
    }
    bool read(uint16_t &sample) const {
        if (!begun_) return false;
        const int value = analogRead(pin_);
        if (value < 0 || value > 1023) return false;
        sample = static_cast<uint16_t>(value);
        return true;
    }

private:
    uint8_t pin_;
    bool begun_;
};

enum AvrNanoI2cError : uint8_t {
    AVR_NANO_I2C_OK = 0,
    AVR_NANO_I2C_NOT_BEGUN,
    AVR_NANO_I2C_INVALID_ARGUMENT,
    AVR_NANO_I2C_SHORT_WRITE,
    AVR_NANO_I2C_BUS_ERROR,
    AVR_NANO_I2C_SHORT_READ,
};

class AvrNanoI2c {
public:
    explicit AvrNanoI2c(TwoWire &wire = Wire)
        : wire_(wire), begun_(false), error_(AVR_NANO_I2C_NOT_BEGUN),
          bus_status_(0) {}

    bool begin(uint32_t frequency_hz = 400000ul) {
        if (frequency_hz < 10000ul || frequency_hz > 400000ul)
            return fail(AVR_NANO_I2C_INVALID_ARGUMENT);
        wire_.begin();
        wire_.setClock(frequency_hz);
        begun_ = true;
        error_ = AVR_NANO_I2C_OK;
        bus_status_ = 0;
        return true;
    }
    bool write(uint8_t address, const uint8_t *bytes, uint8_t length,
               bool stop = true) {
        if (!begun_) return fail(AVR_NANO_I2C_NOT_BEGUN);
        if (address > 0x7f || length > AvrNanoCapabilities::MAX_I2C_TRANSFER ||
            (bytes == 0 && length != 0))
            return fail(AVR_NANO_I2C_INVALID_ARGUMENT);
        wire_.beginTransmission(address);
        const size_t written = length == 0 ? 0 : wire_.write(bytes, length);
        bus_status_ = wire_.endTransmission(stop || written != length);
        if (written != length) return fail(AVR_NANO_I2C_SHORT_WRITE);
        if (bus_status_ != 0) return fail(AVR_NANO_I2C_BUS_ERROR);
        error_ = AVR_NANO_I2C_OK;
        return true;
    }
    uint8_t read(uint8_t address, uint8_t *bytes, uint8_t capacity,
                 bool stop = true) {
        if (!begun_) {
            fail(AVR_NANO_I2C_NOT_BEGUN);
            return 0;
        }
        if (address > 0x7f || bytes == 0 || capacity == 0 ||
            capacity > AvrNanoCapabilities::MAX_I2C_TRANSFER) {
            fail(AVR_NANO_I2C_INVALID_ARGUMENT);
            return 0;
        }
        const uint8_t available = wire_.requestFrom(
            address, capacity, static_cast<uint8_t>(stop));
        uint8_t count = 0;
        while (count < available && count < capacity && wire_.available()) {
            const int value = wire_.read();
            if (value < 0) break;
            bytes[count++] = static_cast<uint8_t>(value);
        }
        error_ = available == capacity && count == capacity
                     ? AVR_NANO_I2C_OK
                     : AVR_NANO_I2C_SHORT_READ;
        return count;
    }
    uint8_t writeRead(uint8_t address, const uint8_t *write_bytes,
                      uint8_t write_length, uint8_t *read_bytes,
                      uint8_t read_capacity) {
        if (!write(address, write_bytes, write_length, false)) return 0;
        return read(address, read_bytes, read_capacity, true);
    }
    bool probe(uint8_t address) { return write(address, 0, 0, true); }
    AvrNanoI2cError lastError() const { return error_; }
    uint8_t lastBusStatus() const { return bus_status_; }

private:
    bool fail(AvrNanoI2cError error) {
        error_ = error;
        return false;
    }

    TwoWire &wire_;
    bool begun_;
    AvrNanoI2cError error_;
    uint8_t bus_status_;
};

class AvrNanoSpi {
public:
    AvrNanoSpi(uint8_t chip_select, uint32_t frequency_hz = 4000000ul,
               SPIClass &spi = SPI)
        : chip_select_(chip_select), frequency_hz_(frequency_hz), spi_(spi),
          begun_(false) {}

    bool begin() {
        if (chip_select_ >= NUM_DIGITAL_PINS || frequency_hz_ < 1000ul ||
            frequency_hz_ > F_CPU / 2ul)
            return false;
        pinMode(chip_select_, OUTPUT);
        digitalWrite(chip_select_, HIGH);
        spi_.begin();
        begun_ = true;
        return true;
    }
    bool transfer(const uint8_t *write_bytes, uint8_t *read_bytes,
                  uint8_t length) {
        if (!begun_ || ((write_bytes == 0 || read_bytes == 0) && length != 0))
            return false;
        if (length == 0) return true;
        SPISettings settings(frequency_hz_, MSBFIRST, SPI_MODE0);
        spi_.beginTransaction(settings);
        digitalWrite(chip_select_, LOW);
        for (uint8_t i = 0; i < length; ++i)
            read_bytes[i] = spi_.transfer(write_bytes[i]);
        digitalWrite(chip_select_, HIGH);
        spi_.endTransaction();
        return true;
    }

private:
    uint8_t chip_select_;
    uint32_t frequency_hz_;
    SPIClass &spi_;
    bool begun_;
};

class AvrNanoUart {
public:
    explicit AvrNanoUart(HardwareSerial &serial = Serial)
        : serial_(serial), begun_(false) {}

    bool begin(uint32_t baud = 38400ul) {
        if (baud < 300ul || baud > 2000000ul) return false;
        serial_.begin(baud);
        begun_ = true;
        return true;
    }
    uint8_t write(const uint8_t *bytes, uint8_t length) {
        if (!begun_ || (bytes == 0 && length != 0)) return 0;
        const uint8_t bounded =
            length < AvrNanoCapabilities::MAX_UART_TRANSFER
                ? length
                : AvrNanoCapabilities::MAX_UART_TRANSFER;
        const size_t written = serial_.write(bytes, bounded);
        return written <= bounded ? static_cast<uint8_t>(written) : 0;
    }
    uint8_t read(uint8_t *bytes, uint8_t capacity) {
        if (!begun_ || bytes == 0) return 0;
        const uint8_t bounded =
            capacity < AvrNanoCapabilities::MAX_UART_TRANSFER
                ? capacity
                : AvrNanoCapabilities::MAX_UART_TRANSFER;
        uint8_t count = 0;
        while (count < bounded && serial_.available()) {
            const int value = serial_.read();
            if (value < 0) break;
            bytes[count++] = static_cast<uint8_t>(value);
        }
        return count;
    }

private:
    HardwareSerial &serial_;
    bool begun_;
};

struct AvrNanoPower {
    static void cooperativeIdle() { yield(); }

    static bool idleSleepOnce() {
        if ((SREG & _BV(SREG_I)) == 0) return false;
        set_sleep_mode(SLEEP_MODE_IDLE);
        /* AVR guarantees that the instruction after SEI executes before an
         * interrupt is serviced. Arm sleep with interrupts masked, then make
         * SLEEP that protected next instruction so a wake event cannot be
         * consumed in the enable-to-sleep race window. */
        cli();
        sleep_enable();
        sei();
        sleep_cpu();
        sleep_disable();
        return true;
    }
};

struct AvrNanoReset {
    static void restartApplication() {
        /* The ATmega328P old Nano bootloader does not reliably clear a watchdog
         * reset before it starts waiting for STK500. Re-entering it with WDT
         * enabled can trap the board in a reset loop. This exact composition
         * therefore provides an application restart, not a hardware-reset claim. */
        cli();
        wdt_disable();
        asm volatile("jmp 0");
        __builtin_unreachable();
    }
};

} // namespace nobro

// Emit this once in the sketch that owns the watchdog interrupt vector.
#define NOBRO_AVR_NANO_WATCHDOG_ISR() \
    ISR(WDT_vect) { nobro::AvrNanoWatchdog::onInterrupt(); }

#endif
