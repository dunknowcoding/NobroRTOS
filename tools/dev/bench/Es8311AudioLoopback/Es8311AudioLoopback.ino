// Bench audio node: AliExpress ESP32-S3 UNO + WeAct ES8311/NS4150B mono codec module
// (built-in mic + speaker). Autonomous acoustic-loopback self-test: play a 1 kHz tone on
// the speaker while recording the mic (full-duplex I2S), Goertzel-detect the tone in the
// recording, then repeat in silence - tone energy must collapse. Proves DAC -> amp ->
// speaker -> air -> mic -> ADC end to end without a human listener.
//
// Wiring (module -> ESP32S3 UNO): DI->11 (codec data in), DO->12 (codec data out),
// WS->13, BCK->14, MC->10 (mic control), SCL->9, SDA->8, SD->3 (amp/codec enable).
// No MCLK line: the ES8311 runs from BCLK (reg01 bit7), 16 kHz x 32-bit stereo slots
// -> BCLK = 1.024 MHz = 64*fs (a native coefficient-table entry).
#include <Wire.h>
#include <ESP_I2S.h>
#include <math.h>

const int PIN_SDA = 8, PIN_SCL = 9, PIN_MC = 10, PIN_SD = 3;
const int PIN_DI = 11, PIN_DO = 12, PIN_WS = 13, PIN_BCK = 14;
const uint32_t RATE = 16000;
const float TONE_HZ = 1000.0;

I2SClass i2s;
uint8_t es_addr = 0x18;

bool esWrite(uint8_t reg, uint8_t val) {
  Wire.beginTransmission(es_addr);
  Wire.write(reg);
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

bool es8311Init() {
  // reset
  esWrite(0x00, 0x1F); delay(20);
  esWrite(0x00, 0x00); delay(20);
  esWrite(0x00, 0x80); delay(20); // power on, slave mode
  // clocks: all on + MCLK from BCLK (bit7)
  esWrite(0x01, 0x3F | 0x80);
  // 64*fs coeff entry (mclk=1.024M, rate=16k): pre_div=1, pre_multi=x4
  esWrite(0x02, (0 << 5) | (2 << 3)); // (pre_div-1)<<5 | pre_multi<<3
  esWrite(0x03, 0x10);                // fs_mode<<6 | adc_osr
  esWrite(0x04, 0x10);                // dac_osr
  esWrite(0x05, 0x00);                // (adc_div-1)<<4 | (dac_div-1)
  esWrite(0x06, 0x03);                // bclk_div=4 (slave: informational)
  esWrite(0x07, 0x00);
  esWrite(0x08, 0xFF);                // lrck dividers
  // format: I2S Philips, 16-bit resolution on both SDP in/out
  esWrite(0x09, 0x0C);
  esWrite(0x0A, 0x0C);
  // analog power + paths
  esWrite(0x0D, 0x01); // power up analog
  esWrite(0x0E, 0x02); // PGA + ADC modulator
  esWrite(0x12, 0x00); // DAC power up
  esWrite(0x13, 0x10); // output to HP drive
  esWrite(0x1C, 0x6A); // ADC eq bypass, DC offset cancel
  esWrite(0x37, 0x08); // DAC eq bypass
  // mic: analog mic, max PGA
  esWrite(0x14, 0x1A);
  esWrite(0x17, 0xC8); // ADC volume
  esWrite(0x32, 0xBF); // DAC volume ~75%
  esWrite(0x31, 0x00); // unmute
  return true;
}

// Goertzel power of `freq` in mono samples.
float goertzel(const int16_t *x, int n, float freq) {
  float w = 2.0 * PI * freq / RATE, c = 2.0 * cos(w);
  float s0 = 0, s1 = 0, s2 = 0;
  for (int i = 0; i < n; i++) {
    s0 = x[i] + c * s1 - s2;
    s2 = s1;
    s1 = s0;
  }
  return sqrt(fabs(s1 * s1 + s2 * s2 - c * s1 * s2)) / n;
}

const int CHUNK = 256; // stereo frames per block
int32_t txbuf[CHUNK * 2];
int32_t rxbuf[CHUNK * 2];
int16_t mono[8192];

// run `blocks` of simultaneous play+record; tone on/off; return captured mono count
int duplexRun(int blocks, bool tone, float *rms) {
  static float phase = 0;
  int captured = 0;
  double acc = 0;
  for (int b = 0; b < blocks; b++) {
    for (int i = 0; i < CHUNK; i++) {
      int16_t s = 0;
      if (tone) {
        s = (int16_t)(12000.0 * sin(phase));
        phase += 2.0 * PI * TONE_HZ / RATE;
        if (phase > 2.0 * PI) phase -= 2.0 * PI;
      }
      txbuf[2 * i] = ((int32_t)s) << 16;     // left slot, audio in top 16 bits
      txbuf[2 * i + 1] = ((int32_t)s) << 16; // right slot
    }
    i2s.write((uint8_t *)txbuf, sizeof(txbuf));
    int got = i2s.readBytes((char *)rxbuf, sizeof(rxbuf));
    int frames = got / 8;
    for (int i = 0; i < frames && captured < 8192; i++) {
      int16_t m = (int16_t)(rxbuf[2 * i] >> 16); // mic on left slot
      mono[captured++] = m;
      acc += (double)m * m;
    }
  }
  *rms = captured ? sqrt(acc / captured) : 0;
  return captured;
}

void setup() {
  Serial.begin(115200);
  delay(1500);
  pinMode(PIN_SD, OUTPUT);
  digitalWrite(PIN_SD, HIGH); // enable amp/codec
  pinMode(PIN_MC, OUTPUT);
  digitalWrite(PIN_MC, HIGH); // mic on
  Wire.begin(PIN_SDA, PIN_SCL, 100000);
  delay(50);

  // find the codec (SD/CE strap decides 0x18 vs 0x19)
  int id1 = -1, id2 = -1;
  for (uint8_t a : {0x18, 0x19}) {
    es_addr = a;
    id1 = esRead(0xFD);
    id2 = esRead(0xFE);
    if (id1 == 0x83 && id2 == 0x11) break;
  }
  Serial.printf("ES8311 addr=0x%02X id=%02X%02X (expect 8311)\n", es_addr, id1, id2);
  if (id1 != 0x83) {
    Serial.println("AUDIO RESULT: FAIL (codec not found)");
    return;
  }

  es8311Init();

  i2s.setPins(PIN_BCK, PIN_WS, PIN_DI, PIN_DO, -1); // bclk, ws, dout(->DI), din(<-DO)
  if (!i2s.begin(I2S_MODE_STD, RATE, I2S_DATA_BIT_WIDTH_32BIT, I2S_SLOT_MODE_STEREO)) {
    Serial.println("AUDIO RESULT: FAIL (i2s begin)");
    return;
  }

  float rmsOn, rmsOff;
  duplexRun(20, true, &rmsOn); // warmup / amp settle
  duplexRun(60, true, &rmsOn);
  float onTone = goertzel(mono, 8192 < 60 * CHUNK ? 8192 : 60 * CHUNK, TONE_HZ);
  float onCtrl = goertzel(mono, 8192 < 60 * CHUNK ? 8192 : 60 * CHUNK, 3300.0);
  duplexRun(20, false, &rmsOff); // drain
  duplexRun(60, false, &rmsOff);
  float offTone = goertzel(mono, 8192 < 60 * CHUNK ? 8192 : 60 * CHUNK, TONE_HZ);

  float ratio = offTone > 0.01 ? onTone / offTone : onTone / 0.01;
  Serial.printf("mic rms: tone=%.1f silent=%.1f\n", rmsOn, rmsOff);
  Serial.printf("goertzel@1k: tone=%.2f silent=%.2f ctrl@3.3k=%.2f ratio=%.1fx\n",
                onTone, offTone, onCtrl, ratio);
  bool pass = onTone > 5.0 && ratio > 4.0 && onTone > 2.0 * onCtrl;
  Serial.printf("AUDIO RESULT: %s (loopback speaker->air->mic %s)\n",
                pass ? "PASS" : "FAIL", pass ? "detected" : "not detected");
}

void loop() { delay(2000); }
