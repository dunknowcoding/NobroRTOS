# NobroRTOS Arduino Package

This folder contains the Arduino IDE library distribution surface.

Current contents:

- `library.properties` for Arduino Library Manager compatible metadata.
- `src/NobroRTOS.h` as a thin forwarding include to the canonical C ABI header.

The Arduino package should remain a thin compatibility surface over the core
contracts rather than a separate implementation.

Repository-local use:

```cpp
#include <NobroRTOS.h>
```

Release packaging should copy the canonical C/C++ binding headers into the
package archive while preserving the same public include names.
