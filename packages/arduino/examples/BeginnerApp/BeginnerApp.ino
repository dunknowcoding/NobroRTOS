#include <NobroRTOS.h>

nobro::NobroApp<3, 1> app;

void setup() {
  Serial.begin(115200);
  nobro::TaskId motor = app.task("motor", nobro::hz(200), nobro::CONTROL);
  nobro::TaskId imu = app.task("imu", nobro::hz(100));
  app.wire(imu, motor);
  Serial.println(app.admit() ? "NobroRTOS app ready" : app.errorText());
}

void loop() {}
