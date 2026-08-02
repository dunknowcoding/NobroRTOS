#include <NiusDisplay.h>
#include <NobroNiusDisplay.h>

NiusTFT panel(ND_MOD_ST7789_240X240_1IN3, 10, 9, 8);
nobro::NiusDisplayAdapter<1> display(panel);
uint8_t pixel[2] = {0xf8, 0x00};

void setup() {
  Serial.begin(115200);
  if (!panel.begin() || display.begin() != NOBRO_DISPLAY_OK) {
    Serial.println("NOBRO:DISPLAY_INIT_ERROR");
    return;
  }
  const nobro_display_region_t region = {0, 0, 1, 1};
  nobro_display_receipt_t receipt = {};
  if (display.submit(region, pixel, sizeof pixel, micros() + 10000U, &receipt)
          == NOBRO_DISPLAY_OK) {
    display.pump();
  }
}

void loop() {}
