#![no_std]

use nobro_wireless::{LinkDescriptor, Protocol};

/// Stable identity shared with the Arduino and PlatformIO facades.
pub const BACKEND_ID: &str = "arduino-esp-ble";
pub const GATT_VALUE_BYTES: u16 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArduinoEspTarget {
    Esp32,
    Esp32C3,
    Esp32S3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorHost {
    Bluedroid,
    Nimble,
}

/// Return the host selected by the pinned Arduino-ESP32 3.3.10 board package.
pub const fn vendor_host(target: ArduinoEspTarget) -> VendorHost {
    match target {
        ArduinoEspTarget::Esp32 => VendorHost::Bluedroid,
        ArduinoEspTarget::Esp32C3 | ArduinoEspTarget::Esp32S3 => VendorHost::Nimble,
    }
}

pub const fn descriptor() -> LinkDescriptor {
    LinkDescriptor {
        name: BACKEND_ID,
        protocol: Protocol::Ble,
        mtu: GATT_VALUE_BYTES,
        requires_join: false,
        broadcast_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_bounded_and_ble_scoped() {
        let value = descriptor();
        assert_eq!(value.name, "arduino-esp-ble");
        assert_eq!(value.protocol, Protocol::Ble);
        assert_eq!(value.mtu, 20);
        assert!(!value.requires_join);
        assert!(!value.broadcast_only);
    }

    #[test]
    fn board_package_host_selection_is_explicit() {
        assert_eq!(vendor_host(ArduinoEspTarget::Esp32), VendorHost::Bluedroid);
        assert_eq!(vendor_host(ArduinoEspTarget::Esp32C3), VendorHost::Nimble);
        assert_eq!(vendor_host(ArduinoEspTarget::Esp32S3), VendorHost::Nimble);
    }
}
