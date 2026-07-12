#include <NobroRTOS.h>

nobro::NobroApp<8, 8> app;

void setup() {
  Serial.begin(115200);
  nobro::TaskId motor = app.control("motor", 5);
  nobro::TaskId imu = app.sensor("imu", 10);
  nobro::TaskId camera = app.service("camera_ai", 40);
  nobro::TaskId radio = app.service("telemetry", 100);
  app.budget(camera, 4000).memory(camera, 16 * 1024, 8 * 1024);
  app.connect(imu, motor).connect(camera, radio);
  if (!app.admit()) Serial.println(app.errorText());
}

void loop() {}
