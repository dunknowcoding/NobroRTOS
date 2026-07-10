// M220 — RFID-RC522 hardware verifier on Arduino UNO R4 WiFi (NiusWireless backend).
//
// Wiring (software SPI on SDA/SCL pins):
//   RC522 SDA  -> SDA  (D18 / A4)   chip-select
//   RC522 SCK  -> SCL  (D19 / A5)   software SPI clock
//   RC522 MOSI -> D11
//   RC522 MISO -> D12
//   RC522 IRQ  -> D13               (input only; polling path used here)
//   RC522 RST  -> D10
//   RC522 3.3V -> 3.3V  (not 5 V)
//   RC522 GND  -> GND
//
// Bench fixture (optional): D5 drives A0 through a loopback jumper.
//
// Serial at 115200 on the stock ESP32-S3 USB bridge (typical COM23 after reset).
// Place a tag on the reader; the sketch prints:
//   M220 RESULT: PASS NiusWireless_RC522_UID
//
// One-command host gate:
//   python tools/m220_rfid_eval.py --port COM23
//
#include <NiusWireless.h>

static const uint8_t LOOPBACK_OUT_PIN = 5;  // D5 -> A0 fixture
static const uint8_t LOOPBACK_ADC_PIN = A0;

#define RC522_CS_PIN    SDA
#define RC522_RST_PIN   10
#define RC522_SCK_PIN   SCL
#define RC522_MOSI_PIN  11
#define RC522_MISO_PIN  12
#define RC522_IRQ_PIN   13

NiusRC522 rfid(RC522_CS_PIN, RC522_RST_PIN, RC522_SCK_PIN, RC522_MOSI_PIN, RC522_MISO_PIN);

static bool rc522Ready = false;

static void printUidBytes() {
  uint8_t uid[NIUS_UID_MAX_LEN];
  uint8_t len = 0;
  if (!rfid.getUIDBytes(uid, len)) {
    Serial.print(F("uid=unavailable"));
    return;
  }
  Serial.print(F("uid_len="));
  Serial.print(len);
  Serial.print(F(" uid="));
  for (uint8_t i = 0; i < len; ++i) {
    if (uid[i] < 0x10) Serial.print('0');
    Serial.print(uid[i], HEX);
  }
}

static void reportLoopback() {
  pinMode(LOOPBACK_OUT_PIN, OUTPUT);
  digitalWrite(LOOPBACK_OUT_PIN, HIGH);
  delay(5);
  int adc = analogRead(LOOPBACK_ADC_PIN);
  Serial.print(F("loopback_d5_a0_adc="));
  Serial.println(adc);
}

void setup() {
  pinMode(RC522_IRQ_PIN, INPUT);
  Serial.begin(115200);
  delay(1500);

  Serial.println(F("NOBRO-M220 NiusWireless RC522 verifier"));
  Serial.println(F("wiring=CS:SDA SCK:SCL MOSI:11 MISO:12 IRQ:13 RST:10 VCC:3V3 GND:GND"));
  reportLoopback();

  rc522Ready = rfid.begin();
  if (!rc522Ready) {
    Serial.println(F("M220 RESULT: FAIL rc522_not_found"));
  } else {
    Serial.print(F("rc522_version="));
    Serial.println(rfid.getVersion());
    rfid.setAntennaGain(NIUS_GAIN_48DB);
    Serial.print(F("antenna_gain=0x"));
    Serial.println(rfid.getAntennaGain(), HEX);
    Serial.println(F("M220 scan=waiting_for_tag"));
  }
}

void loop() {
  static bool passed = false;
  if (passed) {
    delay(1000);
    return;
  }
  if (!rc522Ready) {
    Serial.println(F("M220 RESULT: FAIL rc522_not_found"));
    delay(1000);
    return;
  }
  if (rfid.cardPresentWake()) {
    Serial.print(F("card_type="));
    Serial.print(rfid.getCardTypeName());
    Serial.print(' ');
    printUidBytes();
    Serial.println();
    Serial.println(F("M220 RESULT: PASS NiusWireless_RC522_UID"));
    rfid.halt();
    passed = true;
  } else {
    Serial.print(F("M220 scan=no_card_yet rc522_version="));
    Serial.println(rfid.getVersion());
    delay(500);
  }
}
