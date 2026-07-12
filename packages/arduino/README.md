# NobroRTOS Arduino Package

This folder contains the Arduino IDE library distribution surface.

Current contents:

- `library.properties` for Arduino Library Manager compatible metadata.
- `src/NobroRTOS.h` with an allocation-free `NobroApp` task/channel facade and the
  canonical report ABI.
- beginner, complex robot/IoT, and report-reader examples compile-gated across AVR,
  UNO R4/RA4M1, ESP32-S3, and ArduinoNRF in the repository toolchain.

The Arduino package should remain a thin compatibility surface over the core
contracts rather than a separate implementation.

Repository-local use:

```cpp
#include <NobroRTOS.h>

nobro::NobroApp<3, 1> app;
auto motor = app.control("motor", 5);
auto imu = app.sensor("imu", 10);
app.connect(imu, motor);
if (!app.admit()) Serial.println(app.errorText());
```

The facade is a fixed-capacity contract builder and admission preview; it does not hide
resource limits or allocate memory. Production execution still uses generated/core
firmware, so a passing preview is not measured WCET evidence.

Release packaging should copy the canonical C/C++ binding headers into the
package archive while preserving the same public include names.
