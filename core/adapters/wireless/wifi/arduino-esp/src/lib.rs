#![no_std]

use nobro_wireless::{LinkDescriptor, Protocol, WifiCredentials};

/// Stable backend identifier shared with the Arduino facade and registry.
pub const BACKEND_ID: &str = "arduino-esp-wifi";
/// TCP payload MTU exposed by the station data plane.
pub const WIFI_TCP_MTU: u16 = 1460;

/// Return the fixed association/data-plane identity for admission.
pub const fn descriptor() -> LinkDescriptor {
    LinkDescriptor {
        name: BACKEND_ID,
        protocol: Protocol::WifiTcp,
        mtu: WIFI_TCP_MTU,
        requires_join: true,
        broadcast_only: false,
    }
}

/// Validate the bounded, runtime-only station credentials accepted by the facade.
///
/// Arduino-ESP32 passes separate C strings into ESP-IDF, so printable commas are
/// valid here. NUL and non-printable bytes remain rejected because the borrowed
/// length-delimited Nobro values must be copied into terminated stack buffers.
pub fn valid_credentials(credentials: WifiCredentials<'_>) -> bool {
    let ssid = credentials.ssid();
    let secret = credentials.secret();
    !ssid.is_empty()
        && ssid.len() <= 32
        && (secret.is_empty() || (8..=63).contains(&secret.len()))
        && ssid
            .iter()
            .chain(secret)
            .all(|byte| (32..=126).contains(byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_and_tcp_scoped() {
        let value = descriptor();
        assert_eq!(value.name, "arduino-esp-wifi");
        assert_eq!(value.protocol, Protocol::WifiTcp);
        assert_eq!(value.mtu, 1460);
        assert!(value.requires_join);
        assert!(!value.broadcast_only);
    }

    #[test]
    fn credentials_are_bounded_without_rejecting_valid_commas() {
        let valid = WifiCredentials::new(b"runtime,ssid", b"private,1").unwrap();
        assert!(valid_credentials(valid));

        let short = WifiCredentials::new(b"runtime", b"short").unwrap();
        assert!(!valid_credentials(short));

        let newline = WifiCredentials::new(b"bad\nname", b"private1").unwrap();
        assert!(!valid_credentials(newline));
    }
}
