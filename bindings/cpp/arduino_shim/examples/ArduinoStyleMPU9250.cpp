/*
 * ArduinoStyleMPU9250.cpp - a sensor driver written EXACTLY like an Arduino library,
 * running under NobroRTOS through NobroArduinoShim.h (the one-include-swap port).
 *
 * The class below is the shape you download from the Library Manager: a constructor
 * taking a CS pin, begin() doing SPI writes with digitalWrite framing, register
 * helpers using SPI.transfer, readAccel() returning raw counts. Nothing NobroRTOS-
 * specific appears in the class; the RTOS integration is the extern "C" exports at
 * the bottom, which the host's ImuSal backend calls.
 */
#include "../NobroArduinoShim.h"

class MPU9250 {
public:
    explicit MPU9250(uint8_t csPin) : _cs(csPin) {}

    bool begin() {
        pinMode(_cs, OUTPUT);
        digitalWrite(_cs, HIGH);
        SPI.begin();
        writeRegister(0x6B, 0x80);  // PWR_MGMT_1: reset
        delay(20);
        writeRegister(0x6B, 0x01);  // wake, auto clock
        writeRegister(0x6A, 0x10);  // USER_CTRL: SPI only
        writeRegister(0x6C, 0x00);  // accel + gyro on
        writeRegister(0x1A, 0x03);  // DLPF 41 Hz
        writeRegister(0x19, 0x04);  // 200 Hz
        writeRegister(0x1B, 0x00);  // gyro +/-250 dps
        writeRegister(0x1C, 0x00);  // accel +/-2 g
        delay(20);
        return whoAmI() == 0x71;
    }

    uint8_t whoAmI() { return readRegister(0x75); }

    void readAccel(int16_t &ax, int16_t &ay, int16_t &az) {
        uint8_t raw[6];
        readRegisters(0x3B, raw, 6);
        ax = (int16_t)(((uint16_t)raw[0] << 8) | raw[1]);
        ay = (int16_t)(((uint16_t)raw[2] << 8) | raw[3]);
        az = (int16_t)(((uint16_t)raw[4] << 8) | raw[5]);
    }

private:
    uint8_t _cs;

    void writeRegister(uint8_t reg, uint8_t value) {
        SPI.beginTransaction(SPISettings(1000000, MSBFIRST, SPI_MODE3));
        digitalWrite(_cs, LOW);
        SPI.transfer(reg & 0x7F);
        SPI.transfer(value);
        digitalWrite(_cs, HIGH);
        SPI.endTransaction();
    }

    uint8_t readRegister(uint8_t reg) {
        SPI.beginTransaction(SPISettings(1000000, MSBFIRST, SPI_MODE3));
        digitalWrite(_cs, LOW);
        SPI.transfer(reg | 0x80);
        uint8_t v = SPI.transfer(0x00);
        digitalWrite(_cs, HIGH);
        SPI.endTransaction();
        return v;
    }

    void readRegisters(uint8_t reg, uint8_t *buf, uint8_t n) {
        SPI.beginTransaction(SPISettings(1000000, MSBFIRST, SPI_MODE3));
        digitalWrite(_cs, LOW);
        SPI.transfer(reg | 0x80);
        for (uint8_t i = 0; i < n; i++) {
            buf[i] = SPI.transfer(0x00);
        }
        digitalWrite(_cs, HIGH);
        SPI.endTransaction();
    }
};

/* ---- NobroRTOS integration: the host's ImuSal backend drives the library ---- */

namespace {
MPU9250 imu(10);  // "pin 10" - the shim routes CS to the host's real chip-select
}

extern "C" {
int32_t arduino_imu_begin(void) { return imu.begin() ? 0 : -1; }
uint8_t arduino_imu_whoami(void) { return imu.whoAmI(); }
void arduino_imu_read_accel(int32_t out_counts[3]) {
    int16_t ax = 0, ay = 0, az = 0;
    imu.readAccel(ax, ay, az);
    out_counts[0] = ax;
    out_counts[1] = ay;
    out_counts[2] = az;
}
}
