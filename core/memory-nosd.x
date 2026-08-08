MEMORY
{
  /* ArduinoNRF's packaged no-SoftDevice recovery bootloader reserves
   * 0xE9000..0x100000 for bootloader/settings. Keep the application below
   * the same 950272-byte ceiling enforced by its boards.txt recipe. The final
   * 16 KiB is application-owned persistent storage and is deliberately not
   * part of the executable linker region.
   */
  FLASH (rx)       : ORIGIN = 0x00001000, LENGTH = 933888
  APP_STORAGE (rw) : ORIGIN = 0x000E5000, LENGTH = 16K
  RAM (rwx)   : ORIGIN = 0x20000000, LENGTH = 256K
}

NOBRO_APP_STORAGE_START = ORIGIN(APP_STORAGE);
NOBRO_APP_STORAGE_END = ORIGIN(APP_STORAGE) + LENGTH(APP_STORAGE);
