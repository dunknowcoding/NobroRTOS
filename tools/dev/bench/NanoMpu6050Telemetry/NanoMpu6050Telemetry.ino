// Bench telemetry node: Arduino Nano (AVR) + MPU6050-class IMU (SY-104 module, possibly
// an ICM-based clone) over I2C. Prints one parseable line per sample for the NobroRTOS
// multi-board collector:
//   MPU6050 who=0x68 ax=12 ay=-20 az=16400 mag=1002mg
// WHO_AM_I identifies the actual silicon: 0x68 = genuine MPU6050; other values (0x70,
// 0x98, 0x12, ...) indicate a clone/ICM die. Any nonzero/non-0xFF id + ~1 g magnitude at
// rest proves a live sensor.
#include <Wire.h>

const uint8_t ADDR = 0x68; // AD0 low
const float LSB_PER_G = 16384.0; // +/-2 g

uint8_t rd8(uint8_t reg) {
  Wire.beginTransmission(ADDR);
  Wire.write(reg);
  Wire.endTransmission(false);
  Wire.requestFrom(ADDR, (uint8_t)1);
  return Wire.available() ? Wire.read() : 0xFF;
}

void wr8(uint8_t reg, uint8_t val) {
  Wire.beginTransmission(ADDR);
  Wire.write(reg);
  Wire.write(val);
  Wire.endTransmission();
}

int16_t rd16(uint8_t regH) {
  Wire.beginTransmission(ADDR);
  Wire.write(regH);
  Wire.endTransmission(false);
  Wire.requestFrom(ADDR, (uint8_t)2);
  int16_t hi = Wire.available() ? Wire.read() : 0;
  int16_t lo = Wire.available() ? Wire.read() : 0;
  return (hi << 8) | (lo & 0xFF);
}

uint8_t who = 0xFF;

void setup() {
  Serial.begin(115200);
  Wire.begin();
  Wire.setClock(100000);
  delay(200);
  wr8(0x6B, 0x80); // PWR_MGMT_1: reset
  delay(100);
  wr8(0x6B, 0x00); // wake, internal clock
  wr8(0x1C, 0x00); // ACCEL_CONFIG: +/-2 g
  delay(50);
  who = rd8(0x75);
  Serial.print(F("MPU6050_BOOT who=0x"));
  Serial.println(who, HEX);
}

void loop() {
  int16_t ax = rd16(0x3B), ay = rd16(0x3D), az = rd16(0x3F);
  // integer magnitude in milli-g (avoid float sqrt cost: do it in float, AVR is idle)
  float g = sqrt((float)ax * ax + (float)ay * ay + (float)az * az) / LSB_PER_G;
  Serial.print(F("MPU6050 who=0x"));
  Serial.print(who, HEX);
  Serial.print(F(" ax="));
  Serial.print(ax);
  Serial.print(F(" ay="));
  Serial.print(ay);
  Serial.print(F(" az="));
  Serial.print(az);
  Serial.print(F(" mag="));
  Serial.print((long)(g * 1000));
  Serial.println(F("mg"));
  delay(500);
}
