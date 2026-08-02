#include <Arduino.h>
#include <RoboServo.h>
#include <NobroRoboServo.h>

RoboServo servo;
nobro::RoboServoAdapter actuator(servo);

void setup() {
  Serial.begin(115200);
  if (actuator.begin(5) != NOBRO_SERVO_OK) {
    Serial.println("NOBRO:SERVO_INIT_ERROR");
    return;
  }
  nobro_servo_receipt_t receipt = {};
  const uint32_t now = micros();
  if (actuator.command(0, 1500, now, now + 20000U, &receipt) == NOBRO_SERVO_OK) {
    Serial.println("NOBRO:SERVO_OK");
  }
}

void loop() {}
