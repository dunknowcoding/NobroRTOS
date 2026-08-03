SECTIONS
{
  .module_a_code ALIGN(256) :
  {
    __module_a_code_start = .;
    KEEP(*(.module_a_code));
    __module_a_code_end = .;
    . = __module_a_code_start + 256;
  } > FLASH

  .module_b_code ALIGN(256) :
  {
    __module_b_code_start = .;
    KEEP(*(.module_b_code));
    __module_b_code_end = .;
    . = __module_b_code_start + 256;
  } > FLASH
}
INSERT AFTER .text;

ASSERT(__module_a_code_end - __module_a_code_start <= 256,
       "module A code exceeds its MPU region");
ASSERT(__module_b_code_end - __module_b_code_start <= 256,
       "module B code exceeds its MPU region");
