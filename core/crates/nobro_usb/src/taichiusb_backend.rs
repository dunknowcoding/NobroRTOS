//! TaichiUSB backend scaffold (mountable via `--features backend-taichiusb`). TaichiUSB
//! is ArduinoNRF's native nRF52 USB stack (Layer 0). Wiring it means calling ArduinoNRF's
//! USB core (Adafruit-TinyUSB-derived) through a thin C ABI shim so a NobroRTOS app can
//! reuse the exact stack the Arduino core ships. This type provides the mountable surface
//! now; it reports Disconnected until the ArduinoNRF shim is linked.

use crate::{backend_id, CdcState, UsbConfig, UsbStack};

pub struct TaichiUsbCdc {
    _cfg: UsbConfig,
}

impl TaichiUsbCdc {
    pub fn mount(cfg: &UsbConfig) -> Self {
        // TODO(backend): call into ArduinoNRF's TaichiUSB begin() via the C ABI shim.
        TaichiUsbCdc { _cfg: *cfg }
    }
}

impl UsbStack for TaichiUsbCdc {
    fn poll(&mut self) -> CdcState {
        CdcState::Disconnected
    }
    fn write(&mut self, _data: &[u8]) -> usize {
        0
    }
    fn read(&mut self, _buf: &mut [u8]) -> usize {
        0
    }
    fn configured(&self) -> bool {
        false
    }
    fn backend_id(&self) -> u32 {
        backend_id::TAICHIUSB
    }
}
