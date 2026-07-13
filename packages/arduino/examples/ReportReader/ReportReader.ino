// ReportReader - your first NobroRTOS sketch, no Rust toolchain required.
//
// NobroRTOS firmware self-certifies through fixed-layout NOBRO_* reports. This example
// uses the library's C ABI header to (1) build a sample report image the way device
// firmware does, (2) verify its checksum with the same nobro_report_checksum_words
// helper the host tools use, and (3) decode it field by field over Serial - the exact
// skills you need to read a live NobroRTOS node over a serial link or debug probe.
//
// It runs on ANY Arduino board (architectures=*): open the Serial Monitor at 115200.
#include <NobroRTOS.h>
#include <stdio.h>

// A miniature report in the standard shape: magic, version, completed, all_pass,
// payload words..., checksum-of-everything-before-it.
static uint32_t demo_report[] = {
    NOBRO_RUNTIME_REPORT_MAGIC, // magic ("NBRT")
    NOBRO_REPORT_VERSION,       // version
    1,                          // completed
    1,                          // all_pass
    13,                         // payload: subsystems checked
    0,                          // checksum (filled in setup)
};
static const size_t WORDS = sizeof(demo_report) / sizeof(demo_report[0]);

static void print_hex32(uint32_t v) {
  char buf[11];
  snprintf(buf, sizeof(buf), "0x%08lX", (unsigned long)v);
  Serial.print(buf);
}

void setup() {
  Serial.begin(115200);
  while (!Serial && millis() < 3000) {}

  // Seal the report exactly like device firmware: checksum over the preceding words.
  demo_report[WORDS - 1] = nobro_report_checksum_words(demo_report, WORDS - 1);

  Serial.println();
  Serial.println("NobroRTOS ReportReader - decoding a NOBRO_* report");
  Serial.println("--------------------------------------------------");
}

void loop() {
  // Decode pass: validate the public fixed-layout report contract.
  bool magic_ok = demo_report[0] == NOBRO_RUNTIME_REPORT_MAGIC;
  bool sum_ok =
      demo_report[WORDS - 1] == nobro_report_checksum_words(demo_report, WORDS - 1);
  bool pass = magic_ok && demo_report[2] == 1 && demo_report[3] == 1 && sum_ok;

  Serial.print("magic      = "); print_hex32(demo_report[0]);
  Serial.println(magic_ok ? "  (runtime report)" : "  (UNKNOWN)");
  Serial.print("version    = "); Serial.println(demo_report[1]);
  Serial.print("completed  = "); Serial.println(demo_report[2]);
  Serial.print("all_pass   = "); Serial.println(demo_report[3]);
  Serial.print("subsystems = "); Serial.println(demo_report[4]);
  Serial.print("checksum   = "); print_hex32(demo_report[WORDS - 1]);
  Serial.println(sum_ok ? "  (valid)" : "  (CORRUPT)");
  Serial.print("VERDICT    : "); Serial.println(pass ? "PASS" : "FAIL");
  Serial.println();
  delay(2000);
}
