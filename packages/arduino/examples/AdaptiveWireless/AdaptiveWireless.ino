#include <NobroRTOS.h>

// Pick a ready-made policy, then change only the choices your application needs.
// This example describes traffic; the selected WiFi/BLE/radio adapter owns I/O.
auto wirelessPolicy = nobro::WirelessPolicy::lowEnergy(2, 16, 50000);

nobro_wireless_send_result_t sendTelemetry(
    const uint8_t *, size_t, uint64_t, void *) {
  // Replace this with the selected Nobro WiFi/BLE/radio adapter call.
  return NOBRO_WIRELESS_SEND_OK;
}

void setup() {
  const uint64_t nowUs = 0;
  auto telemetry =
      nobro::WirelessMessage::bestEffort(nowUs, nowUs + 2000000).priority(2);

  // Invalid timing or zero-sized policy choices fail before a provider starts.
  if (!wirelessPolicy.valid() || !telemetry.valid()) {
    for (;;) {}
  }

  nobro::AdaptiveWirelessQueue<2, 16> queue(wirelessPolicy);
  const uint8_t payload[] = {0x4E, 0x42};
  auto ticket = queue.enqueueUrgentWithin(payload, sizeof(payload), nowUs, 20000);
  if (!ticket.valid ||
      queue.service(nowUs, sendTelemetry).kind != nobro::WIRELESS_DELIVERED) {
    for (;;) {}
  }
}

void loop() {}
