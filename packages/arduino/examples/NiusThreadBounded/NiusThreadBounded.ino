#include <Thread.h>
#include <NobroNiusThread.h>

nobro::NiusThreadAdapter<> mesh(Thread);
const uint8_t network_key[16] = {
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
    0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff};

void setup() {
  Serial.begin(115200);
  if (mesh.begin("NobroMesh", 15, 0x4e42, network_key) != NOBRO_MESH_OK) {
    Serial.println("NOBRO:THREAD_INIT_ERROR");
  }
}

void loop() {
  mesh.process(micros() + 5000U);
}
