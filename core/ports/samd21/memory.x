/* ATSAMD21G18 (Arduino-Zero-class): app behind the 8 KB UF2/SAM-BA bootloader */
MEMORY
{
  FLASH : ORIGIN = 0x00002000, LENGTH = 232K
  APP_STORAGE : ORIGIN = 0x0003C000, LENGTH = 16K
  RAM   : ORIGIN = 0x20000000, LENGTH = 32K
}

__nobro_app_storage_start = ORIGIN(APP_STORAGE);
__nobro_app_storage_end = ORIGIN(APP_STORAGE) + LENGTH(APP_STORAGE);
