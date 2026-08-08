MEMORY {
    /* Pico 2 W: 4 MB QSPI flash, 512 KB striped SRAM */
    FLASH : ORIGIN = 0x10000000, LENGTH = 4096K - 16K
    APP_STORAGE : ORIGIN = 0x103FC000, LENGTH = 16K
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
}

SECTIONS {
    /* RP2350 boot: the bootrom scans for the IMAGE_DEF block right after the vectors */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
    } > FLASH
} INSERT AFTER .vector_table;

_stext = ADDR(.start_block) + SIZEOF(.start_block);

SECTIONS {
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

__nobro_app_storage_start = ORIGIN(APP_STORAGE);
__nobro_app_storage_end = ORIGIN(APP_STORAGE) + LENGTH(APP_STORAGE);
