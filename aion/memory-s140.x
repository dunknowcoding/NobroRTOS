/* softdevice-s140-v6 — ArduinoNRF boards.txt (nicenano S140 menu):
 *   uf2_app_start = 0x26000, maximum_size = 798720 (ends @ 0xE9000)
 * RAM @ 0x20006000 matches nrf52840_s140_compat.ld
 */
MEMORY
{
  FLASH (rx)  : ORIGIN = 0x00026000, LENGTH = 798720
  RAM (rwx)   : ORIGIN = 0x20006000, LENGTH = 0x3A000
}
