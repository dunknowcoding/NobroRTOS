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
| PlatformIO Arduino project | `lib_deps = dunknowcoding/NobroRTOS@^0.3.1` | The same checked C/C++ facade in a self-contained registry archive |
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

## PlatformIO

```ini
[env:uno]
platform = atmelavr
board = uno
framework = arduino
lib_deps = dunknowcoding/NobroRTOS@^0.3.1
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
