//! Bounded bridge for ArduinoBLE peripheral stacks.
//!
//! Nobro owns lifecycle validation, caller-owned event storage, stable limits,
//! and deadline accounting. ArduinoBLE owns its process-wide HCI/GATT objects,
//! heap, callbacks, and board-specific controller transport.
#![no_std]

use nobro_wireless::{
    BleEvent, BleStack, LinkDescriptor, LinkState, Protocol, StackError, StackFamily,
    StackIdentity, StackState, WirelessBackend,
};

pub const BACKEND_ID: &str = "arduino-ble";
pub const GATT_VALUE_BYTES: u16 = 20;
pub const QUEUE_SLOTS: u16 = 1;
pub const SERVICE_SLOTS: u16 = 1;
pub const CHARACTERISTIC_SLOTS: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    InvalidConfig,
    Busy,
    Fault,
}

impl TransportError {
    const fn stack_error(self) -> StackError {
        match self {
            Self::InvalidConfig => StackError::InvalidConfig,
            Self::Busy => StackError::Busy,
            Self::Fault => StackError::BackendFault,
        }
    }
}

/// Result of one synchronous ArduinoBLE operation.
///
/// The elapsed time is inspected after the vendor call. It can expose a missed
/// deadline, but it cannot make a blocking vendor call preemptible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedOperation {
    pub elapsed_us: u64,
    pub result: Result<(), TransportError>,
}

pub trait ArduinoBleTransport: WirelessBackend {
    fn mount(&mut self) -> Result<(), TransportError>;
    fn advertise(&mut self, payload: &[u8]) -> TimedOperation;
    fn stop_advertising(&mut self) -> Result<(), TransportError>;
    fn poll_event<const N: usize>(
        &mut self,
        event: &mut BleEvent<N>,
    ) -> Result<bool, TransportError>;
    fn respond_gatt(
        &mut self,
        connection_id: u16,
        attribute_handle: u16,
        value: &[u8],
    ) -> Result<(), TransportError>;
    fn quiesce(&mut self) -> Result<(), TransportError>;
    fn recover_stack(&mut self) -> Result<(), TransportError>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArduinoBleDiagnostics {
    pub advertisements: u32,
    pub advertisement_stops: u32,
    pub events: u32,
    pub gatt_responses: u32,
    pub deadline_misses: u32,
    pub recoveries: u32,
    pub transport_faults: u32,
}

pub struct ArduinoBle<T> {
    transport: T,
    state: StackState,
    diagnostics: ArduinoBleDiagnostics,
}

impl<T> ArduinoBle<T> {
    pub const fn new(transport: T) -> Self {
        Self {
            transport,
            state: StackState::Down,
            diagnostics: ArduinoBleDiagnostics {
                advertisements: 0,
                advertisement_stops: 0,
                events: 0,
                gatt_responses: 0,
                deadline_misses: 0,
                recoveries: 0,
                transport_faults: 0,
            },
        }
    }

    pub const fn diagnostics(&self) -> ArduinoBleDiagnostics {
        self.diagnostics
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    const fn identity() -> StackIdentity {
        StackIdentity {
            backend_id: BACKEND_ID,
            family: StackFamily::Ble,
            mtu: GATT_VALUE_BYTES,
            rx_queue_slots: QUEUE_SLOTS,
            tx_queue_slots: QUEUE_SLOTS,
            service_slots: SERVICE_SLOTS,
            characteristic_slots: CHARACTERISTIC_SLOTS,
        }
    }

    fn fault(&mut self, error: TransportError) -> StackError {
        self.diagnostics.transport_faults = self.diagnostics.transport_faults.saturating_add(1);
        self.state = StackState::Faulted;
        error.stack_error()
    }

    fn operation_error(&mut self, error: TransportError) -> StackError {
        if error == TransportError::Fault {
            self.fault(error)
        } else {
            error.stack_error()
        }
    }
}

impl<T: ArduinoBleTransport> WirelessBackend for ArduinoBle<T> {
    fn descriptor(&self) -> LinkDescriptor {
        self.transport.descriptor()
    }

    fn link_state(&mut self) -> LinkState {
        self.transport.link_state()
    }

    fn send(&mut self, payload: &[u8]) -> bool {
        self.transport.send(payload)
    }

    fn recv(&mut self, destination: &mut [u8]) -> usize {
        self.transport.recv(destination)
    }

    fn recover(&mut self) -> bool {
        self.recover_stack().is_ok()
    }
}

impl<T: ArduinoBleTransport> BleStack for ArduinoBle<T> {
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
        if descriptor.protocol != Protocol::Ble
            || descriptor.mtu != GATT_VALUE_BYTES
            || descriptor.requires_join
            || descriptor.broadcast_only
        {
            return Err(StackError::InvalidIdentity);
        }
        self.state = StackState::Starting;
        self.transport.mount().map_err(|error| self.fault(error))?;
        self.state = StackState::Ready;
        Ok(())
    }

    fn advertise(
        &mut self,
        payload: &[u8],
        now_us: u64,
        deadline_us: u64,
    ) -> Result<(), StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        if payload.len() > usize::from(GATT_VALUE_BYTES) {
            return Err(StackError::InvalidConfig);
        }
        let budget = deadline_us
            .checked_sub(now_us)
            .filter(|remaining| *remaining != 0)
            .ok_or(StackError::DeadlineElapsed)?;
        let operation = self.transport.advertise(payload);
        if operation.elapsed_us > budget {
            let _ = self.transport.stop_advertising();
            self.diagnostics.deadline_misses = self.diagnostics.deadline_misses.saturating_add(1);
            return Err(StackError::DeadlineElapsed);
        }
        operation
            .result
            .map_err(|error| self.operation_error(error))?;
        self.diagnostics.advertisements = self.diagnostics.advertisements.saturating_add(1);
        Ok(())
    }

    fn stop_advertising(&mut self) -> Result<(), StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        self.transport
            .stop_advertising()
            .map_err(|error| self.operation_error(error))?;
        self.diagnostics.advertisement_stops =
            self.diagnostics.advertisement_stops.saturating_add(1);
        Ok(())
    }

    fn poll_event<const N: usize>(&mut self, event: &mut BleEvent<N>) -> Result<bool, StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        let available = self
            .transport
            .poll_event(event)
            .map_err(|error| self.operation_error(error))?;
        if available {
            if event.payload().len() > usize::from(GATT_VALUE_BYTES) {
                return Err(self.fault(TransportError::Fault));
            }
            self.diagnostics.events = self.diagnostics.events.saturating_add(1);
        }
        Ok(available)
    }

    fn respond_gatt(
        &mut self,
        connection_id: u16,
        attribute_handle: u16,
        value: &[u8],
    ) -> Result<(), StackError> {
        if self.state != StackState::Ready {
            return Err(StackError::NotReady);
        }
        if connection_id == 0
            || attribute_handle == 0
            || value.len() > usize::from(GATT_VALUE_BYTES)
        {
            return Err(StackError::InvalidConfig);
        }
        self.transport
            .respond_gatt(connection_id, attribute_handle, value)
            .map_err(|error| self.operation_error(error))?;
        self.diagnostics.gatt_responses = self.diagnostics.gatt_responses.saturating_add(1);
        Ok(())
    }

    fn quiesce_stack(&mut self) -> Result<(), StackError> {
        if self.state == StackState::Down || self.state == StackState::Quiesced {
            self.state = StackState::Quiesced;
            return Ok(());
        }
        self.transport
            .quiesce()
            .map_err(|error| self.fault(error))?;
        self.state = StackState::Quiesced;
        Ok(())
    }

    fn recover_stack(&mut self) -> Result<(), StackError> {
        self.state = StackState::Starting;
        self.transport
            .recover_stack()
            .map_err(|error| self.fault(error))?;
        self.state = StackState::Ready;
        self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_wireless::{BleEventKind, MountedBle};

    struct Fake {
        link: LinkState,
        fail: bool,
        elapsed_us: u64,
        event: Option<BleEvent<20>>,
        stopped: bool,
        last_response: [u8; 20],
        last_response_len: usize,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                link: LinkState::Down,
                fail: false,
                elapsed_us: 10,
                event: None,
                stopped: false,
                last_response: [0; 20],
                last_response_len: 0,
            }
        }
    }

    impl WirelessBackend for Fake {
        fn descriptor(&self) -> LinkDescriptor {
            LinkDescriptor {
                name: "test ArduinoBLE GATT",
                protocol: Protocol::Ble,
                mtu: GATT_VALUE_BYTES,
                requires_join: false,
                broadcast_only: false,
            }
        }

        fn link_state(&mut self) -> LinkState {
            self.link
        }

        fn send(&mut self, payload: &[u8]) -> bool {
            self.link == LinkState::Up && payload.len() <= usize::from(GATT_VALUE_BYTES)
        }

        fn recv(&mut self, _destination: &mut [u8]) -> usize {
            0
        }
    }

    impl ArduinoBleTransport for Fake {
        fn mount(&mut self) -> Result<(), TransportError> {
            if self.fail {
                Err(TransportError::Fault)
            } else {
                Ok(())
            }
        }

        fn advertise(&mut self, _payload: &[u8]) -> TimedOperation {
            TimedOperation {
                elapsed_us: self.elapsed_us,
                result: if self.fail {
                    Err(TransportError::Fault)
                } else {
                    Ok(())
                },
            }
        }

        fn stop_advertising(&mut self) -> Result<(), TransportError> {
            self.stopped = true;
            Ok(())
        }

        fn poll_event<const N: usize>(
            &mut self,
            event: &mut BleEvent<N>,
        ) -> Result<bool, TransportError> {
            let Some(source) = self.event.take() else {
                return Ok(false);
            };
            event.kind = source.kind;
            event.connection_id = source.connection_id;
            event.attribute_handle = source.attribute_handle;
            event
                .set_payload(source.payload())
                .map_err(|_| TransportError::InvalidConfig)?;
            Ok(true)
        }

        fn respond_gatt(
            &mut self,
            _connection_id: u16,
            _attribute_handle: u16,
            value: &[u8],
        ) -> Result<(), TransportError> {
            self.last_response[..value.len()].copy_from_slice(value);
            self.last_response_len = value.len();
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

    fn mount(fake: Fake) -> MountedBle<ArduinoBle<Fake>> {
        match MountedBle::mount(ArduinoBle::new(fake)) {
            Ok(mounted) => mounted,
            Err(_) => panic!("test ArduinoBLE adapter must mount"),
        }
    }

    #[test]
    fn lifecycle_event_gatt_and_recovery_are_bounded() {
        let mut fake = Fake::default();
        let mut write = BleEvent::<20>::empty();
        write.kind = BleEventKind::GattWrite;
        write.connection_id = 1;
        write.attribute_handle = 1;
        write.set_payload(b"command").unwrap();
        fake.event = Some(write);
        let mut mounted = mount(fake);
        assert_eq!(mounted.advertise(b"status", 100, 200), Ok(()));
        let mut event = BleEvent::<20>::empty();
        assert_eq!(mounted.poll_event(&mut event), Ok(true));
        assert_eq!(event.payload(), b"command");
        assert_eq!(mounted.respond_gatt(1, 1, b"ack"), Ok(()));
        assert_eq!(mounted.stop_advertising(), Ok(()));
        assert_eq!(mounted.quiesce(), Ok(()));
        assert_eq!(mounted.recover(), Ok(()));
        let adapter = mounted.into_backend();
        assert_eq!(adapter.transport.last_response_len, 3);
        assert_eq!(
            adapter.diagnostics(),
            ArduinoBleDiagnostics {
                advertisements: 1,
                advertisement_stops: 1,
                events: 1,
                gatt_responses: 1,
                deadline_misses: 0,
                recoveries: 1,
                transport_faults: 0,
            }
        );
    }

    #[test]
    fn synchronous_overrun_stops_advertising() {
        let fake = Fake {
            elapsed_us: 101,
            ..Fake::default()
        };
        let mut mounted = mount(fake);
        assert_eq!(
            mounted.advertise(b"late", 100, 200),
            Err(StackError::DeadlineElapsed)
        );
        let adapter = mounted.into_backend();
        assert!(adapter.transport.stopped);
        assert_eq!(adapter.diagnostics.deadline_misses, 1);
    }

    #[test]
    fn invalid_payloads_and_logical_handles_fail_closed() {
        let mut mounted = mount(Fake::default());
        assert_eq!(
            mounted.advertise(&[0; 21], 0, 100),
            Err(StackError::InvalidConfig)
        );
        assert_eq!(
            mounted.respond_gatt(0, 1, b"x"),
            Err(StackError::InvalidConfig)
        );
        assert_eq!(
            mounted.respond_gatt(1, 0, b"x"),
            Err(StackError::InvalidConfig)
        );
        assert_eq!(
            mounted.respond_gatt(1, 1, &[0; 21]),
            Err(StackError::InvalidConfig)
        );
    }

    #[test]
    fn wrong_transport_identity_is_rejected_before_mount() {
        let fake = Fake {
            link: LinkState::Up,
            ..Fake::default()
        };
        struct Wrong(Fake);
        impl WirelessBackend for Wrong {
            fn descriptor(&self) -> LinkDescriptor {
                LinkDescriptor {
                    protocol: Protocol::WifiTcp,
                    ..self.0.descriptor()
                }
            }
            fn link_state(&mut self) -> LinkState {
                self.0.link_state()
            }
            fn send(&mut self, payload: &[u8]) -> bool {
                self.0.send(payload)
            }
            fn recv(&mut self, destination: &mut [u8]) -> usize {
                self.0.recv(destination)
            }
        }
        impl ArduinoBleTransport for Wrong {
            fn mount(&mut self) -> Result<(), TransportError> {
                self.0.mount()
            }
            fn advertise(&mut self, payload: &[u8]) -> TimedOperation {
                self.0.advertise(payload)
            }
            fn stop_advertising(&mut self) -> Result<(), TransportError> {
                self.0.stop_advertising()
            }
            fn poll_event<const N: usize>(
                &mut self,
                event: &mut BleEvent<N>,
            ) -> Result<bool, TransportError> {
                self.0.poll_event(event)
            }
            fn respond_gatt(
                &mut self,
                connection_id: u16,
                attribute_handle: u16,
                value: &[u8],
            ) -> Result<(), TransportError> {
                self.0.respond_gatt(connection_id, attribute_handle, value)
            }
            fn quiesce(&mut self) -> Result<(), TransportError> {
                self.0.quiesce()
            }
            fn recover_stack(&mut self) -> Result<(), TransportError> {
                self.0.recover_stack()
            }
        }
        let error = match MountedBle::mount(ArduinoBle::new(Wrong(fake))) {
            Ok(_) => panic!("wrong protocol must fail"),
            Err(error) => error,
        };
        assert_eq!(error.error(), StackError::InvalidIdentity);
    }
}
