# NobroRTOS Arduino Package

This folder contains the Arduino IDE library distribution surface.
Its Library Manager source is the dedicated, package-root repository at
<https://github.com/dunknowcoding/NobroRTOS-Arduino>; this monorepo copy is the
gated canonical input used to produce it.

Current contents:

- `library.properties` for Arduino Library Manager compatible metadata.
- `src/NobroRTOS.h` with an allocation-free `NobroApp` task/wire facade and the
  canonical report ABI.
- `src/NobroArduinoProviders.h` with bounded clock/deadline/ADC/generic-duty-PWM,
  optional I2C/SPI, and byte-I/O wrappers that delegate hardware ownership to the
  selected Arduino board package.
- beginner, provider, complex robot/IoT, and report-reader examples compile-gated across AVR,
  UNO R4/RA4M1, ESP32-S3, and ArduinoNRF in the repository toolchain.

## Install and select a board environment

In Arduino IDE 2.x:

1. Install the board package for your MCU in Boards Manager.
2. Install **NobroRTOS** in Library Manager.
3. Select the exact board and port under **Tools**.
4. Open **File > Examples > NobroRTOS > BeginnerApp** and upload it.

For a local checkout, install the release archive with:

```bash
arduino-cli lib install --zip-path NobroRTOS-Arduino-0.3.2.zip
```

Board cores own upload tools, bootloaders, USB configuration, pins, interrupts,
and peripheral implementations. NobroRTOS does not replace those settings.

## Configure only what the sketch uses

The Arduino package remains a thin compatibility surface over the core contracts. The
installed board package continues to own register setup, interrupts, and pin routing.
Provider wrappers are opt-in: define `NOBRO_ARDUINO_ENABLE_PROVIDERS` before including
`NobroRTOS.h`. Define `NOBRO_ARDUINO_ENABLE_I2C` and/or
`NOBRO_ARDUINO_ENABLE_SPI` only when that sketch needs those board-core libraries.
This keeps the package's `architectures=*` declaration from imposing Wire or SPI on
unrelated targets.

`ProviderApp` compiles the RFID-facing SPI shape but never touches a reader or assumes
its wiring. To exercise I2C, explicitly define `NOBRO_PROVIDER_EXAMPLE_I2C_ADDRESS` to
an unreserved 7-bit target address. The example then performs a non-mutating,
address-only probe and reports either `acknowledged` or `error`; without a target it
reports `not_exercised`. Its PWM result means the generic duty request was accepted by
the facade, not that pulse timing or physical output was measured.

Resolution requests are validated before any pin or core state is changed. The current
facade policy accepts ADC/PWM widths of 10/8 bits on classic AVR, 1–16/1–14 on ESP32,
{8, 10, 12, 14, 16}/1–16 on Renesas, and 1–14/1–16 on ArduinoNRF. An unrecognized
Arduino core is kept at the portable 10-bit ADC and 8-bit duty interface; other widths
are rejected and no possibly-missing resolution setter is called. SPI transfers likewise
return `false` until `begin()` succeeds, including for otherwise valid buffers.

`ArduinoByteIo::writeAll()` is resumable despite its compatibility name: one call makes
exactly one underlying `Stream::write` attempt for at most 64 bytes and returns only the
accepted prefix. Call it again with the remaining suffix. A zero result means no progress;
an impossible over-report is rejected as zero. The wrapper itself has no dynamic storage,
but it cannot guarantee that a board core's `Stream` implementation never allocates or
blocks internally.

Repository-local use:

```cpp
#include <NobroRTOS.h>

nobro::NobroApp<3, 1> app;
auto motor = app.task("motor", nobro::hz(200), nobro::CONTROL);
auto imu = app.task("imu", nobro::hz(100));
app.wire(imu, motor, 8);
if (!app.admit()) Serial.println(app.errorText());
```

`NobroApp` is a fixed-capacity contract builder and admission preview with no dynamic
storage of its own; this is not a claim about memory or timing inside vendor provider
calls. Zero execution/resource budgets and arithmetic overflow are rejected fail-closed.
Production execution still uses generated/core firmware, so a passing preview is not
measured WCET evidence.

## Relationship to the full NobroRTOS repository

This repository is the Arduino-facing distribution, not a duplicate Rust source
tree. The full native kernel, ports, adapters, application compositions, generator,
and host tooling live in
<https://github.com/dunknowcoding/NobroRTOS>. Its canonical Arduino input is
`packages/arduino/`; releases copy that directory into this package-root repository.

Use the Arduino package for sketches, bounded contract declarations, report decoding,
and optional board-core provider wrappers. Use the main repository when generating or
building native NobroRTOS firmware, adding a port/adapter, or running the complete
validation matrix. See
<https://github.com/dunknowcoding/NobroRTOS/blob/master/docs/ARDUINO_PLATFORMIO.md>.

`python tools/package_arduino.py --check` verifies the vendored canonical
headers and license. `--zip` writes a self-contained installable archive under
the ignored `_work/` directory.
