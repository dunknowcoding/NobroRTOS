/* ATSAMD21G18 (Arduino-Zero-class): app behind the 8 KB UF2/SAM-BA bootloader */
MEMORY
{
  FLASH : ORIGIN = 0x00002000, LENGTH = 248K
  RAM   : ORIGIN = 0x20000000, LENGTH = 32K
}
