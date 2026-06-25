/*
 * arduino_imu.cpp - the same IMU module in the familiar Arduino setup()/loop() style.
 *
 * Identical behavior and ABI to imu_module.cpp; only the registration idiom differs.
 * Build it by pointing c_abi_demo's build.rs at this file:
 *   NOBRO_CPP_MODULE=../../../bindings/cpp/examples/arduino_imu.cpp \
 *   cargo build -p nobro-c-abi-demo --no-default-features \
 *               --features board-promicro-nosd,cpp-source --release
 */
#include "nobro_app.hpp"

namespace {
constexpr uint8_t kImuAddr = 0x68;

inline int16_t be16(const uint8_t *b, int i) {
    return static_cast<int16_t>((static_cast<uint16_t>(b[i]) << 8) | b[i + 1]);
}
}  // namespace

void setup() {
    const uint8_t wake[2] = {0x6B, 0x01};  // PWR_MGMT_1 = wake + PLL
    nobro::I2c::write(kImuAddr, wake, 2);
}

void loop() {
    uint8_t reg_who = 0x75;
    uint8_t who = 0;
    if (nobro::I2c::write_read(kImuAddr, &reg_who, 1, &who, 1) < 0) {
        return;
    }
    uint8_t reg_burst = 0x3B;
    uint8_t raw[14];
    if (nobro::I2c::write_read(kImuAddr, &reg_burst, 1, raw, 14) < 0) {
        return;
    }
    nobro::publish_imu(who, kImuAddr, be16(raw, 0), be16(raw, 2), be16(raw, 4),
                       be16(raw, 8), be16(raw, 10), be16(raw, 12), be16(raw, 6));
}

NOBRO_ARDUINO_MODULE()
