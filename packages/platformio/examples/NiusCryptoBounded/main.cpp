#include <Arduino.h>
#include <NiusCrypto.h>
#include <NobroNiusCrypto.h>

nobro::NiusCryptoAdapter crypto(Crypto);

void setup() {
  Serial.begin(115200);
  if (crypto.begin(128) != NOBRO_CRYPTO_OK) {
    Serial.println("NOBRO:CRYPTO_INIT_ERROR");
    return;
  }
  const uint8_t message[] = {'n', 'o', 'b', 'r', 'o'};
  uint8_t digest[NIUS_SHA256_BYTES] = {};
  nobro_crypto_receipt_t receipt = {};
  const uint32_t now = micros();
  if (crypto.sha256(message, sizeof message, digest, now, now + 100000U,
                    &receipt) == NOBRO_CRYPTO_OK) {
    Serial.println("NOBRO:CRYPTO_OK");
  }
}

void loop() {}
