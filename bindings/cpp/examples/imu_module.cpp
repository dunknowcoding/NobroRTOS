/*
 * imu_module.cpp - reference NobroRTOS module written in C++ via the C++ facade.
 *
 * Same logic and same ABI as bindings/c/examples/imu_module.c, but in idiomatic,
 * embedded-safe C++: a struct with two static methods, registered in one line.
 * NobroRTOS's c_abi_demo compiles + links this (feature `cpp-source`,
 * arm-none-eabi-g++) and runs it on hardware.
 */
#include "nobro_app.hpp"

namespace {

constexpr uint8_t kImuAddr = 0x68;
constexpr uint8_t kRegWhoAmI = 0x75;
constexpr uint8_t kRegPwrMgmt1 = 0x6B;
constexpr uint8_t kRegAccelXoutH = 0x3B;

inline int16_t be16(const uint8_t *b, int i) {
    return static_cast<int16_t>((static_cast<uint16_t>(b[i]) << 8) | b[i + 1]);
}

struct ImuModule {
    static int32_t init() {
        const uint8_t wake[2] = {kRegPwrMgmt1, 0x01};  // wake + PLL clock
        return nobro::I2c::write(kImuAddr, wake, 2);
    }

    static int32_t poll() {
        const uint8_t reg_who = kRegWhoAmI;
        uint8_t who = 0;
        if (nobro::I2c::write_read(kImuAddr, &reg_who, 1, &who, 1) < 0) {
            return -1;
        }
        const uint8_t reg_burst = kRegAccelXoutH;
        uint8_t raw[14];  // accel(6) + temp(2) + gyro(6)
        if (nobro::I2c::write_read(kImuAddr, &reg_burst, 1, raw, 14) < 0) {
            return -2;
        }
        nobro::publish_imu(who, kImuAddr, be16(raw, 0), be16(raw, 2), be16(raw, 4),
                           be16(raw, 8), be16(raw, 10), be16(raw, 12), be16(raw, 6));
        return 0;
    }
};

}  // namespace

NOBRO_REGISTER_MODULE(ImuModule)
