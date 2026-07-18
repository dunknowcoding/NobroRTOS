#include <NobroRTOS.h>

nobro::NobroApp<8, 8> app;

void setup() {
  Serial.begin(115200);
  nobro::TaskId motor = app.task("motor", nobro::hz(200), nobro::CONTROL);
  nobro::TaskId imu = app.task("imu", nobro::hz(100));
  nobro::TaskId camera = app.task("camera_ai", nobro::hz(25), nobro::SERVICE);
  nobro::TaskId radio = app.task("telemetry", nobro::hz(10), nobro::SERVICE);
  app.budget(camera, 4000).memory(camera, 16 * 1024, 8 * 1024);
  app.wire(imu, motor).wire(camera, radio);
  if (!app.admit()) Serial.println(app.errorText());
}

void loop() {}
