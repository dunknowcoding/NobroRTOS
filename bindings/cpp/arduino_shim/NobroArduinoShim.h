/*
 * NobroArduinoShim.h - run Arduino-style sensor drivers under NobroRTOS.
 *
 * Arduino sensor libraries talk to hardware through a tiny API surface: SPI.transfer,
 * digitalWrite on a chip-select, pinMode, delay. This header provides exactly that
 * surface, backed by extern "C" callbacks the NobroRTOS host implements over its own
 * leased bus drivers. Porting a typical SPI sensor library is then one include swap:
 *
 *     #include <Arduino.h>              ->   #include "NobroArduinoShim.h"
 *     #include <SPI.h>                  ->   (already provided)
 *
 * Scope (deliberate): ONE SPI device per module. digitalWrite(cs, LOW/HIGH) maps to the
 * host's select/deselect for the single wired device, matching how Arduino SPI sensor
 * libraries frame every transaction. I2C-flavored libraries get a Wire shim later.
 */
#ifndef NOBRO_ARDUINO_SHIM_H
#define NOBRO_ARDUINO_SHIM_H

#include <stdint.h>

/* Host callbacks: the NobroRTOS app implements these over its leased bus drivers. */
extern "C" {
void nobro_shim_spi_select(void);
void nobro_shim_spi_deselect(void);
uint8_t nobro_shim_spi_transfer(uint8_t out);
void nobro_shim_delay_ms(uint32_t ms);
}

/* ---- the Arduino API subset sensor libraries actually use ---- */

#define HIGH 1
#define LOW 0
#define INPUT 0
#define OUTPUT 1
#define MSBFIRST 1
#define SPI_MODE0 0
#define SPI_MODE3 3

struct SPISettings {
    SPISettings(uint32_t, uint8_t, uint8_t) {}
    SPISettings() {}
};

class ShimSPIClass {
public:
    void begin() {}
    void beginTransaction(SPISettings) { nobro_shim_spi_select(); }
    void endTransaction() { nobro_shim_spi_deselect(); }
    uint8_t transfer(uint8_t out) { return nobro_shim_spi_transfer(out); }
};

/* The global `SPI` object Arduino code expects. Header-only safe: each translation
 * unit of the module may hold its own instance (the class is stateless). */
static ShimSPIClass SPI;

inline void pinMode(uint8_t, uint8_t) {}
inline void digitalWrite(uint8_t, uint8_t level) {
    /* single-device scope: CS low = select, CS high = deselect */
    if (level == LOW) {
        nobro_shim_spi_select();
    } else {
        nobro_shim_spi_deselect();
    }
}
inline void delay(uint32_t ms) { nobro_shim_delay_ms(ms); }
inline void delayMicroseconds(uint32_t us) { nobro_shim_delay_ms(us / 1000 + 1); }

#endif /* NOBRO_ARDUINO_SHIM_H */
