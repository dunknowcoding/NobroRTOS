#include "FreeRTOS.h"
#include "task.h"

static void motor(void *arg) {
  for (;;) { vTaskDelay(pdMS_TO_TICKS(5)); }
}

static void imu(void *arg) {
  for (;;) { vTaskDelay(pdMS_TO_TICKS(10)); }
}

static void camera(void *arg) {
  for (;;) { vTaskDelay(pdMS_TO_TICKS(40)); }
}

int main(void) {
  xTaskCreate(motor, "motor", 256, NULL, 3, NULL);
  xTaskCreate(imu, "imu", 256, NULL, 2, NULL);
  xTaskCreate(camera, "camera", 256, NULL, 1, NULL);
  vTaskStartScheduler();
  for (;;) {}
}
