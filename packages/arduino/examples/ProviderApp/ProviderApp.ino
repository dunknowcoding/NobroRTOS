// This example explicitly opts in to provider and bus headers. Ordinary
// <NobroRTOS.h> users do not need their architecture to supply Wire or SPI.
#define NOBRO_ARDUINO_ENABLE_PROVIDERS
#define NOBRO_ARDUINO_ENABLE_I2C
#define NOBRO_ARDUINO_ENABLE_SPI
#include <NobroRTOS.h>
#include "ProviderReport.h"

nobro::ArduinoDeadline deadline;
nobro::ArduinoAdc adc(A0, 10);
nobro::ArduinoPwm pwm(5, 8);
#if defined(NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS)
static_assert(NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS >= 0x08 &&
                  NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS <= 0x77,
              "NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS must be an unreserved 7-bit address");
nobro::ArduinoI2c i2c(Wire);
#endif
nobro::ArduinoByteIo console(Serial);
ProviderReport report;

// Compile-gate the SPI shape used by an RFID adapter without assuming a chip-select,
// reset pin, reader model, or wiring. This function is deliberately never called.
static bool rfid_spi_api_compile_only(SPIClass &bus, uint8_t chip_select,
                                      const uint8_t *tx, uint8_t *rx, size_t length) {
  nobro::ArduinoSpi device(chip_select, bus);
  if (!device.begin()) return false;
  return device.transfer(tx, rx, length);
}

static bool adc_ready = false;
static bool pwm_ready = false;
static bool deadline_armed = false;
static bool setup_failure_reported = false;
#if defined(NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS)
static bool i2c_ready = false;
#endif

static void report_cycle(bool cycle_ok, bool adc_ok, bool pwm_ok,
                         bool i2c_exercised, bool i2c_ok) {
  // Arduino's generic analogWrite API has no portable physical-feedback signal;
  // "duty_requested" records only that the bounded provider call succeeded.
  (void)report.begin(deadline_armed, adc_ok, pwm_ok, i2c_exercised, i2c_ok,
                     cycle_ok);
}

static void run_cycle() {
  uint16_t sample = 0;
  const bool adc_ok = adc_ready && adc.read(sample);
  bool pwm_ok = false;
  if (adc_ok) {
    const uint16_t duty = static_cast<uint16_t>(
        (static_cast<uint32_t>(sample) * pwm.maxDuty()) / adc.maxSample());
    pwm_ok = pwm_ready && pwm.setDuty(duty);
  }

  bool i2c_exercised = false;
  bool i2c_ok = false;
#if defined(NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS)
  i2c_exercised = true;
  // Address-only probe: a real transaction with no device-register write.
  i2c_ok = i2c_ready &&
           i2c.probe(static_cast<uint8_t>(NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS));
#endif

  deadline_armed = deadline.armAfterUs(1000000ul);
  const bool cycle_ok = adc_ok && pwm_ok && deadline_armed &&
                        (!i2c_exercised || i2c_ok);
  report_cycle(cycle_ok, adc_ok, pwm_ok, i2c_exercised, i2c_ok);
}

void setup() {
  Serial.begin(115200);
  adc_ready = adc.begin();
  pwm_ready = pwm.begin();
#if defined(NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS)
  i2c_ready = i2c.begin();
#endif
  deadline_armed = deadline.armAfterUs(2000ul);
}

void loop() {
  // A short Stream write retains the exact unsent suffix. Do not run another
  // cycle until the current record is complete, so records cannot interleave.
  (void)report.resume(console);
  if (report.pending()) return;
  if (!deadline_armed) {
    if (!setup_failure_reported) {
      setup_failure_reported = true;
      run_cycle();
    }
    return;
  }
  if (deadline.due()) run_cycle();
}
