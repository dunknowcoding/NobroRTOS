# NobroRTOS PlatformIO Package

This folder contains the PlatformIO distribution surface.

Current contents:

- `library.json` for PlatformIO library metadata.
- `include/NobroRTOS.h` as a thin forwarding include to the canonical C ABI
  header.

The PlatformIO package should reuse the same contracts as the standalone SDK and
Arduino package.

Repository-local use:

```cpp
#include <NobroRTOS.h>
```

Release packaging should copy the canonical C/C++ binding headers into the
package archive while preserving the same public include names.
