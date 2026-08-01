#ifndef NOBRO_ESP8266_H
#define NOBRO_ESP8266_H

#if !defined(ARDUINO_ARCH_ESP8266)
#error "NobroEsp8266.h requires the Arduino-ESP8266 board package"
#endif

#include <Arduino.h>
#include <SPI.h>
#include <Wire.h>

namespace nobro {

/* Exact constrained profile for the 16-pin WeMos D1 Mini. GPIO6..GPIO11 are
 * reserved by the external flash. GPIO0/2/15 are boot straps and GPIO16 has no
 * normal external interrupt; they remain usable only through explicit calls. */
struct WemosD1MiniProfile {
    static bool exposedGpio(uint8_t pin) {
        switch (pin) {
        case 0: case 2: case 4: case 5: case 12:
        case 13: case 14: case 15: case 16:
            return true;
        default:
            return false;
        }
    }
    static bool bootStrapGpio(uint8_t pin) {
        return pin == 0 || pin == 2 || pin == 15;
    }
    static bool interruptGpio(uint8_t pin) {
        return exposedGpio(pin) && pin != 16;
    }
    static bool adcPin(uint8_t pin) { return pin == A0; }
    static uint8_t ledPin() { return LED_BUILTIN; }
    static uint8_t defaultSda() { return 4; }
    static uint8_t defaultScl() { return 5; }
    static uint8_t defaultSpiCs() { return 15; }
};

struct Esp8266Capabilities {
    static bool nativeUsb() { return false; }
    static bool multicore() { return false; }
    static bool memoryIsolation() { return false; }
    static bool hardwarePwm() { return false; }
    static bool vendorManagedWifiHeap() { return true; }
    static uint8_t cores() { return 1; }
    static uint8_t adcChannels() { return 1; }
};

struct Esp8266Clock {
    static uint32_t nowUs() { return micros(); }
    static bool reached(uint32_t now, uint32_t deadline) {
        return static_cast<int32_t>(now - deadline) >= 0;
    }
};

class Esp8266Deadline {
public:
    Esp8266Deadline() : deadline_us_(0), armed_(false) {}

    bool armAfterUs(uint32_t delay_us) {
        if (delay_us == 0 || delay_us > 0x7ffffffful) return false;
        deadline_us_ = Esp8266Clock::nowUs() + delay_us;
        armed_ = true;
        return true;
    }
    bool due() {
        if (!armed_ || !Esp8266Clock::reached(Esp8266Clock::nowUs(), deadline_us_))
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

class Esp8266Gpio {
public:
    explicit Esp8266Gpio(uint8_t pin)
        : pin_(pin), begun_(false), output_(false) {}

    bool begin(uint8_t mode) {
        if (!WemosD1MiniProfile::exposedGpio(pin_)) return false;
        if (mode != INPUT && mode != OUTPUT && mode != INPUT_PULLUP &&
            !(pin_ == 16 && mode == INPUT_PULLDOWN_16)) return false;
        pinMode(pin_, mode);
        begun_ = true;
        output_ = mode == OUTPUT;
        return true;
    }
    bool write(bool high) const {
        if (!begun_ || !output_) return false;
        digitalWrite(pin_, high ? HIGH : LOW);
        return true;
    }
    bool read(bool &high) const {
        if (!begun_) return false;
        high = digitalRead(pin_) == HIGH;
        return true;
    }
    bool isBootStrap() const { return WemosD1MiniProfile::bootStrapGpio(pin_); }

private:
    uint8_t pin_;
    bool begun_;
    bool output_;
};

class Esp8266Interrupt {
public:
    explicit Esp8266Interrupt(uint8_t pin) : pin_(pin), attached_(false) {}

    bool attach(void (*handler)(), int mode) {
        if (handler == 0 || !WemosD1MiniProfile::interruptGpio(pin_)) return false;
        if (mode != CHANGE && mode != RISING && mode != FALLING) return false;
        attachInterrupt(digitalPinToInterrupt(pin_), handler, mode);
        attached_ = true;
        return true;
    }
    void detach() {
        if (attached_) detachInterrupt(digitalPinToInterrupt(pin_));
        attached_ = false;
    }
    bool attached() const { return attached_; }

private:
    uint8_t pin_;
    bool attached_;
};

class Esp8266Pwm {
public:
    explicit Esp8266Pwm(uint8_t pin)
        : pin_(pin), begun_(false), frequency_hz_(0) {}

    bool begin(uint32_t frequency_hz = 1000) {
        if (!WemosD1MiniProfile::exposedGpio(pin_) || frequency_hz < 100 ||
            frequency_hz > 40000) return false;
        pinMode(pin_, OUTPUT);
        analogWriteRange(1023);
        analogWriteFreq(frequency_hz);
        analogWrite(pin_, 0);
        frequency_hz_ = frequency_hz;
        begun_ = true;
        return true;
    }
    bool setDuty(uint16_t duty) const {
        if (!begun_ || duty > 1023) return false;
        /* Arduino-ESP8266 implements this in software waveform scheduling. */
        analogWrite(pin_, duty);
        return true;
    }
    uint16_t maxDuty() const { return 1023; }
    uint32_t frequencyHz() const { return frequency_hz_; }

private:
    uint8_t pin_;
    bool begun_;
    uint32_t frequency_hz_;
};

class Esp8266Adc {
public:
    explicit Esp8266Adc(uint8_t pin = A0) : pin_(pin) {}

    bool read(uint16_t &sample) const {
        if (!WemosD1MiniProfile::adcPin(pin_)) return false;
        const int value = analogRead(pin_);
        if (value < 0 || value > 1023) return false;
        sample = static_cast<uint16_t>(value);
        return true;
    }
    uint16_t maxSample() const { return 1023; }

private:
    uint8_t pin_;
};

class Esp8266I2c {
public:
    explicit Esp8266I2c(TwoWire &wire = Wire) : wire_(wire), begun_(false) {}

    bool begin(uint8_t sda = WemosD1MiniProfile::defaultSda(),
               uint8_t scl = WemosD1MiniProfile::defaultScl(),
               uint32_t frequency_hz = 400000ul) {
        if (!WemosD1MiniProfile::exposedGpio(sda) ||
            !WemosD1MiniProfile::exposedGpio(scl) || sda == scl ||
            frequency_hz < 10000ul || frequency_hz > 400000ul) return false;
        wire_.begin(sda, scl);
        wire_.setClock(frequency_hz);
        begun_ = true;
        return true;
    }
    bool probe(uint8_t address) {
        if (!begun_ || address > 0x7f) return false;
        wire_.beginTransmission(address);
        return wire_.endTransmission(true) == 0;
    }
    bool begun() const { return begun_; }

private:
    TwoWire &wire_;
    bool begun_;
};

class Esp8266Spi {
public:
    explicit Esp8266Spi(uint8_t chip_select = WemosD1MiniProfile::defaultSpiCs(),
                        SPIClass &spi = SPI)
        : chip_select_(chip_select), spi_(spi), begun_(false) {}

    bool begin() {
        if (!WemosD1MiniProfile::exposedGpio(chip_select_)) return false;
        pinMode(chip_select_, OUTPUT);
        digitalWrite(chip_select_, HIGH);
        spi_.begin();
        begun_ = true;
        return true;
    }
    bool transfer(const uint8_t *tx, uint8_t *rx, size_t length,
                  uint32_t frequency_hz = 4000000ul) {
        if (!begun_ || frequency_hz < 1000ul || frequency_hz > 40000000ul ||
            ((tx == 0 || rx == 0) && length != 0)) return false;
        SPISettings settings(frequency_hz, MSBFIRST, SPI_MODE0);
        spi_.beginTransaction(settings);
        digitalWrite(chip_select_, LOW);
        for (size_t index = 0; index < length; ++index) rx[index] = spi_.transfer(tx[index]);
        digitalWrite(chip_select_, HIGH);
        spi_.endTransaction();
        return true;
    }

private:
    uint8_t chip_select_;
    SPIClass &spi_;
    bool begun_;
};

class Esp8266Uart {
public:
    explicit Esp8266Uart(HardwareSerial &serial = Serial) : serial_(serial), begun_(false) {}

    bool begin(uint32_t baud = 115200) {
        if (baud < 300 || baud > 4000000ul) return false;
        serial_.begin(baud);
        begun_ = true;
        return true;
    }
    size_t write(const uint8_t *bytes, size_t length) {
        if (!begun_ || (bytes == 0 && length != 0)) return 0;
        const size_t bounded = length < 64 ? length : 64;
        return serial_.write(bytes, bounded);
    }
    size_t read(uint8_t *bytes, size_t capacity) {
        if (!begun_ || bytes == 0) return 0;
        const size_t bounded = capacity < 64 ? capacity : 64;
        size_t count = 0;
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

struct Esp8266Power {
    /* Cooperative idle services the Wi-Fi/TCP stacks and watchdog. It is the
     * normal Nobro idle path on this target and never causes a reset. */
    static void cooperativeIdle() { yield(); }

    /* Deep sleep on ESP8266 is a reset-style transition and wake requires the
     * board's GPIO16-to-RST hardware connection. Keep it explicit and opt-in. */
    static bool deepSleepReset(uint64_t duration_us) {
        if (duration_us == 0 || duration_us > ESP.deepSleepMax()) return false;
        ESP.deepSleep(duration_us, WAKE_RF_DEFAULT);
        return true;
    }
};

struct Esp8266Reset {
    static void restart() { ESP.restart(); }
};

}  // namespace nobro

#endif
