//! Reference NobroRTOS module written against the C ABI (see
//! bindings/c/include/nobro_app.h). It uses ONLY the extern "C" host services and
//! exposes the extern "C" `nobro_app_init` / `nobro_app_poll` callbacks the kernel
//! drives - exactly the surface a C author implements. Written in Rust with
//! `extern "C"` so the boundary links + runs on hardware without a C cross-compiler;
//! the equivalent C is bindings/c/examples/imu_module.c (identical ABI).
#![no_std]

// Host services provided by the NobroRTOS app (resolved at link time, C ABI).
extern "C" {
    fn nobro_i2c_write(addr: u8, tx: *const u8, len: u32) -> i32;
    fn nobro_i2c_write_read(
        addr: u8,
        tx: *const u8,
        tx_len: u32,
        rx: *mut u8,
        rx_len: u32,
    ) -> i32;
    fn nobro_publish_imu(
        who: u8,
        dev_addr: u8,
        ax: i16,
        ay: i16,
        az: i16,
        gx: i16,
        gy: i16,
        gz: i16,
        temp_raw: i16,
    );
}

const IMU_ADDR: u8 = 0x68;
const REG_WHO_AM_I: u8 = 0x75;
const REG_PWR_MGMT_1: u8 = 0x6B;
const REG_ACCEL_XOUT_H: u8 = 0x3B;

/// Kernel calls this once before polling. Wake the IMU.
#[no_mangle]
pub extern "C" fn nobro_app_init() -> i32 {
    let cmd = [REG_PWR_MGMT_1, 0x01];
    unsafe { nobro_i2c_write(IMU_ADDR, cmd.as_ptr(), 2) }
}

/// Kernel calls this each cycle. Read WHO_AM_I + the 14-byte burst and publish.
#[no_mangle]
pub extern "C" fn nobro_app_poll() -> i32 {
    let reg_who = [REG_WHO_AM_I];
    let mut who = [0u8; 1];
    if unsafe { nobro_i2c_write_read(IMU_ADDR, reg_who.as_ptr(), 1, who.as_mut_ptr(), 1) } < 0 {
        return -1;
    }
    let reg_burst = [REG_ACCEL_XOUT_H];
    let mut raw = [0u8; 14];
    if unsafe { nobro_i2c_write_read(IMU_ADDR, reg_burst.as_ptr(), 1, raw.as_mut_ptr(), 14) } < 0 {
        return -2;
    }
    let rd = |i: usize| i16::from_be_bytes([raw[i], raw[i + 1]]);
    unsafe {
        nobro_publish_imu(
            who[0],
            IMU_ADDR,
            rd(0),
            rd(2),
            rd(4),
            rd(8),
            rd(10),
            rd(12),
            rd(6),
        );
    }
    0
}
