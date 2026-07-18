# NobroRTOS PlatformIO Package

This folder contains the PlatformIO distribution surface.

Current contents:

- `library.json` for PlatformIO library metadata.
- `include/NobroRTOS.h` plus checked vendored C ABI headers, so a registry
  archive does not depend on the surrounding monorepo.
- the repository's noncommercial license.

The PlatformIO package should reuse the same contracts as the standalone SDK and
Arduino package.

Repository-local use:

```cpp
#include <NobroRTOS.h>
```

`python tools/package_arduino.py --check` verifies the vendored headers and
license. `python tools/check_distribution_artifacts.py` packs and clean-install
checks the package without publishing it.
