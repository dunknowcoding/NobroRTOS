#include <CC2530Radio.h>
#include <NobroNiusZigbee.h>

CC2530Radio radio;
nobro::NiusCc2530Adapter zigbee(radio);

void setup() {
  Serial.begin(115200);
  if (!zigbee.begin(15)) {
    Serial.println("NOBRO:ZIGBEE_INIT_ERROR");
  }
}

void loop() {
  if (!zigbee.process(micros() + 5000U)) {
    zigbee.quiesce();
    zigbee.recover();
  }
}
