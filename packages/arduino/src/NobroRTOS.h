/*
 * Arduino compatibility include for NobroRTOS.
 *
 * The canonical C ABI headers live in bindings/c/include and are vendored into
 * this library by tools/package_arduino.py --sync (drift-gated in CI). This
 * header keeps the Arduino package thin while repository-local examples and
 * library consumers can include <NobroRTOS.h>.
 */

#ifndef NOBRO_RTOS_ARDUINO_H
#define NOBRO_RTOS_ARDUINO_H

#include "nobro_rtos.h"

#endif /* NOBRO_RTOS_ARDUINO_H */
