/* softdevice-s140-v6 layout aligned with ArduinoNRF boards.txt:
 *   uf2_app_start = 0x26000, maximum_size = 798720 (ends @ 0xE9000)
 * RAM @ 0x20006000 matches nrf52840_s140_compat.ld
 */
MEMORY
{
  FLASH (rx)       : ORIGIN = 0x00026000, LENGTH = 782336
  APP_STORAGE (rw) : ORIGIN = 0x000E5000, LENGTH = 16K
  RAM (rwx)   : ORIGIN = 0x20006000, LENGTH = 0x3A000
}

NOBRO_APP_STORAGE_START = ORIGIN(APP_STORAGE);
NOBRO_APP_STORAGE_END = ORIGIN(APP_STORAGE) + LENGTH(APP_STORAGE);
