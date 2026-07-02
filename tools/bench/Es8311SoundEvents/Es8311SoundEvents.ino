// Sound-event classification with acoustic self-verification (M116) on on-device
// log-filterbank features (M114). The ES8311 node plays each event class through its
// own speaker - LOW BEEP (800 Hz bursts), HIGH ALARM (3 kHz warble), NOISE burst - while
// recording its mic, extracts 6-band log-energy features per frame (Goertzel filterbank,
// the embedded stand-in for log-mel), classifies with a nearest-centroid rule, and
// checks every played event is recognized and silence stays "quiet". No human needed.
//
// Wiring: same as Es8311AudioLoopback (ESP32-S3 UNO + WeAct ES8311/NS4150B, COM28).
#include <Wire.h>
#include <ESP_I2S.h>
#include <math.h>

const int PIN_SDA = 8, PIN_SCL = 9, PIN_MC = 10, PIN_SD = 3;
const int PIN_DI = 11, PIN_DO = 12, PIN_WS = 13, PIN_BCK = 14;
const uint32_t RATE = 16000;

I2SClass i2s;
uint8_t es_addr = 0x18;

bool esWrite(uint8_t reg, uint8_t val) {
  Wire.beginTransmission(es_addr);
  Wire.write(uint8_t(reg >> 8) ? 0 : reg); // 8-bit regs
  Wire.write(val);
  return Wire.endTransmission() == 0;
}

int esRead(uint8_t reg) {
  Wire.beginTransmission(es_addr);
  Wire.write(reg);
  if (Wire.endTransmission(false) != 0) return -1;
  if (Wire.requestFrom(es_addr, (uint8_t)1) != 1) return -1;
  return Wire.read();
}

void es8311Init() {
  esWrite(0x00, 0x1F); delay(20);
  esWrite(0x00, 0x00); delay(20);
  esWrite(0x00, 0x80); delay(20);
  esWrite(0x01, 0x3F | 0x80); // clocks on, MCLK from BCLK
  esWrite(0x02, (0 << 5) | (2 << 3));
  esWrite(0x03, 0x10);
  esWrite(0x04, 0x10);
  esWrite(0x05, 0x00);
  esWrite(0x06, 0x03);
  esWrite(0x07, 0x00);
  esWrite(0x08, 0xFF);
  esWrite(0x09, 0x0C);
  esWrite(0x0A, 0x0C);
  esWrite(0x0D, 0x01);
  esWrite(0x0E, 0x02);
  esWrite(0x12, 0x00);
  esWrite(0x13, 0x10);
  esWrite(0x1C, 0x6A);
  esWrite(0x37, 0x08);
  esWrite(0x14, 0x1A);
  esWrite(0x17, 0xC8);
  esWrite(0x32, 0xBF);
  esWrite(0x31, 0x00);
}

// ---- M114: 6-band log-energy filterbank (Goertzel bank over 256-sample frames) ----
const float BANDS_HZ[6] = {400, 800, 1500, 2200, 3000, 4200};
const int FRAME = 256;

void frameFeatures(const int16_t *x, float *feat) {
  for (int b = 0; b < 6; b++) {
    float w = 2.0f * PI * BANDS_HZ[b] / RATE, c = 2.0f * cosf(w);
    float s1 = 0, s2 = 0;
    for (int i = 0; i < FRAME; i++) {
      float s0 = x[i] + c * s1 - s2;
      s2 = s1;
      s1 = s0;
    }
    float p = fabsf(s1 * s1 + s2 * s2 - c * s1 * s2) / FRAME;
    feat[b] = log10f(p + 1.0f);
  }
}

// ---- M116: nearest-centroid classifier over normalized band shapes ----
// Event signatures (which bands dominate): LOW_BEEP -> band1 (800 Hz);
// HIGH_ALARM -> bands 4-5 (3 k / 4.2 k); NOISE -> flat spread; QUIET -> low energy.
const char *CLASSES[4] = {"quiet", "low_beep", "high_alarm", "noise"};

int classify(const float *feat) {
  float total = 0, mx = 0;
  for (int b = 0; b < 6; b++) {
    total += feat[b];
    if (feat[b] > mx) mx = feat[b];
  }
  if (mx < 3.0f) return 0; // quiet: no band has real energy
  // normalized shape
  float n[6];
  for (int b = 0; b < 6; b++) n[b] = feat[b] / total;
  float lowScore = n[0] + n[1];        // 400+800
  float highScore = n[4] + n[5];       // 3000+4200
  float spread = 0;                    // flatness -> noise
  for (int b = 0; b < 6; b++) {
    float d = n[b] - 1.0f / 6;
    spread += d * d;
  }
  if (spread < 0.004f) return 3;       // flat spectrum = noise
  return (lowScore > highScore) ? 1 : 2;
}

// ---- playback synthesis + duplex run ----
const int CHUNK = 256;
int32_t txbuf[CHUNK * 2];
int32_t rxbuf[CHUNK * 2];
int16_t mono[FRAME];
uint32_t noiseState = 0x12345678;

int16_t synth(int event, uint32_t t) {
  switch (event) {
    case 1: { // low beep: 800 Hz, 100 ms on / 100 ms off
      if ((t / 1600) % 2) return 0;
      return (int16_t)(11000.0f * sinf(2 * PI * 800.0f * t / RATE));
    }
    case 2: { // high alarm: warble 2.8-3.2 kHz... keep centered 3 kHz + 4.2 kHz mix
      float s = sinf(2 * PI * 3000.0f * t / RATE) + 0.6f * sinf(2 * PI * 4200.0f * t / RATE);
      return (int16_t)(7000.0f * s);
    }
    case 3: { // noise burst
      noiseState = noiseState * 1664525u + 1013904223u;
      return (int16_t)((int32_t)(noiseState >> 16) - 32768) / 4;
    }
    default:
      return 0;
  }
}

// play `event` while recording; returns the majority class over the captured frames
int playAndClassify(int event, int blocks) {
  static uint32_t t = 0;
  int votes[4] = {0, 0, 0, 0};
  for (int b = 0; b < blocks; b++) {
    for (int i = 0; i < CHUNK; i++) {
      int16_t s = synth(event, t++);
      txbuf[2 * i] = ((int32_t)s) << 16;
      txbuf[2 * i + 1] = ((int32_t)s) << 16;
    }
    i2s.write((uint8_t *)txbuf, sizeof(txbuf));
    int got = i2s.readBytes((char *)rxbuf, sizeof(rxbuf));
    int frames = got / 8;
    if (frames >= FRAME / CHUNK) { // one analysis frame per block here (256 = 256)
      for (int i = 0; i < FRAME && i < frames; i++)
        mono[i] = (int16_t)(rxbuf[2 * i] >> 16);
      float feat[6];
      frameFeatures(mono, feat);
      if (b >= 4) votes[classify(feat)]++; // skip the amp settle
    }
  }
  int best = 0;
  for (int c = 1; c < 4; c++)
    if (votes[c] > votes[best]) best = c;
  return best;
}

void setup() {
  Serial.begin(115200);
  delay(1500);
  pinMode(PIN_SD, OUTPUT);
  digitalWrite(PIN_SD, HIGH);
  pinMode(PIN_MC, OUTPUT);
  digitalWrite(PIN_MC, HIGH);
  Wire.begin(PIN_SDA, PIN_SCL, 100000);
  delay(50);
  int id1 = esRead(0xFD), id2 = esRead(0xFE);
  Serial.printf("ES8311 id=%02X%02X\n", id1, id2);
  if (id1 != 0x83) {
    Serial.println("SOUND RESULT: FAIL (codec not found)");
    return;
  }
  es8311Init();
  i2s.setPins(PIN_BCK, PIN_WS, PIN_DI, PIN_DO, -1);
  if (!i2s.begin(I2S_MODE_STD, RATE, I2S_DATA_BIT_WIDTH_32BIT, I2S_SLOT_MODE_STEREO)) {
    Serial.println("SOUND RESULT: FAIL (i2s)");
    return;
  }

  // Self-verifying matrix: play each class through the speaker, classify from the mic.
  const int expect[4] = {0, 1, 2, 3};
  const int blocks[4] = {30, 40, 40, 40}; // ~0.5-0.7 s each
  bool all = true;
  for (int e = 0; e < 4; e++) {
    int got = playAndClassify(expect[e], blocks[e]);
    bool ok = got == expect[e];
    all &= ok;
    Serial.printf("  played=%-10s heard=%-10s %s\n", CLASSES[expect[e]], CLASSES[got],
                  ok ? "OK" : "MISS");
    delay(150);
  }
  Serial.printf("SOUND RESULT: %s (4-class acoustic self-verification)\n",
                all ? "PASS" : "FAIL");
}

void loop() {
  Serial.println("NOBRO-AUDIO node=es8311 classes=4 ready=1");
  delay(2000);
}
