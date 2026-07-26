#include <NobroRTOS.h>

nobro::NobroApp<10, 12> app;

void setup() {
  Serial.begin(115200);
  nobro::TaskId actuator = app.task("actuator", nobro::hz(200), nobro::CONTROL);
  nobro::TaskId imu = app.task("imu", nobro::hz(100));
  nobro::TaskId vision = app.task("vision", nobro::hz(25), nobro::SERVICE);
  nobro::TaskId audio = app.task("audio", nobro::hz(100), nobro::SERVICE);
  nobro::TaskId rfid = app.task("rfid", nobro::hz(20));
  nobro::TaskId power = app.task("power", nobro::hz(10));
  nobro::TaskId radio = app.task("radio", nobro::hz(50), nobro::SERVICE);
  nobro::TaskId fusion = app.task("fusion", nobro::hz(100), nobro::CONTROL);
  nobro::TaskId gateway = app.task("gateway", nobro::hz(10), nobro::SERVICE);
  nobro::TaskId health = app.task("health", nobro::hz(5), nobro::SERVICE);

  app.budget(actuator, 800)
      .budget(vision, 6000)
      .budget(audio, 2500)
      .budget(fusion, 3000)
      .memory(vision, 20 * 1024, 4 * 1024)
      .memory(audio, 16 * 1024, 4 * 1024)
      .memory(radio, 12 * 1024, 4 * 1024)
      .memory(fusion, 8 * 1024, 2 * 1024);

  app.wire(imu, fusion, 8)
      .wire(vision, fusion, 2)
      .wire(audio, fusion, 4)
      .wire(rfid, fusion, 2)
      .wire(power, fusion, 2)
      .wire(radio, fusion, 8)
      .wire(fusion, actuator, 4)
      .wire(fusion, gateway, 4)
      .wire(power, actuator, 2)
      .wire(health, gateway, 2)
      .wire(radio, gateway, 8)
      .wire(actuator, health, 2);

  // This sketch previews the bounded contract. Generated NobroRTOS firmware
  // remains the one authoritative execution path for real task bodies.
  if (!app.admit()) {
    Serial.print(app.errorCode());
    Serial.print(": ");
    Serial.println(app.errorText());
  } else {
    Serial.println("robot-iot graph admitted");
  }
}

void loop() {}
