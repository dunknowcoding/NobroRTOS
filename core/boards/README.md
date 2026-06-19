# Board Packages

Future board packages should keep hardware facts in data first:

```text
boards/
  promicro-nrf52840/   memory.x, board.json, feature flags
  rp2040-pico/
  stm32-*/
```

Each board package should expose `BoardDesc` constants, boot memory layout,
critical pins, capacity budgets, and the platform feature expected by the HAL.
Application crates should select exactly one board feature.
