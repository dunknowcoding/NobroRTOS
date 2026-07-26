MEMORY
{
  /* Debugger-launched image: code executes from RAM while NVMC updates only
     the four declared application scratch pages. */
  FLASH : ORIGIN = 0x20000000, LENGTH = 160K
  RAM   : ORIGIN = 0x20028000, LENGTH = 96K
}

_stack_start = ORIGIN(RAM) + LENGTH(RAM);
