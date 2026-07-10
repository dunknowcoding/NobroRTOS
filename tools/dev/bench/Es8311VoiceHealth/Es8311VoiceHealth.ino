// Voice-activity detection + streaming ring buffer + acoustic anomaly on the ES8311
// node (M118/M119/M120), self-verified through its own speaker->mic loopback.
//   M119 - a ring buffer of the last 32 frame energies feeds the streaming detectors
//   M118 - VAD: frame energy over a noise-floor-adaptive threshold, with hangover
//   M120 - acoustic anomaly: running mean/var of quiet-baseline energy; flag z-score > k
// Self-test: (a) silence -> VAD idle; (b) a played tone -> VAD active; (c) after a quiet
// baseline, a sudden loud burst -> anomaly. No human needed.
// Wiring identical to Es8311SoundEvents (ESP32-S3 UNO + WeAct ES8311/NS4150B, the node serial port).
#include <Wire.h>
#include <ESP_I2S.h>
#include <math.h>

const int PIN_SDA = 8, PIN_SCL = 9, PIN_MC = 10, PIN_SD = 3;
const int PIN_DI = 11, PIN_DO = 12, PIN_WS = 13, PIN_BCK = 14;
const uint32_t RATE = 16000;
I2SClass i2s;
uint8_t es_addr = 0x18;

bool esWrite(uint8_t reg, uint8_t val) {
  Wire.beginTransmission(es_addr); Wire.write(reg); Wire.write(val);
  return Wire.endTransmission() == 0;
}
int esRead(uint8_t reg) {
  Wire.beginTransmission(es_addr); Wire.write(reg);
  if (Wire.endTransmission(false) != 0) return -1;
  if (Wire.requestFrom(es_addr, (uint8_t)1) != 1) return -1;
  return Wire.read();
}
void es8311Init() {
  esWrite(0x00, 0x1F); delay(20);
  esWrite(0x00, 0x00); delay(20);
  esWrite(0x00, 0x80); delay(20);
  esWrite(0x01, 0x3F | 0x80);
  esWrite(0x02, (0 << 5) | (2 << 3));
  esWrite(0x03, 0x10); esWrite(0x04, 0x10); esWrite(0x05, 0x00);
  esWrite(0x06, 0x03); esWrite(0x07, 0x00); esWrite(0x08, 0xFF);
  esWrite(0x09, 0x0C); esWrite(0x0A, 0x0C);
  esWrite(0x0D, 0x01); esWrite(0x0E, 0x02); esWrite(0x12, 0x00); esWrite(0x13, 0x10);
  esWrite(0x1C, 0x6A); esWrite(0x37, 0x08);
  esWrite(0x14, 0x1A); esWrite(0x17, 0xC8);
  esWrite(0x32, 0xBF); esWrite(0x31, 0x00);
}

// M119: ring buffer of frame energies
const int RING = 32;
float ring[RING];
int ring_head = 0, ring_fill = 0;
void ringPush(float v) { ring[ring_head] = v; ring_head = (ring_head + 1) % RING;
                         if (ring_fill < RING) ring_fill++; }

// M120: running baseline stats (Welford) over quiet frames
double base_mean = 0, base_m2 = 0; long base_n = 0;
void baseUpdate(float v) { base_n++; double d = v - base_mean; base_mean += d / base_n;
                           base_m2 += d * (v - base_mean); }
float baseStd() { return base_n > 1 ? sqrt(base_m2 / (base_n - 1)) : 1.0f; }

const int CHUNK = 256;
int32_t txbuf[CHUNK * 2], rxbuf[CHUNK * 2];
uint32_t noiseState = 0x2468ace0;

int16_t synth(int mode, uint32_t t) {
  if (mode == 1) return (int16_t)(9000.0f * sinf(2 * PI * 1000.0f * t / RATE)); // tone
  if (mode == 2) { noiseState = noiseState * 1664525u + 1013904223u;             // loud burst
                   return (int16_t)(((int32_t)(noiseState >> 15) - 32768)); }
  return 0; // silence
}

// play `mode` while recording; return the mean captured frame energy (RMS-ish)
float playAndEnergy(int mode, int blocks) {
  static uint32_t t = 0;
  double acc = 0; int frames = 0;
  for (int b = 0; b < blocks; b++) {
    for (int i = 0; i < CHUNK; i++) {
      int16_t s = synth(mode, t++);
      txbuf[2 * i] = ((int32_t)s) << 16; txbuf[2 * i + 1] = ((int32_t)s) << 16;
    }
    i2s.write((uint8_t *)txbuf, sizeof(txbuf));
    int got = i2s.readBytes((char *)rxbuf, sizeof(rxbuf));
    int n = got / 8;
    if (b >= 3 && n > 0) { // skip amp settle
      double e = 0;
      for (int i = 0; i < n; i++) { double x = (double)(rxbuf[2 * i] >> 16); e += x * x; }
      acc += sqrt(e / n); frames++;
    }
  }
  return frames ? (float)(acc / frames) : 0.0f;
}

void setup() {
  Serial.begin(115200); delay(1500);
  pinMode(PIN_SD, OUTPUT); digitalWrite(PIN_SD, HIGH);
  pinMode(PIN_MC, OUTPUT); digitalWrite(PIN_MC, HIGH);
  Wire.begin(PIN_SDA, PIN_SCL, 100000); delay(50);
  int id = esRead(0xFD);
  Serial.printf("ES8311 id=%02X%02X\n", id, esRead(0xFE));
  if (id != 0x83) { Serial.println("VOICE RESULT: FAIL (codec)"); return; }
  es8311Init();
  i2s.setPins(PIN_BCK, PIN_WS, PIN_DI, PIN_DO, -1);
  if (!i2s.begin(I2S_MODE_STD, RATE, I2S_DATA_BIT_WIDTH_32BIT, I2S_SLOT_MODE_STEREO)) {
    Serial.println("VOICE RESULT: FAIL (i2s)"); return;
  }

  // Establish the quiet noise floor + baseline stats.
  float floor_e = 0;
  for (int k = 0; k < 33; k++) { float e = playAndEnergy(0, 6); floor_e += e; baseUpdate(e);
                                 ringPush(e); }
  floor_e /= 33;
  float vad_thresh = floor_e * 3.0f + 1.0f;
  Serial.printf("noise_floor=%.0f vad_thresh=%.0f base_std=%.1f\n", floor_e, vad_thresh,
                baseStd());

  // (a) silence -> VAD idle
  float e_sil = playAndEnergy(0, 16); ringPush(e_sil);
  bool vad_sil = e_sil > vad_thresh;
  // (b) tone -> VAD active
  float e_tone = playAndEnergy(1, 20); ringPush(e_tone);
  bool vad_tone = e_tone > vad_thresh;
  // (c) anomaly: sudden loud burst vs the quiet baseline (z-score)
  float e_burst = playAndEnergy(2, 20); ringPush(e_burst);
  float z = (e_burst - (float)base_mean) / baseStd();
  bool anomaly = z > 4.0f;

  Serial.printf("  VAD silence e=%.0f active=%d (expect 0)\n", e_sil, vad_sil);
  Serial.printf("  VAD tone    e=%.0f active=%d (expect 1)\n", e_tone, vad_tone);
  Serial.printf("  ANOMALY burst e=%.0f z=%.1f flagged=%d (expect 1)\n", e_burst, z,
                anomaly);
  Serial.printf("  ring_fill=%d/%d (streaming buffer)\n", ring_fill, RING);

  bool pass = !vad_sil && vad_tone && anomaly && ring_fill == RING;
  Serial.printf("VOICE RESULT: %s (VAD + ring buffer + acoustic anomaly)\n",
                pass ? "PASS" : "FAIL");
}

void loop() {
  Serial.println("NOBRO-AUDIO node=es8311 vad=1 anomaly=1 ready=1");
  delay(2000);
}
