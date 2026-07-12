#include <NobroRTOS.h>

nobro::NobroApp<3, 1> app;

void setup() {
  Serial.begin(115200);
  nobro::TaskId motor = app.control("motor", 5);
  nobro::TaskId imu = app.sensor("imu", 10);
  app.connect(imu, motor);
  Serial.println(app.admit() ? "NobroRTOS app ready" : app.errorText());
}

void loop() {}
