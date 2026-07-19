# NobroRTOS PlatformIO Package

This folder contains the self-contained PlatformIO Registry distribution.

Current contents:

- `library.json` for PlatformIO library metadata.
- `include/NobroRTOS.h`, the allocation-free C++ task/wire facade, optional
  Arduino provider wrappers, integration facades, and checked vendored C ABI
  headers.
- `examples/BeginnerApp/main.cpp` as a board-independent Arduino-framework
  admission example.
- the repository's noncommercial license.

## Install

After the registry package is available:

```ini
[env:uno]
platform = atmelavr
board = uno
framework = arduino
lib_deps = dunknowcoding/NobroRTOS@^0.3.2
monitor_speed = 115200
```

Or install the checked archive directly:

```bash
pio pkg install --library /absolute/path/NobroRTOS-PlatformIO-0.3.2.tar.gz
```

PlatformIO treats a relative archive argument as a possible VCS specification;
use an absolute path for a local package.

Application code:

```cpp
#include <Arduino.h>
#include <NobroRTOS.h>

nobro::NobroApp<3, 1> app;

void setup() {
  Serial.begin(115200);
  auto motor = app.control("motor", 5);
  auto imu = app.sensor("imu", 10);
  app.wire(imu, motor, 4);
  Serial.println(app.admit() ? "NobroRTOS app ready" : app.errorText());
}

void loop() {}
```

The base facade is dependency-free. For Arduino board-core providers, define
`NOBRO_ARDUINO_ENABLE_PROVIDERS` before including `NobroRTOS.h`; add
`NOBRO_ARDUINO_ENABLE_I2C` and/or `NOBRO_ARDUINO_ENABLE_SPI` only when needed.
External integration facades such as NiusIMU remain optional and require their
corresponding library in `lib_deps`.

ESP32 Arduino projects may include `NobroEsp32Peripherals.h` directly. Use
`Esp32ContinuousAdc` for the compact board-core path or
`Esp32PersistentContinuousAdc<Pins, Conversions>` for fixed DMA-aligned object
storage with no per-frame heap allocation. LEDC and RMT providers remain
independently optional. All capacities and the exact aligned conversion count
are explicit at compile time.

UNO R4 WiFi projects may include `NobroArduinoWiFiS3.h` directly. The
compile-only facade uses the platform's own WiFiS3 library for bounded
association lifecycle and caller-sized scan output; it does not promote
physical traffic or measured vendor resource bounds.

The selected PlatformIO platform/framework owns upload settings, bootloaders,
USB mode, pin routing, interrupts, and peripheral drivers. The C++ facade
validates a bounded declaration; native execution and target evidence remain in
the main NobroRTOS repository.

`python tools/package_arduino.py --check` verifies the vendored headers and
license. `python tools/check_distribution_artifacts.py` packs and clean-install
checks the package without publishing it.
