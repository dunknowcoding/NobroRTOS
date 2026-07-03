// On-device vision analytics on the XIAO ESP32S3 Sense (M99/M105/M111). Captures
// GRAYSCALE QVGA, then ON THE ESP32:
//   M99  - average-pool to an 8x8 thumbnail (the preprocessing a tiny CNN consumes)
//   M105 - frame-diff motion trigger (mean abs diff of the thumbnail vs the previous)
//   M111 - image anomaly vs a running-average baseline thumbnail (L1 distance threshold)
// Emits one compact line per frame the NobroRTOS collector ingests, plus a self-test
// summary. No host-side image processing - all analytics run on-device.
//
// XIAO ESP32S3 Sense camera pin map (Seeed).
#include "esp_camera.h"

static const int XCLK = 10, SIOD = 40, SIOC = 39, VSYNC = 38, HREF = 47, PCLK = 13;
static const int D7 = 48, D6 = 11, D5 = 12, D4 = 14, D3 = 16, D2 = 18, D1 = 17, D0 = 15;

uint8_t thumb[64];
uint8_t prev[64];
int32_t baseline[64]; // running-average baseline (x256 fixed point)
bool have_prev = false;
bool have_base = false;
uint32_t frames = 0, motion_events = 0, anomaly_events = 0;

bool cameraInit() {
  camera_config_t c = {};
  c.ledc_channel = LEDC_CHANNEL_0;
  c.ledc_timer = LEDC_TIMER_0;
  c.pin_pwdn = -1; c.pin_reset = -1; c.pin_xclk = XCLK;
  c.pin_sccb_sda = SIOD; c.pin_sccb_scl = SIOC;
  c.pin_d7 = D7; c.pin_d6 = D6; c.pin_d5 = D5; c.pin_d4 = D4;
  c.pin_d3 = D3; c.pin_d2 = D2; c.pin_d1 = D1; c.pin_d0 = D0;
  c.pin_vsync = VSYNC; c.pin_href = HREF; c.pin_pclk = PCLK;
  c.xclk_freq_hz = 20000000;
  c.pixel_format = PIXFORMAT_GRAYSCALE; // pixels, not JPEG - decode-free on-device
  c.frame_size = FRAMESIZE_QVGA;        // 320x240
  c.fb_count = 2;
  c.fb_location = CAMERA_FB_IN_PSRAM;
  c.grab_mode = CAMERA_GRAB_WHEN_EMPTY;
  return esp_camera_init(&c) == ESP_OK;
}

// M99: average-pool a WxH grayscale frame into the 8x8 thumbnail.
void downscale(const uint8_t *buf, int w, int h) {
  int cw = w / 8, ch = h / 8;
  for (int ty = 0; ty < 8; ty++) {
    for (int tx = 0; tx < 8; tx++) {
      uint32_t acc = 0;
      for (int y = 0; y < ch; y++)
        for (int x = 0; x < cw; x++)
          acc += buf[(ty * ch + y) * w + (tx * cw + x)];
      thumb[ty * 8 + tx] = acc / (cw * ch);
    }
  }
}

void setup() {
  Serial.begin(115200);
  delay(1500);
  if (!cameraInit()) {
    Serial.println("VISION8 init=FAIL");
    return;
  }
  sensor_t *s = esp_camera_sensor_get();
  Serial.printf("VISION8 ready sensor=0x%x fmt=gray\n", s ? s->id.PID : 0);
  for (int i = 0; i < 5; i++) { // let AE settle
    camera_fb_t *fb = esp_camera_fb_get();
    if (fb) esp_camera_fb_return(fb);
    delay(80);
  }
}

void loop() {
  camera_fb_t *fb = esp_camera_fb_get();
  if (!fb) { Serial.println("VISION8 capture=FAIL"); delay(500); return; }
  downscale(fb->buf, fb->width, fb->height);
  int w = fb->width, h = fb->height;
  esp_camera_fb_return(fb);

  // M105: frame-diff motion
  uint32_t diff = 0;
  if (have_prev)
    for (int i = 0; i < 64; i++) diff += abs((int)thumb[i] - (int)prev[i]);
  uint32_t motion = have_prev ? (diff / 64) : 0;
  bool moved = motion > 8;

  // M111: anomaly vs running-average baseline (L1 distance)
  uint32_t adist = 0;
  if (have_base)
    for (int i = 0; i < 64; i++) adist += abs((int)thumb[i] - (baseline[i] >> 8));
  uint32_t anom = have_base ? (adist / 64) : 0;
  bool anomaly = have_base && anom > 20;

  // update baseline (EWMA, alpha=1/16) and prev
  for (int i = 0; i < 64; i++) {
    if (!have_base) baseline[i] = (int32_t)thumb[i] << 8;
    else baseline[i] += ((int32_t)((int)thumb[i] << 8) - baseline[i]) >> 4;
    prev[i] = thumb[i];
  }
  have_base = true; have_prev = true;

  uint32_t mean = 0;
  for (int i = 0; i < 64; i++) mean += thumb[i];
  mean /= 64;

  frames++;
  if (moved) motion_events++;
  if (anomaly) anomaly_events++;

  // emit the on-device analytics line (thumbnail as 64 hex bytes)
  Serial.printf("VISION8 res=%dx%d mean=%u motion=%u anomaly=%u moved=%d anom=%d thumb=",
                w, h, (unsigned)mean, (unsigned)motion, (unsigned)anom, moved ? 1 : 0,
                anomaly ? 1 : 0);
  for (int i = 0; i < 64; i++) Serial.printf("%02x", thumb[i]);
  Serial.println();

  // periodic self-test summary line the collector/tool can gate on
  if (frames % 10 == 0)
    Serial.printf("VISION8 SUMMARY frames=%u motion_events=%u anomaly_events=%u ready=1\n",
                  (unsigned)frames, (unsigned)motion_events, (unsigned)anomaly_events);
  delay(400);
}
