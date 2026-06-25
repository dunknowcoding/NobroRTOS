/*
 * imu_module.c - reference NobroRTOS module written in C against the C ABI.
 *
 * Reads an MPU-9250-class IMU using only the nobro_app.h host services and exposes
 * the two module callbacks the kernel drives. This is the entire authoring surface
 * for a C module: no kernel headers, no linker scripts, no Rust. NobroRTOS's
 * c_abi_demo compiles + links this (feature `c-source`) and runs it on hardware;
 * a passing NOBRO_IMU_HW_EVAL_REPORT proves a C-authored module works end to end.
 */
#include "nobro_app.h"

#define IMU_ADDR 0x68
#define REG_WHO_AM_I 0x75
#define REG_PWR_MGMT_1 0x6B
#define REG_ACCEL_XOUT_H 0x3B

/* big-endian i16 from raw[i],raw[i+1] */
#define BE16(buf, i) ((int16_t)(((uint16_t)(buf)[i] << 8) | (uint16_t)(buf)[(i) + 1]))

int32_t nobro_app_init(void) {
    uint8_t wake[2] = {REG_PWR_MGMT_1, 0x01}; /* wake + PLL clock */
    return nobro_i2c_write(IMU_ADDR, wake, 2);
}

int32_t nobro_app_poll(void) {
    uint8_t reg_who = REG_WHO_AM_I;
    uint8_t who = 0;
    if (nobro_i2c_write_read(IMU_ADDR, &reg_who, 1, &who, 1) < 0) {
        return -1;
    }

    uint8_t reg_burst = REG_ACCEL_XOUT_H;
    uint8_t raw[14]; /* accel(6) + temp(2) + gyro(6) */
    if (nobro_i2c_write_read(IMU_ADDR, &reg_burst, 1, raw, 14) < 0) {
        return -2;
    }

    nobro_publish_imu(who, IMU_ADDR, BE16(raw, 0), BE16(raw, 2), BE16(raw, 4),
                      BE16(raw, 8), BE16(raw, 10), BE16(raw, 12), BE16(raw, 6));
    return 0;
}
