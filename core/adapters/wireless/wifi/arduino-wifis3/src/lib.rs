//! Bounded bridge for the Arduino UNO R4 WiFiS3 stack.
//!
//! Nobro owns lifecycle validation, caller-sized scan results, runtime-only
//! credentials, and deadline accounting. The Arduino Renesas core and its
//! coprocessor firmware own the synchronous AT transport, IP stack, sockets,
//! heap, and controller resources.
#![no_std]

use nobro_wireless::{
    LinkDescriptor, LinkState, Protocol, StackError, StackFamily, StackIdentity, StackState,
    WifiCredentials, WifiNetwork, WifiStack, WirelessBackend,
};

pub const BACKEND_ID: &str = "arduino-wifis3";
pub const WIFI_TCP_MTU: u16 = 1460;
pub const CONTROL_QUEUE_SLOTS: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    InvalidConfig,
    Busy,
    Rejected,
    Fault,
}

impl TransportError {
    const fn stack_error(self) -> StackError {
        match self {
            Self::InvalidConfig => StackError::InvalidConfig,
            Self::Busy => StackError::Busy,
            Self::Rejected => StackError::AssociationRejected,
            Self::Fault => StackError::BackendFault,
        }
    }
}

/// Result of one synchronous WiFiS3 association call.
///
/// `elapsed_us` is checked after the vendor call returns. It is evidence of a
/// deadline miss, not a claim that the vendor call can be preempted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedJoin {
    pub elapsed_us: u64,
    pub result: Result<(), TransportError>,
}

pub trait ArduinoWifiS3Transport: WirelessBackend {
    fn mount(&mut self) -> Result<(), TransportError>;
    fn scan(&mut self, results: &mut [WifiNetwork]) -> Result<usize, TransportError>;
    fn join(&mut self, ssid: &[u8], secret: &[u8], timeout_us: u64) -> TimedJoin;
    fn leave(&mut self) -> Result<(), TransportError>;
    fn quiesce(&mut self) -> Result<(), TransportError>;
    fn recover_stack(&mut self) -> Result<(), TransportError>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WifiS3Diagnostics {
    pub scans: u32,
    pub scan_results: u32,
    pub join_attempts: u32,
    pub join_failures: u32,
    pub deadline_misses: u32,
    pub leaves: u32,
    pub recoveries: u32,
    pub transport_faults: u32,
}

pub struct ArduinoWifiS3<T> {
    transport: T,
    state: StackState,
    diagnostics: WifiS3Diagnostics,
}

impl<T> ArduinoWifiS3<T> {
    pub const fn new(transport: T) -> Self {
        Self {
            transport,
            state: StackState::Down,
            diagnostics: WifiS3Diagnostics {
                scans: 0,
                scan_results: 0,
                join_attempts: 0,
                join_failures: 0,
                deadline_misses: 0,
                leaves: 0,
                recoveries: 0,
                transport_faults: 0,
            },
        }
    }

    pub const fn diagnostics(&self) -> WifiS3Diagnostics {
        self.diagnostics
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    const fn identity() -> StackIdentity {
        StackIdentity {
            backend_id: BACKEND_ID,
            family: StackFamily::Wifi,
            mtu: WIFI_TCP_MTU,
            // The bridge admits one synchronous control/data operation at a
            // time and retains no hidden Nobro queue.
            rx_queue_slots: CONTROL_QUEUE_SLOTS,
            tx_queue_slots: CONTROL_QUEUE_SLOTS,
            service_slots: 0,
            characteristic_slots: 0,
        }
    }

    fn transport_fault(&mut self, error: TransportError) -> StackError {
        self.diagnostics.transport_faults = self.diagnostics.transport_faults.saturating_add(1);
        self.state = StackState::Faulted;
        error.stack_error()
    }

    fn transport_error(&mut self, error: TransportError) -> StackError {
        if error == TransportError::Fault {
            self.transport_fault(error)
        } else {
            self.state = StackState::Ready;
            error.stack_error()
        }
    }

    fn valid_credentials(credentials: WifiCredentials<'_>) -> bool {
        fn valid_text(bytes: &[u8]) -> bool {
            bytes
                .iter()
                .all(|byte| (32..=126).contains(byte) && *byte != b',')
        }

        valid_text(credentials.ssid())
            && (credentials.secret().is_empty()
                || ((8..=63).contains(&credentials.secret().len())
                    && valid_text(credentials.secret())))
    }
}

impl<T: ArduinoWifiS3Transport> WirelessBackend for ArduinoWifiS3<T> {
    fn descriptor(&self) -> LinkDescriptor {
        self.transport.descriptor()
    }

    fn link_state(&mut self) -> LinkState {
        self.transport.link_state()
    }

    fn send(&mut self, payload: &[u8]) -> bool {
        self.transport.send(payload)
    }

    fn recv(&mut self, buf: &mut [u8]) -> usize {
        self.transport.recv(buf)
    }

    fn recover(&mut self) -> bool {
        self.recover_stack().is_ok()
    }
}

impl<T: ArduinoWifiS3Transport> WifiStack for ArduinoWifiS3<T> {
    fn stack_identity(&self) -> StackIdentity {
        Self::identity()
    }

    fn stack_state(&mut self) -> StackState {
        self.state
    }

    fn mount_stack(&mut self) -> Result<(), StackError> {
        if self.state != StackState::Down && self.state != StackState::Quiesced {
            return Err(StackError::Busy);
        }
        let descriptor = self.transport.descriptor();
        if descriptor.protocol != Protocol::WifiTcp || descriptor.mtu != WIFI_TCP_MTU {
            return Err(StackError::InvalidIdentity);
        }
        self.state = StackState::Starting;
        if let Err(error) = self.transport.mount() {
            return Err(self.transport_fault(error));
        }
        self.state = StackState::Ready;
        Ok(())
    }

    fn scan(&mut self, results: &mut [WifiNetwork]) -> Result<usize, StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        let count = self
            .transport
            .scan(results)
            .map_err(|error| self.transport_fault(error))?;
        if count > results.len() {
            return Err(self.transport_fault(TransportError::Fault));
        }
        self.diagnostics.scans = self.diagnostics.scans.saturating_add(1);
        self.diagnostics.scan_results = self.diagnostics.scan_results.saturating_add(count as u32);
        Ok(count)
    }

    fn join(
        &mut self,
        credentials: WifiCredentials<'_>,
        now_us: u64,
        deadline_us: u64,
    ) -> Result<(), StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        if !Self::valid_credentials(credentials) {
            return Err(StackError::InvalidConfig);
        }
        let timeout_us = deadline_us
            .checked_sub(now_us)
            .filter(|remaining| *remaining != 0)
            .ok_or(StackError::DeadlineElapsed)?;
        self.diagnostics.join_attempts = self.diagnostics.join_attempts.saturating_add(1);
        self.state = StackState::Starting;
        let outcome = self
            .transport
            .join(credentials.ssid(), credentials.secret(), timeout_us);
        if outcome.elapsed_us > timeout_us {
            let _ = self.transport.leave();
            self.state = StackState::Ready;
            self.diagnostics.deadline_misses = self.diagnostics.deadline_misses.saturating_add(1);
            self.diagnostics.join_failures = self.diagnostics.join_failures.saturating_add(1);
            return Err(StackError::DeadlineElapsed);
        }
        if let Err(error) = outcome.result {
            self.diagnostics.join_failures = self.diagnostics.join_failures.saturating_add(1);
            return Err(self.transport_error(error));
        }
        if self.transport.link_state() != LinkState::Up {
            self.diagnostics.join_failures = self.diagnostics.join_failures.saturating_add(1);
            return Err(self.transport_fault(TransportError::Fault));
        }
        self.state = StackState::Ready;
        Ok(())
    }

    fn leave(&mut self) -> Result<(), StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        self.transport
            .leave()
            .map_err(|error| self.transport_fault(error))?;
        self.diagnostics.leaves = self.diagnostics.leaves.saturating_add(1);
        Ok(())
    }

    fn quiesce_stack(&mut self) -> Result<(), StackError> {
        if self.state == StackState::Down || self.state == StackState::Quiesced {
            self.state = StackState::Quiesced;
            return Ok(());
        }
        self.transport
            .quiesce()
            .map_err(|error| self.transport_fault(error))?;
        self.state = StackState::Quiesced;
        Ok(())
    }

    fn recover_stack(&mut self) -> Result<(), StackError> {
        self.state = StackState::Starting;
        self.transport
            .recover_stack()
            .map_err(|error| self.transport_fault(error))?;
        self.state = StackState::Ready;
        self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_wireless::{MountedWifi, StackError};

    struct Fake {
        link: LinkState,
        fail: bool,
        elapsed_us: u64,
        hostile_scan_count: bool,
        left: bool,
        rejected: bool,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                link: LinkState::Down,
                fail: false,
                elapsed_us: 10,
                hostile_scan_count: false,
                left: false,
                rejected: false,
            }
        }
    }

    impl WirelessBackend for Fake {
        fn descriptor(&self) -> LinkDescriptor {
            LinkDescriptor {
                name: "test WiFiS3 TCP",
                protocol: Protocol::WifiTcp,
                mtu: WIFI_TCP_MTU,
                requires_join: true,
                broadcast_only: false,
            }
        }

        fn link_state(&mut self) -> LinkState {
            self.link
        }

        fn send(&mut self, payload: &[u8]) -> bool {
            self.link == LinkState::Up && payload.len() <= usize::from(WIFI_TCP_MTU)
        }

        fn recv(&mut self, _buf: &mut [u8]) -> usize {
            0
        }
    }

    impl ArduinoWifiS3Transport for Fake {
        fn mount(&mut self) -> Result<(), TransportError> {
            if self.fail {
                Err(TransportError::Fault)
            } else {
                Ok(())
            }
        }

        fn scan(&mut self, results: &mut [WifiNetwork]) -> Result<usize, TransportError> {
            if self.fail {
                return Err(TransportError::Fault);
            }
            if self.hostile_scan_count {
                return Ok(results.len() + 1);
            }
            if let Some(first) = results.first_mut() {
                first.set_ssid(b"bounded").unwrap();
                first.channel = 6;
                first.rssi_dbm = -42;
                first.secured = true;
                Ok(1)
            } else {
                Ok(0)
            }
        }

        fn join(&mut self, _ssid: &[u8], _secret: &[u8], _timeout_us: u64) -> TimedJoin {
            if self.fail {
                return TimedJoin {
                    elapsed_us: self.elapsed_us,
                    result: Err(TransportError::Fault),
                };
            }
            if self.rejected {
                return TimedJoin {
                    elapsed_us: self.elapsed_us,
                    result: Err(TransportError::Rejected),
                };
            }
            self.link = LinkState::Up;
            TimedJoin {
                elapsed_us: self.elapsed_us,
                result: Ok(()),
            }
        }

        fn leave(&mut self) -> Result<(), TransportError> {
            self.left = true;
            self.link = LinkState::Down;
            Ok(())
        }

        fn quiesce(&mut self) -> Result<(), TransportError> {
            self.link = LinkState::Down;
            Ok(())
        }

        fn recover_stack(&mut self) -> Result<(), TransportError> {
            self.fail = false;
            self.link = LinkState::Down;
            Ok(())
        }
    }

    fn mount(fake: Fake) -> MountedWifi<ArduinoWifiS3<Fake>> {
        match MountedWifi::mount(ArduinoWifiS3::new(fake)) {
            Ok(mounted) => mounted,
            Err(_) => panic!("test WiFiS3 adapter must mount"),
        }
    }

    #[test]
    fn owned_mount_scan_join_leave_and_recovery_are_bounded() {
        let mut mounted = mount(Fake::default());
        let mut networks = [WifiNetwork::empty(); 2];
        assert_eq!(mounted.scan(&mut networks), Ok(1));
        assert_eq!(networks[0].ssid(), b"bounded");
        let credentials = WifiCredentials::new(b"runtime", b"private1").unwrap();
        assert_eq!(mounted.join(credentials, 100, 1_000), Ok(()));
        assert_eq!(mounted.leave(), Ok(()));
        assert_eq!(mounted.quiesce(), Ok(()));
        assert_eq!(mounted.recover(), Ok(()));
        let adapter = mounted.into_backend();
        assert_eq!(
            adapter.diagnostics(),
            WifiS3Diagnostics {
                scans: 1,
                scan_results: 1,
                join_attempts: 1,
                join_failures: 0,
                deadline_misses: 0,
                leaves: 1,
                recoveries: 1,
                transport_faults: 0,
            }
        );
    }

    #[test]
    fn synchronous_overrun_is_reported_and_disconnects() {
        let mut fake = Fake::default();
        fake.elapsed_us = 901;
        let mut mounted = mount(fake);
        let credentials = WifiCredentials::new(b"runtime", b"private1").unwrap();
        assert_eq!(
            mounted.join(credentials, 100, 1_000),
            Err(StackError::DeadlineElapsed)
        );
        let adapter = mounted.into_backend();
        assert!(adapter.transport.left);
        assert_eq!(adapter.state, StackState::Ready);
        assert_eq!(adapter.diagnostics.deadline_misses, 1);
    }

    #[test]
    fn credentials_reject_at_command_truncation_and_injection() {
        let mut mounted = mount(Fake::default());
        let short = WifiCredentials::new(b"runtime", b"short").unwrap();
        assert_eq!(mounted.join(short, 0, 100), Err(StackError::InvalidConfig));
        let comma = WifiCredentials::new(b"bad,name", b"private1").unwrap();
        assert_eq!(mounted.join(comma, 0, 100), Err(StackError::InvalidConfig));
        let newline = WifiCredentials::new(b"bad\nname", b"private1").unwrap();
        assert_eq!(
            mounted.join(newline, 0, 100),
            Err(StackError::InvalidConfig)
        );
    }

    #[test]
    fn association_rejection_does_not_fault_the_stack() {
        let mut fake = Fake::default();
        fake.rejected = true;
        let mut mounted = mount(fake);
        let credentials = WifiCredentials::new(b"runtime", b"private1").unwrap();
        assert_eq!(
            mounted.join(credentials, 0, 100),
            Err(StackError::AssociationRejected)
        );
        assert_eq!(mounted.state(), StackState::Ready);
    }

    #[test]
    fn hostile_scan_count_faults_the_stack() {
        let mut fake = Fake::default();
        fake.hostile_scan_count = true;
        let mut mounted = mount(fake);
        let mut networks = [WifiNetwork::empty(); 1];
        assert_eq!(mounted.scan(&mut networks), Err(StackError::BackendFault));
        assert_eq!(mounted.state(), StackState::Faulted);
    }

    #[test]
    fn wrong_data_plane_identity_is_rejected() {
        struct Wrong(Fake);
        impl WirelessBackend for Wrong {
            fn descriptor(&self) -> LinkDescriptor {
                LinkDescriptor {
                    name: "wrong",
                    protocol: Protocol::Ble,
                    mtu: WIFI_TCP_MTU,
                    requires_join: false,
                    broadcast_only: false,
                }
            }
            fn link_state(&mut self) -> LinkState {
                self.0.link_state()
            }
            fn send(&mut self, payload: &[u8]) -> bool {
                self.0.send(payload)
            }
            fn recv(&mut self, buf: &mut [u8]) -> usize {
                self.0.recv(buf)
            }
        }
        impl ArduinoWifiS3Transport for Wrong {
            fn mount(&mut self) -> Result<(), TransportError> {
                Ok(())
            }
            fn scan(&mut self, _: &mut [WifiNetwork]) -> Result<usize, TransportError> {
                Ok(0)
            }
            fn join(&mut self, _: &[u8], _: &[u8], _: u64) -> TimedJoin {
                TimedJoin {
                    elapsed_us: 0,
                    result: Ok(()),
                }
            }
            fn leave(&mut self) -> Result<(), TransportError> {
                Ok(())
            }
            fn quiesce(&mut self) -> Result<(), TransportError> {
                Ok(())
            }
            fn recover_stack(&mut self) -> Result<(), TransportError> {
                Ok(())
            }
        }
        let error = MountedWifi::mount(ArduinoWifiS3::new(Wrong(Fake::default())))
            .err()
            .unwrap();
        assert_eq!(error.error(), StackError::InvalidIdentity);
    }
}
