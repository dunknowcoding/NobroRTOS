/*
 * Arduino compatibility include for NobroRTOS.
 *
 * The canonical C ABI header lives in bindings/c/include. This forwarding
 * header keeps the Arduino package thin while repository-local examples and
 * library consumers can include <NobroRTOS.h>.
 */

#ifndef NOBRO_RTOS_ARDUINO_H
#define NOBRO_RTOS_ARDUINO_H

#include "../../../bindings/c/include/nobro_rtos.h"

#endif /* NOBRO_RTOS_ARDUINO_H */
