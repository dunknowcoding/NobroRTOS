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
- `src/NobroNiusAudio.h` with a fixed-capacity ES8311 playback/capture queue,
  deadline accounting, backpressure, lifecycle, and recovery over the pinned
  NiusAudio library.
- `src/NobroEsp32Peripherals.h` with bounded continuous ADC/DMA frames, LEDC
  duty output, and RMT pulse symbols. Each provider is optional and keeps
  lifecycle, deadline, recovery, and vendor-resource ownership visible.
- `src/NobroArduinoWiFiS3.h` with an opt-in UNO R4 WiFi association facade,
  caller-sized scan output, runtime-only credentials, and explicit lifecycle.
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
facade policy accepts ADC/PWM widths of 10/8 bits on classic AVR, 1-16/1-14 on ESP32,
{8, 10, 12, 14, 16}/1-16 on Renesas, and 1-14/1-16 on ArduinoNRF. An unrecognized
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

## NiusAudio composition

Install NiusAudio 0.3.1 and include its facade explicitly:

```cpp
#include <NiusAudio.h>
#include <NobroRTOS.h>
#include <NobroNiusAudio.h>

NiusAudioWeActEs8311Board codec;
nobro::NiusEs8311AudioAdapter<2, 96> audio(codec);
```

The adapter stores exactly two frames of at most 96 signed 16-bit samples.
`submit()` rejects an oversized or full queue, `pump(max_block_us)` sends at
most one frame, and `capture(..., max_block_us)` records partial transfers and
deadline misses. NiusAudio and Arduino-ESP32 still own codec and I2S/DMA
implementation details; their runtime reservations are priced at the exact
board binding rather than hidden inside the portable contract. That binding
is limited to the shown 16 kHz mono signed-16 format, 96-sample frames, two
queue slots, and 100 capture/playback transfers per second; another shape
requires its own evidence and price.

## ESP32 continuous ADC, LEDC, and RMT

Include the optional facade only in ESP32 sketches:

```cpp
#include <NobroEsp32Peripherals.h>

nobro::Esp32ContinuousAdc<2> adc;
nobro::Esp32PersistentContinuousAdc<2, 32> sustainedAdc;
nobro::Esp32LedcPwm pwm(4);
nobro::Esp32RmtPulse<8> pulse(5);
```

The template capacities are the maximum retained pins, conversions per
channel, or pulse symbols, so RAM is visible at compile time. Calls reject
invalid shapes and report lifecycle, transport, partial-frame, and deadline
failures. `Esp32ContinuousAdc` is the compact Arduino convenience path.
`Esp32PersistentContinuousAdc` is preferable for sustained sampling: ESP-IDF
reads into its DMA-aligned object storage, so no heap allocation occurs per
frame. Both expose `alignedConversionsPerChannel()` and reject a request the
vendor core would silently widen, preserving averaging and deadline meaning.
Each sample contains the averaged raw code and the factory-calibrated
millivolt result from the pinned ESP-IDF calibration scheme.
Only one process-wide continuous ADC provider may be mounted at a time.

Three-run physical comparisons on C3 and P4 found zero ADC-specific transient
heap above the common measurement floor for the persistent path, versus
80/144 bytes for the convenience path. Worst active cycles/read improved by
about 39-40% with unchanged p99 latency. The persistent path traded
40/192 bytes of observed task-stack high-water; in an equivalent S3 build it
used 20,520 B flash / 456 B static RAM versus 21,108 B / 368 B for the compact
path. Interleaved ADC, LEDC, and RMT, quiescence/recovery/release, and exact
flash restoration passed on both physical targets. The exact S3 persistent
binding additionally passed 1,250 reads/s, ten recoveries, zero transient
heap, and concurrent physical ES8311 playback/capture. Its fixed/runtime
price is configuration-specific. Unreferenced ADC inputs prove transport and
calibrated conversion execution, not absolute voltage accuracy. Defining
`NOBRO_ESP32_PERIPHERALS_DISABLED`
before the include removes all three providers and their vendor symbols.

Use `quiesce()` when the configuration must remain recoverable. Use
`release()` to stop and deinitialize/detach the vendor engine, forget its
configuration, and return the provider to `Down`; configure it again before
reuse. This explicit split makes optional modules detachable without hiding
vendor resources behind object lifetime.

## UNO R4 WiFiS3 association

Include the optional facade only in an UNO R4 WiFi sketch:

```cpp
#include <NobroArduinoWiFiS3.h>

nobro::ArduinoWiFiS3Stack wifi;
```

`mount()`, `scan()`, `join()`, `poll()`, `leave()`, `quiesce()`, and
`recover()` keep association lifecycle explicit. `scan()` copies no more than
the caller-provided record capacity. `join()` accepts borrowed byte spans, so
credentials are never retained in the adapter. The Arduino Renesas WiFiS3
library remains authoritative for the board's UART/coprocessor protocol.

WiFiS3 internally uses dynamic strings and synchronous modem calls. The
facade reports a deadline miss after a call returns; it cannot preempt that
call. TCP/UDP clients and endpoints remain separate caller-owned objects.
Physical association, socket traffic, controller resources, and exact runtime
prices are not established by the current target-build gate. Define
`NOBRO_WIFI_S3_DISABLED` before including the facade to remove both Nobro and
WiFiS3 symbols from that composition.

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
headers and license. Release archives are generated from that verified package
surface and are not source-controlled.
