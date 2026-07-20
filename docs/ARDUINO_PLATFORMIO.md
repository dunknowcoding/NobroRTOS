# Arduino and PlatformIO packages

NobroRTOS has one canonical C/C++ compatibility source and two package
distributions:

- `packages/arduino/` is the canonical Arduino package input.
- `NobroRTOS-Arduino` is the package-root repository consumed by Arduino
  Library Manager.
- `packages/platformio/` is the self-contained PlatformIO Registry package.

The release process copies the canonical Arduino directory to the dedicated
repository. It does not maintain an independent implementation there.
`tools/package_arduino.py --check` also requires the PlatformIO C++ facade and
vendored C ABI headers to match the Arduino/canonical sources byte-for-byte.

## Choose the surface

| Goal | Install | What it provides |
| --- | --- | --- |
| Arduino sketch | Arduino Library Manager: **NobroRTOS** | Fixed-capacity task/wire declaration, reports, optional Arduino provider wrappers and external-library facades |
| PlatformIO Arduino project | `lib_deps = dunknowcoding/NobroRTOS@^0.3.2` | The same checked C/C++ facade in a self-contained registry archive |
| Native NobroRTOS firmware | Clone the main repository | Rust kernel, admission, ports, adapters, firmware generation, target builds, and full validation |
| Python host workflow | `pip install nobro_rtos` | Dependency-free contracts, reports, simulations, project helpers, and CLI |

Arduino and PlatformIO board packages remain authoritative for board selection,
upload protocol, bootloader, USB mode, pins, interrupts, and vendor peripheral
drivers. The compatibility facade does not silently replace these settings.

## Arduino IDE

1. Install the exact board core through Boards Manager.
2. Install **NobroRTOS** through Library Manager.
3. Select the board and port.
4. Open **File > Examples > NobroRTOS > BeginnerApp**.

Providers are opt-in:

```cpp
#define NOBRO_ARDUINO_ENABLE_PROVIDERS
#define NOBRO_ARDUINO_ENABLE_I2C
#include <NobroRTOS.h>
```

Add `NOBRO_ARDUINO_ENABLE_SPI` only when used. Optional NiusIMU,
NiusWireless, and NiusCam facades require the matching library; they are not
forced dependencies of the base package.

UNO R4 WiFi uses its board package's WiFiS3 implementation through the
explicit `NobroArduinoWiFiS3.h` facade. It is not included by the base
package. The exact WiFiS3 0.6.0 binding has zero-disabled target proof and
state-restoring association, DNS, TCP, leave, quiesce, and recovery evidence.
One 25-HTTP-transaction/slice workload at one operation/s has an exact
RA-side/controller-image price. Controller-internal runtime resources, BLE
coexistence, other firmware versions, and other workloads remain separate.

UNO R4 BLE projects may include `NobroArduinoBLE.h` and add ArduinoBLE 2.1.0
as an explicit project dependency. The facade follows ArduinoBLE's official
`HCIVirtualTransportAT` into the installed board package's WiFiS3 modem,
admits one global stack/service/characteristic, and bounds values at 20
bytes. `NOBRO_ARDUINO_BLE_DISABLED` removes the facade and ArduinoBLE symbols.
The facade supplies bounded `HCIEND`, cleared-service-retain repair, and
provider disconnect for ArduinoBLE 2.1.0. BLE-only and WiFi+BLE images
target-build, and three exact physical cycles pass GATT write/read/notify,
disconnect, remount/recovery, stable RA-side heap, and a subscribed link across
WiFiS3 DNS/TCP traffic. Synchronous modem calls remain non-preemptible and the
complete shared-controller resource price remains unmeasured.

ESP32, ESP32-C3, and ESP32-S3 sketches use the installed Arduino-ESP32 board
package's official `WiFi` implementation through the explicit
`NobroArduinoEspWiFi.h` facade. It is also opt-in. The pinned 3.3.10 family
target gate proves C3 zero-disabled linkage and compiles all three targets.
The exact C3 composition also passed repeated isolated association, DNS, TCP,
leave, quiesce, and recovery. Its no-debug four-HTTP-operations/s workload has
a complete fixed/runtime resource price. ESP-IDF heap/task ownership remains
vendor managed; other workloads, boards, and WiFi/BLE coexistence need their
own prices.

The same installed package supplies its official `BLE` library through
`NobroArduinoEspBLE.h`; no separate NimBLE dependency is required. The
3.3.10 board configuration selects Bluedroid on classic ESP32 and NimBLE on
ESP32-C3/S3. `NOBRO_ESP_BLE_DISABLED` is byte-identical to the same-target
baseline for all three, and enabled images target-build. The current binding
is compile-only: physical GATT, recovery, vendor resources, and coexistence
are not inferred.

## PlatformIO

```ini
[env:uno]
platform = atmelavr
board = uno
framework = arduino
lib_deps = dunknowcoding/NobroRTOS@^0.3.2
monitor_speed = 115200
```

The same source works with another board by changing the environment's
`platform` and `board` to IDs supported by the installed PlatformIO platform.
Do not copy a machine-specific port name, upload port, or toolchain path into a
shared project.

## From a contract to native firmware

`NobroApp` in an Arduino/PlatformIO sketch is an allocation-free declaration
and admission preview. It is not a hidden Rust executor and does not turn
vendor calls into measured real-time operations. For native execution, export
or author the same task/wire graph in the main repository and run:

```bash
python sdk/cli/nobro.py firmware app.json --build
```

The generated firmware uses the selected NobroRTOS board profile, linker
layout, admission data, and native runtime. Review the board and resource
contract before flashing.

## Maintainer synchronization

From the main repository:

```bash
python tools/package_arduino.py --sync
python tools/package_arduino.py --check
python tools/check_distribution_artifacts.py
python tools/check_release_versions.py --release
```

Only the clean package-root contents are copied to `NobroRTOS-Arduino`.
Repository-private plans, hardware evidence, build directories, caches, local
ports, and machine-specific environment names must never enter either package.
