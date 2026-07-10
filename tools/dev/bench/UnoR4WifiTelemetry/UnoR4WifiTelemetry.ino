// Arduino UNO R4 WiFi telemetry transport for the NobroRTOS collector (M96). Joins WiFi
// via the board's ESP32-S3 co-processor (WiFiS3) and streams JSONL samples over TCP to
// wifi_collector.py - the same schema the ESP32 WiFi node and serial jsonl_bridge use,
// proving the WiFi transport is MCU-agnostic (Renesas RA4M1 / Cortex-M4 this time).
//
// Fill in your own credentials + host below (do NOT commit real values).
#include <WiFiS3.h>

const char *WIFI_SSID = "<YOUR_SSID>";
const char *WIFI_PASS = "<YOUR_WIFI_PASSWORD>";
const char *HOST_IP   = "<COLLECTOR_HOST_IP>";
const uint16_t HOST_PORT = 9099;

WiFiClient client;

void setup() {
  Serial.begin(115200);
  delay(1500);
  Serial.println("NOBRO-UNOR4-WIFI boot");
  int status = WL_IDLE_STATUS;
  for (int attempt = 0; attempt < 5 && status != WL_CONNECTED; attempt++) {
    Serial.print("joining WiFi... ");
    status = WiFi.begin(WIFI_SSID, WIFI_PASS); // blocks ~a few seconds per try
    Serial.println(status == WL_CONNECTED ? "ok" : "retry");
  }
  if (status == WL_CONNECTED) {
    Serial.print("WIFI_OK ip=");
    Serial.print(WiFi.localIP());
    Serial.print(" rssi=");
    Serial.println(WiFi.RSSI());
  } else {
    Serial.println("WIFI_FAIL");
  }
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) {
    Serial.println("WIFI_DOWN");
    WiFi.begin(WIFI_SSID, WIFI_PASS);
    delay(2000);
    return;
  }
  if (!client.connected()) {
    if (!client.connect(HOST_IP, HOST_PORT)) {
      Serial.println("TCP connect failed");
      delay(2000);
      return;
    }
    Serial.println("TCP connected to collector");
  }
  // Node telemetry: uptime, a raw analog channel, and link quality.
  String json = "{\"chip\":\"RA4M1\",\"transport\":\"wifi\",\"uptime_ms\":" + String(millis()) +
                ",\"a0_raw\":" + String(analogRead(A0)) +
                ",\"rssi\":" + String(WiFi.RSSI()) + "}";
  client.println(json);
  Serial.println(json);
  delay(1000);
}
