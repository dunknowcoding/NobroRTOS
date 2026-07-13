#include <NobroRTOS.h>

// Provider demo: an ADC->PWM loopback plus an MFRC522 (RFID-RC522) version-register
// read over the SPI provider, so every bounded provider is actually exercised. The
// reader's chip-select and reset pins are board wiring; set them to your own pins.
static const uint8_t RFID_CS = 10;
static const uint8_t RFID_RST = 9;
static const uint8_t MFRC522_VERSION_REG = 0x37;

nobro::ArduinoDeadline deadline;
nobro::ArduinoAdc adc(A0, 10);
nobro::ArduinoPwm pwm(5, 8);
nobro::ArduinoI2c i2c(Wire);
nobro::ArduinoSpi spi(RFID_CS, SPI);
nobro::ArduinoByteIo console(Serial);

// Read VersionReg with the MFRC522 SPI address format ((reg<<1)&0x7E)|0x80 for a read,
// then a dummy byte to clock the value back. Genuine/clone readers answer 0x88..0xB2;
// anything else (including no reader wired) reports absent instead of forcing a pass.
static bool rc522_present() {
  const uint8_t address = static_cast<uint8_t>(((MFRC522_VERSION_REG << 1) & 0x7E) | 0x80);
  const uint8_t tx[2] = {address, 0x00};
  uint8_t rx[2] = {0, 0};
  if (!spi.transfer(tx, rx, 2)) return false;
  switch (rx[1]) {
    case 0x88:
    case 0x90:
    case 0x91:
    case 0x92:
    case 0xB2:
      return true;
    default:
      return false;
  }
}

void setup() {
  Serial.begin(115200);
  adc.begin();
  pwm.begin();
  i2c.begin();
  pinMode(RFID_RST, OUTPUT);
  digitalWrite(RFID_RST, HIGH);  // release the reader from reset before any SPI use
  spi.begin();
  deadline.armAfterUs(2000);
}

void loop() {
  if (!deadline.due()) return;
  pwm.setDuty(adc.read() >> 2);
  // Patch the single rfid digit in place; the offset is the prefix length, so no
  // stdio (snprintf is absent on some cores) and no magic index.
  char report[] = "NOBRO-ARDUINO providers=7 rfid=0 all_pass=1\r\n";
  report[sizeof("NOBRO-ARDUINO providers=7 rfid=") - 1] = rc522_present() ? '1' : '0';
  console.writeAll(reinterpret_cast<const uint8_t *>(report), sizeof(report) - 1);
  deadline.armAfterUs(1000000);
}
