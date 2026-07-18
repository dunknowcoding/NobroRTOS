/*
 * nobro_app.hpp - C++ facade for authoring a NobroRTOS module.
 *
 * A thin, zero-overhead C++ wrapper over the C ABI (nobro_app.h): the same two
 * callbacks and host services, but with a tidy `nobro::` surface and a one-line
 * registration macro. It is deliberately embedded-safe for bare-metal C++:
 *   - no global constructors (cortex-m-rt does not run .init_array),
 *   - no virtual dispatch / vtables, no exceptions, no RTTI, no heap.
 * Compile with -fno-exceptions -fno-rtti. A module is a plain struct with two
 * static methods; NOBRO_REGISTER_MODULE wires it to the kernel callbacks.
 *
 * See bindings/cpp/examples/imu_module.cpp.
 */
#ifndef NOBRO_APP_HPP
#define NOBRO_APP_HPP

#include <stdint.h>
#include "nobro_app.h"

extern "C" {
uint64_t nobro_now_us(void);
int32_t nobro_i2c_write(uint8_t addr, const uint8_t *tx, uint32_t len);
int32_t nobro_i2c_write_read(uint8_t addr, const uint8_t *tx, uint32_t tx_len,
                             uint8_t *rx, uint32_t rx_len);
void nobro_publish_imu(uint8_t who, uint8_t dev_addr, int16_t ax, int16_t ay,
                       int16_t az, int16_t gx, int16_t gy, int16_t gz,
                       int16_t temp_raw);
}

namespace nobro {

/// Canonical task/wire authoring aliases over the allocation-free C facade.
inline int32_t task(const char *name, uint32_t period_us, nobro_step_fn step) {
    return nobro_task(name, period_us, step);
}

inline int32_t task(const char *name, uint32_t period_us, nobro_step_fn step,
                    const nobro_task_options_t &options) {
    return nobro_task_with(name, period_us, step, &options);
}

inline int32_t wire(const char *from, const char *to, uint32_t capacity = 1) {
    return nobro_wire(from, to, capacity);
}

inline int32_t run() { return nobro_run(); }
inline int32_t poll() { return nobro_poll(); }

/// Monotonic microsecond timebase.
inline uint64_t now_us() { return nobro_now_us(); }

/// I2C bus access (static; the NobroRTOS app owns the bus lease).
struct I2c {
    static int32_t write(uint8_t addr, const uint8_t *tx, uint32_t len) {
        return nobro_i2c_write(addr, tx, len);
    }
    static int32_t write_read(uint8_t addr, const uint8_t *tx, uint32_t tx_len,
                              uint8_t *rx, uint32_t rx_len) {
        return nobro_i2c_write_read(addr, tx, tx_len, rx, rx_len);
    }
    /// Read one register byte (returns <0 on bus error, else the byte 0..255).
    static int32_t read_reg(uint8_t addr, uint8_t reg) {
        uint8_t v = 0;
        int32_t rc = nobro_i2c_write_read(addr, &reg, 1, &v, 1);
        return rc < 0 ? rc : static_cast<int32_t>(v);
    }
};

/// Publish a parsed IMU sample to the host-readable report.
inline void publish_imu(uint8_t who, uint8_t dev_addr, int16_t ax, int16_t ay,
                        int16_t az, int16_t gx, int16_t gy, int16_t gz,
                        int16_t temp_raw) {
    nobro_publish_imu(who, dev_addr, ax, ay, az, gx, gy, gz, temp_raw);
}

}  // namespace nobro

/// Register a module type as the kernel-driven app. `Type` is any struct with
/// `static int32_t init()` and `static int32_t poll()` (no instance is created,
/// so there is no global constructor to run).
#define NOBRO_REGISTER_MODULE(Type)                                            \
    extern "C" int32_t nobro_app_init(void) { return Type::init(); }           \
    extern "C" int32_t nobro_app_poll(void) { return Type::poll(); }

/// Arduino-style registration: define `void setup()` and `void loop()`, then place
/// NOBRO_ARDUINO_MODULE() after them. setup() runs once after admission; loop() runs
/// every cycle. Familiar to Arduino users, same verified kernel callbacks underneath.
#define NOBRO_ARDUINO_MODULE()                                                 \
    extern "C" int32_t nobro_app_init(void) {                                  \
        setup();                                                               \
        return 0;                                                              \
    }                                                                          \
    extern "C" int32_t nobro_app_poll(void) {                                  \
        loop();                                                                \
        return 0;                                                              \
    }

#endif  // NOBRO_APP_HPP
