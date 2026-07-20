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
facade uses the platform's own WiFiS3 library for bounded association
lifecycle and caller-sized scan output. One exact WiFiS3 0.6.0 workload has
zero-disabled, physical DNS/TCP/lifecycle, and RA-side/controller-image price
evidence; controller-internal runtime resources, other firmware/workloads,
and complete shared-controller BLE pricing remain separate. The pinned
controller artifact records 1,180,064 B application flash / 64,628 B static
RAM and source minima of 22,288 B across three persistent tasks.

UNO R4 WiFi projects may include `NobroArduinoBLE.h` and declare ArduinoBLE
2.1.0 in `lib_deps`. The facade uses ArduinoBLE's official UNO R4
`HCIVirtualTransportAT` over the platform WiFiS3 modem and admits one
service, one characteristic, and 20-byte values. The facade adds bounded
`HCIEND`, cleared-service-retain repair, and provider disconnect around
ArduinoBLE 2.1.0. Exact physical evidence covers GATT write/read/notify,
disconnect, remount/recovery, and a subscribed BLE link across WiFiS3 DNS/TCP
traffic. Controller retained/transient heap, complete task/stack reservations,
CPU, and the complete shared-controller resource price remain unmeasured.

ESP32-family Arduino projects may include `NobroArduinoEspWiFi.h` directly.
It delegates to the selected platform's official `WiFi` stack and keeps
credentials runtime-only. ESP32/C3/S3 compilation, C3 zero-disabled linkage,
and exact C3 association/DNS/TCP/lifecycle evidence are present. One no-debug
C3 workload is completely priced at four HTTP transactions/s; other targets,
rates, socket workloads, and WiFi/BLE coexistence require separate evidence.

ESP32-family projects may also include `NobroArduinoEspBLE.h` without adding
an external BLE library: Arduino-ESP32 3.3.10 supplies Bluedroid on classic
ESP32 and NimBLE on ESP32-C3/S3. All three target profiles have exact
zero-disabled and enabled-build gates. Physical GATT/lifecycle behavior and
complete resource/coexistence prices remain unmeasured.

The selected PlatformIO platform/framework owns upload settings, bootloaders,
USB mode, pin routing, interrupts, and peripheral drivers. The C++ facade
validates a bounded declaration; native execution and target evidence remain in
the main NobroRTOS repository.

`python tools/package_arduino.py --check` verifies the vendored headers and
license. `python tools/check_distribution_artifacts.py` packs and clean-install
checks the package without publishing it.
