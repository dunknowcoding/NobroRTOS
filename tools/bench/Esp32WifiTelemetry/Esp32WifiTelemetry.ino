// ESP32 WiFi telemetry transport for the NobroRTOS collector (M95). Joins WiFi, reads an
// INA3221 power monitor over I2C, and streams JSONL samples over TCP to the collector's
// wifi_collector.py sink - the same schema the serial jsonl_bridge uses, now over WiFi.
//
// Fill in your own credentials + host below (do NOT commit real values). Wiring
// (ESP32-C3): INA3221 SDA=GPIO8, SCL=GPIO9.
//
// Hard-won reliability notes (both verified on real hardware):
//  - WPA2/WPA3 transition APs: build the station config by hand BEFORE esp_wifi_connect()
//    (setting it after WiFi.begin() fails with "sta is connecting, cannot set config"),
//    with H2E allowed - some APs reject hunt-and-peck-only SAE with reason=2 AUTH_EXPIRE.
//  - Many ESP32-C3 "SuperMini"-style boards have a mis-matched antenna: at the default
//    19.5 dBm TX power the AP cannot decode the board's auth frames, so scans see the AP
//    at strong RSSI yet every join dies in a reason=2 loop. Backing TX power off to
//    8.5 dBm fixes it. If your board joins fine at full power, remove that line.
#include <WiFi.h>
#include <Wire.h>
#include "esp_wifi.h"

const char *WIFI_SSID = "<YOUR_SSID>";
const char *WIFI_PASS = "<YOUR_WIFI_PASSWORD>";
const char *HOST_IP   = "<COLLECTOR_HOST_IP>";
const uint16_t HOST_PORT = 9099;

const int PIN_SDA = 8, PIN_SCL = 9;
const uint8_t INA_ADDR = 0x40;

uint16_t inaRead(uint8_t reg) {
  Wire.beginTransmission(INA_ADDR);
  Wire.write(reg);
  Wire.endTransmission(false);
  Wire.requestFrom(INA_ADDR, (uint8_t)2);
  uint16_t hi = Wire.available() ? Wire.read() : 0;
  uint16_t lo = Wire.available() ? Wire.read() : 0;
  return (hi << 8) | lo;
}

WiFiClient client;

void onWiFiEvent(WiFiEvent_t event, WiFiEventInfo_t info) {
  switch (event) {
    case ARDUINO_EVENT_WIFI_STA_DISCONNECTED:
      Serial.printf("EVENT disconnected reason=%d\n", info.wifi_sta_disconnected.reason);
      break;
    case ARDUINO_EVENT_WIFI_STA_CONNECTED:
      Serial.println("EVENT associated (L2)");
      break;
    case ARDUINO_EVENT_WIFI_STA_GOT_IP:
      Serial.printf("EVENT got_ip %s\n", WiFi.localIP().toString().c_str());
      break;
    default:
      break;
  }
}

void setup() {
  Serial.begin(115200);
  delay(800);
  Serial.println("\nNOBRO-WIFI-TELEMETRY boot");
  Wire.begin(PIN_SDA, PIN_SCL, 100000);
  WiFi.persistent(false);
  WiFi.mode(WIFI_STA);
  WiFi.setSleep(false);
  WiFi.onEvent(onWiFiEvent);

  Serial.printf("connecting to '%s'...\n", WIFI_SSID);
  wifi_config_t conf = {};
  strncpy((char *)conf.sta.ssid, WIFI_SSID, sizeof(conf.sta.ssid) - 1);
  strncpy((char *)conf.sta.password, WIFI_PASS, sizeof(conf.sta.password) - 1);
  conf.sta.threshold.authmode = WIFI_AUTH_WPA2_PSK;
  conf.sta.pmf_cfg.capable = true;
  conf.sta.pmf_cfg.required = false;
  conf.sta.sae_pwe_h2e = WPA3_SAE_PWE_BOTH;
  conf.sta.scan_method = WIFI_ALL_CHANNEL_SCAN;
  conf.sta.sort_method = WIFI_CONNECT_AP_BY_SIGNAL;
  esp_wifi_set_config(WIFI_IF_STA, &conf);
  WiFi.setTxPower(WIFI_POWER_8_5dBm); // see the antenna note in the header
  esp_wifi_connect();
}

uint32_t lastStatus = 0;

void loop() {
  wl_status_t st = WiFi.status();
  if (st != WL_CONNECTED) {
    if (millis() - lastStatus > 1000) {
      lastStatus = millis();
      Serial.printf("WIFI_DOWN status=%d rssi=%d\n", (int)st, WiFi.RSSI());
    }
    return;
  }

  static bool announced = false;
  if (!announced) {
    announced = true;
    Serial.printf("WIFI_OK ip=%s rssi=%d\n", WiFi.localIP().toString().c_str(), WiFi.RSSI());
  }

  if (!client.connected()) {
    if (!client.connect(HOST_IP, HOST_PORT)) {
      Serial.printf("TCP connect to %s:%u failed\n", HOST_IP, HOST_PORT);
      delay(2000);
      return;
    }
    Serial.println("TCP connected to collector");
  }
  // INA3221: bus reg 0x02/0x04/0x06 (8 mV/LSB, bits 15:3); shunt 0x01/0x03/0x05 (40 uV/LSB)
  String json = "{\"chip\":\"INA3221\",\"transport\":\"wifi\",\"channels\":[";
  for (int ch = 0; ch < 3; ch++) {
    int16_t bus = (int16_t)inaRead(0x02 + ch * 2);
    int16_t shunt = (int16_t)inaRead(0x01 + ch * 2);
    float busV = (bus >> 3) * 0.008f;
    float curA = ((shunt >> 3) * 40e-6f) / 0.1f; // 100 mOhm shunt
    json += "{\"bus_V\":" + String(busV, 3) + ",\"current_A\":" + String(curA, 4) + "}";
    if (ch < 2) json += ",";
  }
  json += "],\"rssi\":" + String(WiFi.RSSI()) + "}";
  client.println(json);
  Serial.println(json);
  delay(1000);
}
