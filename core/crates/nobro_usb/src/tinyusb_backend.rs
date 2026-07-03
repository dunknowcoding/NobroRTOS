//! TinyUSB backend scaffold (mountable via `--features backend-tinyusb`). TinyUSB is a
//! C stack; wiring it means vendoring tinyusb + a `tusb_config.h` and calling its
//! `tud_*` API through FFI. This type gives the modular surface a real, compiling shape
//! today - `mount()` returns it, apps compile against it - while the FFI glue is a
//! documented follow-up. It reports Disconnected until the C stack is linked.

use crate::{backend_id, CdcState, UsbConfig, UsbStack};

pub struct TinyUsbCdc {
    _cfg: UsbConfig,
}

impl TinyUsbCdc {
    pub fn mount(cfg: &UsbConfig) -> Self {
        // TODO(backend): tusb_init(); configure the CDC class from `cfg`.
        TinyUsbCdc { _cfg: *cfg }
    }
}

impl UsbStack for TinyUsbCdc {
    fn poll(&mut self) -> CdcState {
        // TODO(backend): tud_task(); map tud_mounted()/tud_ready() -> CdcState.
        CdcState::Disconnected
    }
    fn write(&mut self, _data: &[u8]) -> usize {
        0 // TODO(backend): tud_cdc_write + tud_cdc_write_flush
    }
    fn read(&mut self, _buf: &mut [u8]) -> usize {
        0 // TODO(backend): tud_cdc_read
    }
    fn configured(&self) -> bool {
        false
    }
    fn backend_id(&self) -> u32 {
        backend_id::TINYUSB
    }
}
