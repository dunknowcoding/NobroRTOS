MEMORY
{
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x4100
    APP_STORAGE : ORIGIN = 0x101FC000, LENGTH = 16K
    RAM   : ORIGIN = 0x20000000, LENGTH = 264K
}

EXTERN(BOOT2)

SECTIONS
{
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;

ASSERT(SIZEOF(.boot2) == 256, "RP2040 BOOT2 must be exactly 256 bytes");

__nobro_app_storage_start = ORIGIN(APP_STORAGE);
__nobro_app_storage_end = ORIGIN(APP_STORAGE) + LENGTH(APP_STORAGE);
