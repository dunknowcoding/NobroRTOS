#include <NobroEsp8266.h>
#include <NobroArduinoEsp8266WiFi.h>

nobro::Esp8266Deadline heartbeat;
nobro::Esp8266Gpio led(LED_BUILTIN);
nobro::ArduinoEsp8266WiFiStack wifi;

void setup() {
  Serial.begin(115200);
  led.begin(OUTPUT);
  heartbeat.armAfterUs(500000);
  wifi.mount();
}

void loop() {
  wifi.poll();
  if (heartbeat.due()) {
    static bool level = false;
    level = !level;
    led.write(level);
    heartbeat.armAfterUs(500000);
  }
  nobro::Esp8266Power::cooperativeIdle();
}
