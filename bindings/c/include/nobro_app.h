/*
 * nobro_app.h - NobroRTOS C ABI for authoring a module in C.
 *
 * A C module implements the two callbacks (nobro_app_init / nobro_app_poll); the
 * NobroRTOS kernel admits it and drives them. The module reaches hardware and
 * publishes results only through the host services below - it never touches the
 * kernel internals. The ABI is plain extern "C", so a module compiled by any C
 * (or C++/Rust-extern-"C") toolchain links unchanged. See examples/imu_module.c.
 */
#ifndef NOBRO_APP_H
#define NOBRO_APP_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- Host services (provided by NobroRTOS; call these from your module) ---- */

/* Monotonic microsecond timebase. */
uint64_t nobro_now_us(void);

/* I2C write of `len` bytes to `addr`. Returns 0 on success, <0 on bus error. */
int32_t nobro_i2c_write(uint8_t addr, const uint8_t *tx, uint32_t len);

/* I2C write-then-read (register read): write `tx_len` bytes, then read `rx_len`
 * bytes from `addr`. Returns 0 on success, <0 on bus error. */
int32_t nobro_i2c_write_read(uint8_t addr, const uint8_t *tx, uint32_t tx_len,
                             uint8_t *rx, uint32_t rx_len);

/* Publish a parsed IMU sample; the kernel computes magnitudes and fills the
 * host-readable report (so modules need no floating-point/math dependency). */
void nobro_publish_imu(uint8_t who, uint8_t dev_addr, int16_t ax, int16_t ay,
                       int16_t az, int16_t gx, int16_t gy, int16_t gz,
                       int16_t temp_raw);

/* ---- Module callbacks (you implement these; the kernel calls them) ---- */

/* Called once after admission, before polling. Return <0 to abort. */
int32_t nobro_app_init(void);

/* Called every cycle. Return <0 to signal a transient module error. */
int32_t nobro_app_poll(void);

#ifdef __cplusplus
}
#endif

#endif /* NOBRO_APP_H */
