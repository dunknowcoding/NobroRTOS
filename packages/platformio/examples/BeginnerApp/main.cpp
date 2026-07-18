#include <Arduino.h>
#include <NobroRTOS.h>

nobro::NobroApp<3, 1> app;

void setup() {
    Serial.begin(115200);
    const nobro::TaskId motor = app.control("motor", 5);
    const nobro::TaskId imu = app.sensor("imu", 10);
    app.wire(imu, motor, 4);
    Serial.println(app.admit() ? "NobroRTOS app ready" : app.errorText());
}

void loop() {}
