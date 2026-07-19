//! Allocation-free wireless domain: bounded link contracts, admission, and helpers.
//!
//! Concrete transports can implement [`WirelessBackend`], apps talk bytes + link state,
//! and protocol identity/limits are **data** ([`LinkDescriptor`]) so schedulers can
//! reason about different links uniformly. Implementations are constructed explicitly;
//! protocol-specific WiFi/BLE lifecycle stacks compose beneath [`ManagedLink`] instead
//! of creating a second link crate. The traits do not select or claim a board backend.
//! Pure-logic helpers live here too: [`BleAdvBuilder`] constructs advertising PDUs, and
//! the RFID code carries ISO 14443A anticollision arithmetic.
#![cfg_attr(not(test), no_std)]

/// Protocol identities a link descriptor can carry.
///
/// A variant describes scheduling and diagnostic data; it does not claim that a board
/// or a complete protocol stack is implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// BLE (advertising or connection-based).
    Ble,
    /// WiFi carrying TCP/UDP (e.g. the telemetry JSONL link).
    WifiTcp,
    /// Zigbee network/APS payload (a complete Zigbee stack is required).
    Zigbee,
    /// Raw IEEE 802.15.4 MAC PSDU, without a Zigbee/Thread network-stack claim.
    Ieee802154Raw,
    /// Thread (802.15.4 + 6LoWPAN mesh).
    Thread,
    /// Proximity RFID/NFC (ISO 14443 family).
    Rfid,
    /// Raw proprietary 2.4 GHz (nRF RADIO link mode).
    Proprietary,
}

/// IEEE 802.15.4 maximum PHY service data unit carried as one raw PSDU.
pub const IEEE802154_MAX_PSDU_BYTES: usize = 127;

/// A wireless link as data: what it is and what it can carry.
#[derive(Clone, Copy, Debug)]
pub struct LinkDescriptor {
    pub name: &'static str,
    pub protocol: Protocol,
    /// Largest payload or raw protocol data unit one frame carries.
    pub mtu: u16,
    /// True when the link must join/associate before payload flows.
    pub requires_join: bool,
    /// True when the link is broadcast-only (no per-frame acknowledgement).
    pub broadcast_only: bool,
}

/// Built-in link descriptors (extend with a `pub const`, like every other catalog).
///
/// Catalog membership is metadata, not a backend- or board-support claim.
pub mod link_catalog {
    use super::*;

    pub const BLE_ADV: LinkDescriptor = LinkDescriptor {
        name: "BLE legacy advertising",
        protocol: Protocol::Ble,
        mtu: 24, // ADV_NONCONN_IND payload after AdvA + AD overhead
        requires_join: false,
        broadcast_only: true,
    };
    pub const WIFI_TCP: LinkDescriptor = LinkDescriptor {
        name: "WiFi TCP stream",
        protocol: Protocol::WifiTcp,
        mtu: 1460,
        requires_join: true,
        broadcast_only: false,
    };
    pub const ZIGBEE_APS: LinkDescriptor = LinkDescriptor {
        name: "Zigbee APS payload",
        protocol: Protocol::Zigbee,
        mtu: 82,
        requires_join: true,
        broadcast_only: false,
    };
    pub const IEEE802154_RAW: LinkDescriptor = LinkDescriptor {
        name: "IEEE 802.15.4 raw PSDU",
        protocol: Protocol::Ieee802154Raw,
        mtu: IEEE802154_MAX_PSDU_BYTES as u16,
        requires_join: false,
        broadcast_only: false,
    };
    pub const THREAD_UDP: LinkDescriptor = LinkDescriptor {
        name: "Thread UDP (6LoWPAN)",
        protocol: Protocol::Thread,
        mtu: 1232,
        requires_join: true,
        broadcast_only: false,
    };
    pub const RFID_14443A: LinkDescriptor = LinkDescriptor {
        name: "ISO 14443A proximity",
        protocol: Protocol::Rfid,
        mtu: 18, // MFRC522 FIFO response window used by the no-heap backend
        requires_join: false,
        broadcast_only: false,
    };
    pub const NRF_PROPRIETARY: LinkDescriptor = LinkDescriptor {
        name: "nRF proprietary 2.4 GHz",
        protocol: Protocol::Proprietary,
        mtu: 60,
        requires_join: false,
        broadcast_only: true,
    };
}

/// Link liveness a transport reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Joining,
    Up,
}

/// Bounded data-plane surface implemented by an explicitly constructed transport.
///
/// This trait does not select a board or vendor stack. Protocol lifecycle contracts
/// compose beneath it and [`ManagedLink`]; concrete backend selection remains separate.
pub trait WirelessBackend {
    fn descriptor(&self) -> LinkDescriptor;
    fn link_state(&mut self) -> LinkState;
    /// Send one payload (<= mtu); returns true when the radio accepted it.
    fn send(&mut self, payload: &[u8]) -> bool;
    /// Receive into `buf`; returns bytes delivered (0 = nothing pending).
    fn recv(&mut self, buf: &mut [u8]) -> usize;

    /// Reinitialize the same physical transport. Backends that cannot recover
    /// without board-specific intervention return false explicitly.
    fn recover(&mut self) -> bool {
        false
    }
}

/// Bounded packet owned by the caller; no allocator or hidden global queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet<const N: usize> {
    bytes: [u8; N],
    len: u16,
}

impl<const N: usize> Default for Packet<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Packet<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn copy_from(payload: &[u8]) -> Option<Self> {
        if payload.len() > N || payload.len() > usize::from(u16::MAX) {
            return None;
        }
        let mut packet = Self::new();
        packet.bytes[..payload.len()].copy_from_slice(payload);
        packet.len = payload.len() as u16;
        Some(packet)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub const fn len(&self) -> u16 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Admission contract for one immediate send attempt.
///
/// Priority belongs to the scheduler and retries belong to explicit caller-owned
/// retry state; this synchronous wrapper does not pretend to enforce either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxContract {
    /// Latest time at which an immediate, single-attempt send may be submitted.
    pub deadline_us: u64,
}

impl TxContract {
    pub const fn by(deadline_us: u64) -> Self {
        Self { deadline_us }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkBudget {
    pub max_payload_bytes: u16,
    pub max_tx_per_window: u16,
    pub max_bytes_per_window: u32,
}

impl LinkBudget {
    pub const fn new(
        max_payload_bytes: u16,
        max_tx_per_window: u16,
        max_bytes_per_window: u32,
    ) -> Self {
        Self {
            max_payload_bytes,
            max_tx_per_window,
            max_bytes_per_window,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkDiagnostics {
    pub tx_accepted: u32,
    pub tx_rejected: u32,
    pub rx_packets: u32,
    /// Backend receive counts rejected because they exceeded caller capacity.
    pub rx_rejected: u32,
    pub deadline_rejections: u32,
    pub budget_rejections: u32,
    pub recoveries: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    LinkDown,
    PayloadTooLarge,
    DeadlineElapsed,
    WindowExhausted,
    BackendRejected,
}

/// Deadline and resource-accounting wrapper shared by every backend. Resetting a
/// window is an explicit scheduler action, so no backend owns a private clock.
pub struct ManagedLink<B> {
    backend: B,
    budget: LinkBudget,
    window_tx: u16,
    window_bytes: u32,
    diagnostics: LinkDiagnostics,
}

impl<B: WirelessBackend> ManagedLink<B> {
    pub const fn new(backend: B, budget: LinkBudget) -> Self {
        Self {
            backend,
            budget,
            window_tx: 0,
            window_bytes: 0,
            diagnostics: LinkDiagnostics {
                tx_accepted: 0,
                tx_rejected: 0,
                rx_packets: 0,
                rx_rejected: 0,
                deadline_rejections: 0,
                budget_rejections: 0,
                recoveries: 0,
            },
        }
    }

    pub fn send_at(
        &mut self,
        now_us: u64,
        contract: TxContract,
        payload: &[u8],
    ) -> Result<(), LinkError> {
        if self.backend.link_state() != LinkState::Up {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            return Err(LinkError::LinkDown);
        }
        let limit = self
            .budget
            .max_payload_bytes
            .min(self.backend.descriptor().mtu);
        if payload.len() > usize::from(limit) {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            return Err(LinkError::PayloadTooLarge);
        }
        if now_us > contract.deadline_us {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            self.diagnostics.deadline_rejections =
                self.diagnostics.deadline_rejections.saturating_add(1);
            return Err(LinkError::DeadlineElapsed);
        }
        let bytes = payload.len() as u32;
        if self.window_tx >= self.budget.max_tx_per_window
            || self.window_bytes.saturating_add(bytes) > self.budget.max_bytes_per_window
        {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            self.diagnostics.budget_rejections =
                self.diagnostics.budget_rejections.saturating_add(1);
            return Err(LinkError::WindowExhausted);
        }
        if !self.backend.send(payload) {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            return Err(LinkError::BackendRejected);
        }
        self.window_tx = self.window_tx.saturating_add(1);
        self.window_bytes = self.window_bytes.saturating_add(bytes);
        self.diagnostics.tx_accepted = self.diagnostics.tx_accepted.saturating_add(1);
        Ok(())
    }

    pub fn recv(&mut self, destination: &mut [u8]) -> usize {
        let received = self.backend.recv(destination);
        if received > destination.len() {
            self.diagnostics.rx_rejected = self.diagnostics.rx_rejected.saturating_add(1);
            return 0;
        }
        if received != 0 {
            self.diagnostics.rx_packets = self.diagnostics.rx_packets.saturating_add(1);
        }
        received
    }

    pub fn reset_window(&mut self) {
        self.window_tx = 0;
        self.window_bytes = 0;
    }

    pub fn recover(&mut self) -> bool {
        let recovered = self.backend.recover();
        if recovered {
            self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        }
        recovered
    }

    pub const fn diagnostics(&self) -> LinkDiagnostics {
        self.diagnostics
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn into_backend(self) -> B {
        self.backend
    }
}

// -------------------------------------------------------- WiFi / BLE stack contracts

/// Protocol stack family, kept separate from a board or vendor backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackFamily {
    Wifi,
    Ble,
    Thread,
}

/// Observable lifecycle shared by mountable connectivity stacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackState {
    Down,
    Starting,
    Ready,
    Quiesced,
    Faulted,
}

/// Stable, admission-relevant bounds for one concrete stack instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackIdentity {
    /// Stable public backend id; never a local device or port name.
    pub backend_id: &'static str,
    pub family: StackFamily,
    pub mtu: u16,
    pub rx_queue_slots: u16,
    pub tx_queue_slots: u16,
    /// WiFi reports zero; BLE reports the admitted GATT service capacity.
    pub service_slots: u16,
    /// WiFi reports zero; BLE reports the admitted GATT characteristic capacity.
    pub characteristic_slots: u16,
}

impl StackIdentity {
    pub fn valid_for(self, family: StackFamily) -> bool {
        !self.backend_id.is_empty()
            && self.family == family
            && self.mtu != 0
            && self.rx_queue_slots != 0
            && self.tx_queue_slots != 0
    }
}

/// Protocol-control failure. Vendor-specific detail remains in provider diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackError {
    InvalidConfig,
    InvalidIdentity,
    NotReady,
    Busy,
    DeadlineElapsed,
    QueueFull,
    BackendFault,
    AssociationRejected,
}

/// A mount failure returns ownership so callers can inspect, replace, or retry a backend.
pub struct StackMountError<B> {
    backend: B,
    error: StackError,
}

impl<B> StackMountError<B> {
    pub const fn error(&self) -> StackError {
        self.error
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

/// Runtime-only WiFi association material.
///
/// This value borrows caller storage and is never part of board metadata or a provider
/// identity. The portable layer validates only representation bounds; authentication
/// policy belongs to the selected stack.
#[derive(Clone, Copy)]
pub struct WifiCredentials<'a> {
    ssid: &'a [u8],
    secret: &'a [u8],
}

impl<'a> WifiCredentials<'a> {
    pub fn new(ssid: &'a [u8], secret: &'a [u8]) -> Result<Self, StackError> {
        if ssid.is_empty() || ssid.len() > 32 || secret.len() > 63 {
            return Err(StackError::InvalidConfig);
        }
        Ok(Self { ssid, secret })
    }

    pub const fn ssid(&self) -> &[u8] {
        self.ssid
    }

    pub const fn secret(&self) -> &[u8] {
        self.secret
    }
}

/// One allocation-free WiFi scan result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    ssid: [u8; 32],
    ssid_len: u8,
    pub channel: u8,
    pub rssi_dbm: i8,
    pub secured: bool,
}

impl WifiNetwork {
    pub const fn empty() -> Self {
        Self {
            ssid: [0; 32],
            ssid_len: 0,
            channel: 0,
            rssi_dbm: 0,
            secured: false,
        }
    }

    pub fn set_ssid(&mut self, ssid: &[u8]) -> Result<(), StackError> {
        if ssid.is_empty() || ssid.len() > self.ssid.len() {
            return Err(StackError::InvalidConfig);
        }
        self.ssid.fill(0);
        self.ssid[..ssid.len()].copy_from_slice(ssid);
        self.ssid_len = ssid.len() as u8;
        Ok(())
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }
}

impl Default for WifiNetwork {
    fn default() -> Self {
        Self::empty()
    }
}

/// WiFi association and lifecycle control.
///
/// IP addressing, TCP, UDP, and sockets deliberately do not appear here. An IP-capable
/// backend composes its data plane with `nobro_net` or a bounded external bridge.
pub trait WifiStack: WirelessBackend {
    fn stack_identity(&self) -> StackIdentity;
    fn stack_state(&mut self) -> StackState;
    fn mount_stack(&mut self) -> Result<(), StackError>;
    fn scan(&mut self, results: &mut [WifiNetwork]) -> Result<usize, StackError>;
    fn join(
        &mut self,
        credentials: WifiCredentials<'_>,
        now_us: u64,
        deadline_us: u64,
    ) -> Result<(), StackError>;
    fn leave(&mut self) -> Result<(), StackError>;
    fn quiesce_stack(&mut self) -> Result<(), StackError>;
    fn recover_stack(&mut self) -> Result<(), StackError>;
}

/// An owned, successfully mounted WiFi stack.
pub struct MountedWifi<B> {
    backend: B,
}

impl<B: WifiStack> MountedWifi<B> {
    pub fn mount(mut backend: B) -> Result<Self, StackMountError<B>> {
        if !backend.stack_identity().valid_for(StackFamily::Wifi) {
            return Err(StackMountError {
                backend,
                error: StackError::InvalidIdentity,
            });
        }
        if let Err(error) = backend.mount_stack() {
            return Err(StackMountError { backend, error });
        }
        if backend.stack_state() != StackState::Ready {
            return Err(StackMountError {
                backend,
                error: StackError::BackendFault,
            });
        }
        Ok(Self { backend })
    }

    pub fn state(&mut self) -> StackState {
        self.backend.stack_state()
    }

    pub fn scan(&mut self, results: &mut [WifiNetwork]) -> Result<usize, StackError> {
        let count = self.backend.scan(results)?;
        if count > results.len() {
            return Err(StackError::BackendFault);
        }
        Ok(count)
    }

    pub fn join(
        &mut self,
        credentials: WifiCredentials<'_>,
        now_us: u64,
        deadline_us: u64,
    ) -> Result<(), StackError> {
        self.backend.join(credentials, now_us, deadline_us)?;
        if self.backend.link_state() != LinkState::Up {
            return Err(StackError::BackendFault);
        }
        Ok(())
    }

    pub fn leave(&mut self) -> Result<(), StackError> {
        self.backend.leave()
    }

    pub fn quiesce(&mut self) -> Result<(), StackError> {
        self.backend.quiesce_stack()?;
        if self.backend.stack_state() != StackState::Quiesced {
            return Err(StackError::BackendFault);
        }
        Ok(())
    }

    pub fn recover(&mut self) -> Result<(), StackError> {
        self.backend.recover_stack()?;
        if self.backend.stack_state() != StackState::Ready {
            return Err(StackError::BackendFault);
        }
        Ok(())
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

/// BLE callback/event kind. GATT remains distinct from the WiFi/IP control plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleEventKind {
    Connected,
    Disconnected,
    GattRead,
    GattWrite,
    NotificationComplete,
}

/// One fixed-capacity BLE event copied out of vendor callback context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BleEvent<const N: usize> {
    pub kind: BleEventKind,
    pub connection_id: u16,
    pub attribute_handle: u16,
    bytes: [u8; N],
    len: u16,
}

impl<const N: usize> BleEvent<N> {
    pub const fn empty() -> Self {
        Self {
            kind: BleEventKind::Disconnected,
            connection_id: 0,
            attribute_handle: 0,
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn set_payload(&mut self, payload: &[u8]) -> Result<(), StackError> {
        if payload.len() > N || payload.len() > usize::from(u16::MAX) {
            return Err(StackError::InvalidConfig);
        }
        self.bytes[..payload.len()].copy_from_slice(payload);
        self.len = payload.len() as u16;
        Ok(())
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// Caller-sized queue used to move BLE events out of vendor callback context.
pub struct BleEventQueue<const EVENTS: usize, const BYTES: usize> {
    events: [BleEvent<BYTES>; EVENTS],
    head: usize,
    len: usize,
}

impl<const EVENTS: usize, const BYTES: usize> BleEventQueue<EVENTS, BYTES> {
    pub const fn new() -> Self {
        Self {
            events: [BleEvent::empty(); EVENTS],
            head: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, event: BleEvent<BYTES>) -> Result<(), StackError> {
        if self.len == EVENTS {
            return Err(StackError::QueueFull);
        }
        if EVENTS == 0 {
            return Err(StackError::QueueFull);
        }
        let tail = (self.head + self.len) % EVENTS;
        self.events[tail] = event;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<BleEvent<BYTES>> {
        if self.len == 0 {
            return None;
        }
        let event = self.events[self.head];
        self.head = (self.head + 1) % EVENTS;
        self.len -= 1;
        Some(event)
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const EVENTS: usize, const BYTES: usize> Default for BleEventQueue<EVENTS, BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

/// BLE advertising, GATT, and lifecycle control for one logical stack instance.
pub trait BleStack: WirelessBackend {
    fn stack_identity(&self) -> StackIdentity;
    fn stack_state(&mut self) -> StackState;
    fn mount_stack(&mut self) -> Result<(), StackError>;
    fn advertise(
        &mut self,
        payload: &[u8],
        now_us: u64,
        deadline_us: u64,
    ) -> Result<(), StackError>;
    fn stop_advertising(&mut self) -> Result<(), StackError>;
    fn poll_event<const N: usize>(&mut self, event: &mut BleEvent<N>) -> Result<bool, StackError>;
    fn respond_gatt(
        &mut self,
        connection_id: u16,
        attribute_handle: u16,
        value: &[u8],
    ) -> Result<(), StackError>;
    fn quiesce_stack(&mut self) -> Result<(), StackError>;
    fn recover_stack(&mut self) -> Result<(), StackError>;
}

/// An owned, successfully mounted BLE stack.
pub struct MountedBle<B> {
    backend: B,
}

impl<B: BleStack> MountedBle<B> {
    pub fn mount(mut backend: B) -> Result<Self, StackMountError<B>> {
        if !backend.stack_identity().valid_for(StackFamily::Ble) {
            return Err(StackMountError {
                backend,
                error: StackError::InvalidIdentity,
            });
        }
        if let Err(error) = backend.mount_stack() {
            return Err(StackMountError { backend, error });
        }
        if backend.stack_state() != StackState::Ready {
            return Err(StackMountError {
                backend,
                error: StackError::BackendFault,
            });
        }
        Ok(Self { backend })
    }

    pub fn state(&mut self) -> StackState {
        self.backend.stack_state()
    }

    pub fn advertise(
        &mut self,
        payload: &[u8],
        now_us: u64,
        deadline_us: u64,
    ) -> Result<(), StackError> {
        self.backend.advertise(payload, now_us, deadline_us)
    }

    pub fn stop_advertising(&mut self) -> Result<(), StackError> {
        self.backend.stop_advertising()
    }

    pub fn poll_event<const N: usize>(
        &mut self,
        event: &mut BleEvent<N>,
    ) -> Result<bool, StackError> {
        self.backend.poll_event(event)
    }

    pub fn respond_gatt(
        &mut self,
        connection_id: u16,
        attribute_handle: u16,
        value: &[u8],
    ) -> Result<(), StackError> {
        self.backend
            .respond_gatt(connection_id, attribute_handle, value)
    }

    pub fn quiesce(&mut self) -> Result<(), StackError> {
        self.backend.quiesce_stack()?;
        if self.backend.stack_state() != StackState::Quiesced {
            return Err(StackError::BackendFault);
        }
        Ok(())
    }

    pub fn recover(&mut self) -> Result<(), StackError> {
        self.backend.recover_stack()?;
        if self.backend.stack_state() != StackState::Ready {
            return Err(StackError::BackendFault);
        }
        Ok(())
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

/// Mesh primitives retain their focused implementation crate, while the domain
/// facade gives applications one import path for link and mesh composition.
pub use nobro_net::{PrioQueue, Route, RoutingTable, SeenSet, TimeSync};

// ---------------------------------------------------------------- BLE advertising

/// Builds the legacy ADV_NONCONN_IND PDU format `ble_adv_demo` verified on air:
/// header + AdvA + Complete-Local-Name AD + Manufacturer-Data AD.
pub struct BleAdvBuilder<'a> {
    /// Random-static advertising address, little-endian (MSB must have top bits 11).
    pub adv_addr: &'a [u8; 6],
    /// Advertised name (kept short; it must fit the 31-byte AdvData budget).
    pub name: &'a [u8],
    /// Manufacturer company id (0xFFFF = test/prototyping).
    pub company_id: u16,
}

/// Legacy advertising PDU kind, selecting the on-air header (Bluetooth Core, LE 1M).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvKind {
    /// ADV_IND: connectable + scannable undirected (a central may connect).
    Connectable,
    /// ADV_SCAN_IND: scannable undirected (scan-response allowed, no connect).
    Scannable,
    /// ADV_NONCONN_IND: broadcast only (the beacon shape ble_adv_demo shipped).
    NonConnectable,
}

impl AdvKind {
    /// PDU header byte (PDU type in bits 0-3) with TxAdd = random.
    const fn header(self) -> u8 {
        match self {
            AdvKind::Connectable => 0x40,    // ADV_IND
            AdvKind::Scannable => 0x46,      // ADV_SCAN_IND
            AdvKind::NonConnectable => 0x42, // ADV_NONCONN_IND
        }
    }
}

impl<'a> BleAdvBuilder<'a> {
    /// Assemble a non-connectable beacon PDU.
    pub fn build(&self, payload: &[u8], out: &mut [u8]) -> Option<usize> {
        self.build_as(AdvKind::NonConnectable, payload, out)
    }

    /// Assemble an advertising PDU of the chosen [`AdvKind`] with `payload` as the
    /// manufacturer-data body. Returns the total PDU length, or None if it would exceed
    /// the 31-byte legacy AdvData budget or `out`.
    pub fn build_as(&self, kind: AdvKind, payload: &[u8], out: &mut [u8]) -> Option<usize> {
        let name_ad = 2 + self.name.len(); // len byte counts type + body
        let mfr_ad = 2 + 2 + payload.len();
        let adv_data = name_ad + mfr_ad;
        if adv_data > 31 {
            return None; // legacy advertising AdvData budget
        }
        let total = 2 + 6 + adv_data;
        if out.len() < total {
            return None;
        }
        out[0] = kind.header();
        out[1] = (6 + adv_data) as u8;
        out[2..8].copy_from_slice(self.adv_addr);
        let mut pos = 8;
        out[pos] = (1 + self.name.len()) as u8;
        out[pos + 1] = 0x09; // Complete Local Name
        out[pos + 2..pos + 2 + self.name.len()].copy_from_slice(self.name);
        pos += name_ad;
        out[pos] = (1 + 2 + payload.len()) as u8;
        out[pos + 1] = 0xFF; // Manufacturer Specific Data
        out[pos + 2..pos + 4].copy_from_slice(&self.company_id.to_le_bytes());
        out[pos + 4..pos + 4 + payload.len()].copy_from_slice(payload);
        Some(total)
    }

    /// Assemble a SCAN_RSP PDU (header 0x44) carrying extra AD data a central fetches
    /// after a scan request - lets a scannable advertiser expose more than 31 bytes
    /// total across ADV + SCAN_RSP. `ad` is raw AD structures (each: len, type, body).
    pub fn build_scan_response(&self, ad: &[u8], out: &mut [u8]) -> Option<usize> {
        if ad.len() > 31 {
            return None;
        }
        let total = 2 + 6 + ad.len();
        if out.len() < total {
            return None;
        }
        out[0] = 0x44; // SCAN_RSP, TxAdd = random
        out[1] = (6 + ad.len()) as u8;
        out[2..8].copy_from_slice(self.adv_addr);
        out[8..8 + ad.len()].copy_from_slice(ad);
        Some(total)
    }
}

// ---------------------------------------------------------------- RFID (ISO 14443A)

/// SPI register access a board support package provides for an RFID reader.
///
/// MFRC522 and similar readers use full-duplex SPI transactions for register reads and
/// writes. The trait stays byte-oriented and dependency-free so Arduino, PlatformIO,
/// embedded-hal, or a custom HAL can adapt to it with a tiny shim.
pub trait SpiIo {
    /// Transfer `write` bytes while collecting the same number of bytes in `read`.
    /// Returns false when the bus transaction failed or the device was not selected.
    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> bool;
}

/// Static RFID reader metadata used by board profiles and app generators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RfidReaderDescriptor {
    pub name: &'static str,
    pub protocol: Protocol,
    pub host_bus: &'static str,
    pub max_uid_len: u8,
    pub fifo_len: u8,
}

pub mod rfid_readers {
    use super::*;

    /// Common RC522 / MFRC522 SPI reader module for ISO 14443A tags.
    pub const MFRC522_SPI: RfidReaderDescriptor = RfidReaderDescriptor {
        name: "MFRC522 SPI reader",
        protocol: Protocol::Rfid,
        host_bus: "spi",
        max_uid_len: 10,
        fifo_len: 64,
    };
}

/// Fixed-size ISO 14443A UID returned by a reader poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RfidUid {
    bytes: [u8; 10],
    len: u8,
}

impl RfidUid {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; 10],
            len: 0,
        }
    }

    pub fn from_slice(uid: &[u8]) -> Option<Self> {
        if uid.is_empty() || uid.len() > 10 {
            return None;
        }
        let mut out = Self::empty();
        let mut i = 0;
        while i < uid.len() {
            out.bytes[i] = uid[i];
            i += 1;
        }
        out.len = uid.len() as u8;
        Some(out)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Errors reported by the MFRC522 backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RfidError {
    Bus,
    Timeout,
    Collision,
    Protocol,
    BufferTooSmall,
}

/// No-heap MFRC522 / RC522 reader backend for ISO 14443A polling.
pub struct Mfrc522<S: SpiIo> {
    spi: S,
    initialized: bool,
    rx_cache: [u8; 18],
    rx_len: usize,
}

impl<S: SpiIo> Mfrc522<S> {
    pub const fn new(spi: S) -> Self {
        Self {
            spi,
            initialized: false,
            rx_cache: [0; 18],
            rx_len: 0,
        }
    }

    pub fn into_inner(self) -> S {
        self.spi
    }

    pub fn reader_descriptor(&self) -> RfidReaderDescriptor {
        rfid_readers::MFRC522_SPI
    }

    /// Reset and configure the reader for ISO 14443A polling.
    pub fn init(&mut self) -> Result<(), RfidError> {
        // Re-initialization is fail-closed. A failed register sequence must not
        // preserve prior liveness or expose a response from the old session.
        self.initialized = false;
        self.rx_cache = [0; 18];
        self.rx_len = 0;
        self.write_reg(reg::COMMAND, cmd::SOFT_RESET)?;
        self.write_reg(reg::MODE, 0x3D)?;
        self.write_reg(reg::TIMER_MODE, 0x8D)?;
        self.write_reg(reg::TIMER_PRESCALER, 0x3E)?;
        self.write_reg(reg::TIMER_RELOAD_H, 0x00)?;
        self.write_reg(reg::TIMER_RELOAD_L, 0x1E)?;
        self.write_reg(reg::TX_ASK, 0x40)?;
        self.write_reg(reg::TX_MODE, 0x00)?;
        self.write_reg(reg::RX_MODE, 0x00)?;
        self.set_bits(reg::TX_CONTROL, 0x03)?;
        self.initialized = true;
        Ok(())
    }

    /// Send REQA and return the ATQA bytes when a tag answers.
    pub fn request_a(&mut self, out: &mut [u8]) -> Result<usize, RfidError> {
        self.write_reg(reg::BIT_FRAMING, 0x07)?;
        self.transceive(&[0x26], 7, out)
    }

    /// Run cascade-level-1 anticollision and return a 4-byte UID when present.
    pub fn anticollision_level1(&mut self) -> Result<RfidUid, RfidError> {
        let mut frame = [0u8; 5];
        let n = self.transceive(&[0x93, 0x20], 0, &mut frame)?;
        if n != 5 || !rfid::validate_anticollision(&frame) || rfid::has_next_cascade(&frame) {
            return Err(RfidError::Protocol);
        }
        RfidUid::from_slice(&frame[..4]).ok_or(RfidError::Protocol)
    }

    /// Poll for one ISO 14443A card UID. This is non-allocating and bounded by `poll_budget`.
    pub fn poll_uid(&mut self, poll_budget: u16) -> Result<RfidUid, RfidError> {
        let mut atqa = [0u8; 2];
        let mut tries = 0;
        while tries < poll_budget {
            match self.request_a(&mut atqa) {
                Ok(2) => return self.anticollision_level1(),
                Ok(_) => return Err(RfidError::Protocol),
                Err(RfidError::Timeout) => tries += 1,
                Err(e) => return Err(e),
            }
        }
        Err(RfidError::Timeout)
    }

    pub fn transceive(
        &mut self,
        tx: &[u8],
        valid_bits: u8,
        rx: &mut [u8],
    ) -> Result<usize, RfidError> {
        if tx.len() > usize::from(rfid_readers::MFRC522_SPI.fifo_len) {
            return Err(RfidError::BufferTooSmall);
        }
        self.write_reg(reg::COMMAND, cmd::IDLE)?;
        self.write_reg(reg::COMM_IRQ, 0x7F)?;
        self.write_reg(reg::FIFO_LEVEL, 0x80)?;
        for &b in tx {
            self.write_reg(reg::FIFO_DATA, b)?;
        }
        self.write_reg(reg::BIT_FRAMING, valid_bits & 0x07)?;
        self.write_reg(reg::COMMAND, cmd::TRANSCEIVE)?;
        self.set_bits(reg::BIT_FRAMING, 0x80)?;

        let mut polls = 0;
        while polls < 200 {
            let irq = self.read_reg(reg::COMM_IRQ)?;
            if irq & 0x30 != 0 {
                break;
            }
            if irq & 0x01 != 0 {
                self.clear_bits(reg::BIT_FRAMING, 0x80)?;
                return Err(RfidError::Timeout);
            }
            polls += 1;
        }
        if polls == 200 {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::Timeout);
        }

        let err = self.read_reg(reg::ERROR)?;
        if err & 0x08 != 0 {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::Collision);
        }
        if err & 0x13 != 0 {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::Protocol);
        }
        let fifo = usize::from(self.read_reg(reg::FIFO_LEVEL)?);
        if fifo > rx.len() {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::BufferTooSmall);
        }
        let mut i = 0;
        while i < fifo {
            rx[i] = self.read_reg(reg::FIFO_DATA)?;
            i += 1;
        }
        self.clear_bits(reg::BIT_FRAMING, 0x80)?;
        Ok(fifo)
    }

    fn read_reg(&mut self, reg: u8) -> Result<u8, RfidError> {
        let write = [((reg << 1) & 0x7E) | 0x80, 0x00];
        let mut read = [0u8; 2];
        if self.spi.transfer(&write, &mut read) {
            Ok(read[1])
        } else {
            Err(RfidError::Bus)
        }
    }

    fn write_reg(&mut self, reg: u8, value: u8) -> Result<(), RfidError> {
        let write = [(reg << 1) & 0x7E, value];
        let mut read = [0u8; 2];
        if self.spi.transfer(&write, &mut read) {
            Ok(())
        } else {
            Err(RfidError::Bus)
        }
    }

    fn set_bits(&mut self, reg: u8, mask: u8) -> Result<(), RfidError> {
        let v = self.read_reg(reg)?;
        self.write_reg(reg, v | mask)
    }

    fn clear_bits(&mut self, reg: u8, mask: u8) -> Result<(), RfidError> {
        let v = self.read_reg(reg)?;
        self.write_reg(reg, v & !mask)
    }
}

impl<S: SpiIo> WirelessBackend for Mfrc522<S> {
    fn descriptor(&self) -> LinkDescriptor {
        link_catalog::RFID_14443A
    }

    fn link_state(&mut self) -> LinkState {
        if self.initialized {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    fn send(&mut self, payload: &[u8]) -> bool {
        if !self.initialized || payload.len() > usize::from(self.descriptor().mtu) {
            return false;
        }
        let mut tmp = [0u8; 18];
        match self.transceive(payload, 0, &mut tmp) {
            Ok(n) => {
                self.rx_cache[..n].copy_from_slice(&tmp[..n]);
                self.rx_len = n;
                true
            }
            Err(_) => false,
        }
    }

    fn recv(&mut self, buf: &mut [u8]) -> usize {
        if !self.initialized {
            return 0;
        }
        if self.rx_len == 0 {
            if let Ok(uid) = self.poll_uid(1) {
                let n = uid.len().min(self.rx_cache.len());
                self.rx_cache[..n].copy_from_slice(uid.as_slice());
                self.rx_len = n;
            }
        }
        // Preserve packet atomicity: a caller can retry with a sufficiently sized
        // destination instead of silently losing a cached suffix.
        if self.rx_len > buf.len() {
            return 0;
        }
        let n = self.rx_len;
        buf[..n].copy_from_slice(&self.rx_cache[..n]);
        self.rx_len = 0;
        n
    }
}

mod reg {
    pub const COMMAND: u8 = 0x01;
    pub const COMM_IRQ: u8 = 0x04;
    pub const ERROR: u8 = 0x06;
    pub const FIFO_DATA: u8 = 0x09;
    pub const FIFO_LEVEL: u8 = 0x0A;
    pub const BIT_FRAMING: u8 = 0x0D;
    pub const MODE: u8 = 0x11;
    pub const TX_MODE: u8 = 0x12;
    pub const RX_MODE: u8 = 0x13;
    pub const TX_CONTROL: u8 = 0x14;
    pub const TX_ASK: u8 = 0x15;
    pub const TIMER_MODE: u8 = 0x2A;
    pub const TIMER_PRESCALER: u8 = 0x2B;
    pub const TIMER_RELOAD_H: u8 = 0x2C;
    pub const TIMER_RELOAD_L: u8 = 0x2D;
}

mod cmd {
    pub const IDLE: u8 = 0x00;
    pub const TRANSCEIVE: u8 = 0x0C;
    pub const SOFT_RESET: u8 = 0x0F;
}
pub mod rfid {
    /// BCC of a 4-byte UID cascade level (XOR of the four bytes, ISO 14443-3 A).
    pub fn iso14443a_bcc(uid: &[u8; 4]) -> u8 {
        uid[0] ^ uid[1] ^ uid[2] ^ uid[3]
    }

    /// Validate a 5-byte anticollision frame (uid0..3 + bcc).
    pub fn validate_anticollision(frame: &[u8; 5]) -> bool {
        iso14443a_bcc(&[frame[0], frame[1], frame[2], frame[3]]) == frame[4]
    }

    /// The cascade-tag byte marking a UID that continues at the next level.
    pub const CASCADE_TAG: u8 = 0x88;

    /// True when a cascade level says the UID continues (7/10-byte UIDs).
    pub fn has_next_cascade(frame: &[u8; 5]) -> bool {
        frame[0] == CASCADE_TAG
    }
}

// ---------------------------------------------------------------- CC2530 802.15.4

/// Byte-level UART access a board provides so the hardware-agnostic CC2530 backend can
/// run on any MCU (the nRF app, an ESP32, etc. each supply their own UART).
pub trait ByteIo {
    /// Transmit one byte (blocking until accepted).
    fn write(&mut self, b: u8);
    /// Poll one received byte, or None if none is pending.
    fn read(&mut self) -> Option<u8>;
}

const CC2530_UART_MAX_DATA_BYTES: usize = u8::MAX as usize - 1;

/// 802.15.4 MAC frame types (low 3 bits of the frame-control field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacFrameType {
    Beacon,
    Data,
    Ack,
    MacCommand,
    Reserved,
}

/// Classify a raw PSDU by its frame-control field - the same rule the gateway and the
/// host `nobro_rtos.zigbee` contract use, so on-device and host agree.
pub fn mac_frame_type(psdu: &[u8]) -> Option<MacFrameType> {
    let fcf_lo = *psdu.first()?;
    Some(match fcf_lo & 0x7 {
        0 => MacFrameType::Beacon,
        1 => MacFrameType::Data,
        2 => MacFrameType::Ack,
        3 => MacFrameType::MacCommand,
        _ => MacFrameType::Reserved,
    })
}

/// Modular CC2530 802.15.4 backend: drives the NiusZigbee SDCC firmware protocol
/// (`FE LEN CMD DATA FCS`, LEN counts CMD, FCS = XOR of LEN..DATA) over any [`ByteIo`],
/// and presents the common [`WirelessBackend`] data plane. The implemented firmware
/// surface carries raw IEEE 802.15.4 PSDUs; it is not a Zigbee NWK/APS stack.
pub struct Cc2530<U: ByteIo> {
    io: U,
    initialized: bool,
    dec: Cc2530Decoder,
}

/// Streaming frame decoder for the CC2530 UART protocol (mirrors the C++ host driver).
struct Cc2530Decoder {
    state: u8,
    len: u8,
    idx: u8,
    fcs: u8,
    buf: [u8; 160],
}

impl Cc2530Decoder {
    const fn new() -> Self {
        Cc2530Decoder {
            state: 0,
            len: 0,
            idx: 0,
            fcs: 0,
            buf: [0; 160],
        }
    }
    /// Feed one byte; returns the frame length (in `buf`) when a valid frame completes.
    fn feed(&mut self, b: u8) -> Option<u8> {
        match self.state {
            0 => {
                if b == 0xFE {
                    self.state = 1;
                }
            }
            1 => {
                self.len = b;
                self.idx = 0;
                self.fcs = b;
                self.state = if b == 0 || b as usize > self.buf.len() {
                    0
                } else {
                    2
                };
            }
            2 => {
                self.buf[self.idx as usize] = b;
                self.idx += 1;
                self.fcs ^= b;
                if self.idx >= self.len {
                    self.state = 3;
                }
            }
            _ => {
                self.state = 0;
                if b == self.fcs {
                    return Some(self.len);
                }
            }
        }
        None
    }
}

impl<U: ByteIo> Cc2530<U> {
    pub fn new(io: U) -> Self {
        Cc2530 {
            io,
            initialized: false,
            dec: Cc2530Decoder::new(),
        }
    }

    /// Send a raw command frame (`FE LEN CMD DATA FCS`).
    fn send_cmd(&mut self, cmd: u8, data: &[u8]) -> bool {
        if data.len() > CC2530_UART_MAX_DATA_BYTES {
            return false;
        }
        let len = (data.len() + 1) as u8;
        let mut fcs = len ^ cmd;
        self.io.write(0xFE);
        self.io.write(len);
        self.io.write(cmd);
        for &b in data {
            self.io.write(b);
            fcs ^= b;
        }
        self.io.write(fcs);
        true
    }

    /// Initialize the raw radio: flush its parser, PING, then set channel + RX filtering.
    /// Returns true once a PONG is seen. `poll_budget` bounds the byte-poll wait.
    pub fn initialize(&mut self, channel: u8, poll_budget: u32) -> bool {
        // Re-initialization is fail-closed: neither prior liveness nor a partial
        // host-side frame may survive a new handshake attempt.
        self.initialized = false;
        self.dec = Cc2530Decoder::new();
        for _ in 0..140 {
            self.io.write(0x00); // flush any partial frame in the firmware parser
        }
        // PING
        if !self.send_cmd(0x01, &[]) {
            return false;
        }
        for _ in 0..poll_budget {
            if let Some(b) = self.io.read() {
                if let Some(_len) = self.dec.feed(b) {
                    if self.dec.buf[0] == 0x81 {
                        // PONG
                        // SET_CHANNEL + SET_PROMISC (filter off)
                        if !self.send_cmd(0x02, &[channel]) || !self.send_cmd(0x04, &[0]) {
                            return false;
                        }
                        self.initialized = true;
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Compatibility alias for callers written before the raw-link contract was named
    /// precisely. This initializes the co-processor; it does not join a Zigbee network.
    #[doc(hidden)]
    pub fn join(&mut self, channel: u8, poll_budget: u32) -> bool {
        self.initialize(channel, poll_budget)
    }

    /// Poll for one captured 802.15.4 PSDU into `out`; returns (len, frame_type) once a
    /// full RX_FRAME arrives, or None when no complete frame is pending (non-blocking:
    /// drains available bytes, then yields). Non-RX frames are consumed and skipped.
    pub fn poll_frame(&mut self, out: &mut [u8]) -> Option<(usize, MacFrameType)> {
        while let Some(b) = self.io.read() {
            if let Some(flen) = self.dec.feed(b) {
                let flen = flen as usize;
                if self.dec.buf[0] == 0x84 && flen >= 4 {
                    // buf = [0x84, rssi, lqi, psdu..]
                    let psdu = &self.dec.buf[3..flen];
                    let n = psdu.len().min(out.len());
                    out[..n].copy_from_slice(&psdu[..n]);
                    if let Some(ft) = mac_frame_type(psdu) {
                        return Some((n, ft));
                    }
                }
            }
        }
        None
    }
}

impl<U: ByteIo> WirelessBackend for Cc2530<U> {
    fn descriptor(&self) -> LinkDescriptor {
        link_catalog::IEEE802154_RAW
    }
    fn link_state(&mut self) -> LinkState {
        if self.initialized {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }
    fn send(&mut self, payload: &[u8]) -> bool {
        if payload.len() > usize::from(self.descriptor().mtu)
            || payload.len() > IEEE802154_MAX_PSDU_BYTES
            || payload.len() > CC2530_UART_MAX_DATA_BYTES
            || !self.initialized
        {
            return false;
        }
        self.send_cmd(0x03, payload) // TX raw PSDU
    }
    fn recv(&mut self, buf: &mut [u8]) -> usize {
        self.poll_frame(buf).map(|(n, _)| n).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_distinguishes_link_shapes() {
        let ble_adv = core::hint::black_box(link_catalog::BLE_ADV);
        let wifi_tcp = core::hint::black_box(link_catalog::WIFI_TCP);
        let zigbee_aps = core::hint::black_box(link_catalog::ZIGBEE_APS);
        let ieee802154_raw = core::hint::black_box(link_catalog::IEEE802154_RAW);
        assert!(ble_adv.broadcast_only);
        assert!(!ble_adv.requires_join);
        assert!(wifi_tcp.requires_join);
        assert!(zigbee_aps.mtu < wifi_tcp.mtu);
        assert_eq!(ieee802154_raw.protocol, Protocol::Ieee802154Raw);
        assert_eq!(ieee802154_raw.mtu, 127);
        assert!(!ieee802154_raw.requires_join);
        assert_eq!(link_catalog::RFID_14443A.protocol, Protocol::Rfid);
        assert_eq!(link_catalog::RFID_14443A.mtu, 18);
        assert_eq!(rfid_readers::MFRC522_SPI.host_bus, "spi");
        assert_eq!(rfid_readers::MFRC522_SPI.max_uid_len, 10);
    }

    struct MockWifi {
        state: StackState,
        fail_mount: bool,
        joined: bool,
        scan_count: usize,
    }

    impl MockWifi {
        const fn new(fail_mount: bool) -> Self {
            Self {
                state: StackState::Down,
                fail_mount,
                joined: false,
                scan_count: 1,
            }
        }

        const fn hostile_scan_count() -> Self {
            Self {
                state: StackState::Down,
                fail_mount: false,
                joined: false,
                scan_count: usize::MAX,
            }
        }
    }

    impl WirelessBackend for MockWifi {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::WIFI_TCP
        }

        fn link_state(&mut self) -> LinkState {
            if self.joined {
                LinkState::Up
            } else {
                LinkState::Down
            }
        }

        fn send(&mut self, _payload: &[u8]) -> bool {
            self.joined
        }

        fn recv(&mut self, _buf: &mut [u8]) -> usize {
            0
        }
    }

    impl WifiStack for MockWifi {
        fn stack_identity(&self) -> StackIdentity {
            StackIdentity {
                backend_id: "test-wifi",
                family: StackFamily::Wifi,
                mtu: 1460,
                rx_queue_slots: 2,
                tx_queue_slots: 2,
                service_slots: 0,
                characteristic_slots: 0,
            }
        }

        fn stack_state(&mut self) -> StackState {
            self.state
        }

        fn mount_stack(&mut self) -> Result<(), StackError> {
            if self.fail_mount {
                self.state = StackState::Faulted;
                return Err(StackError::BackendFault);
            }
            self.state = StackState::Ready;
            Ok(())
        }

        fn scan(&mut self, results: &mut [WifiNetwork]) -> Result<usize, StackError> {
            if self.state != StackState::Ready {
                return Err(StackError::NotReady);
            }
            let Some(first) = results.first_mut() else {
                return Ok(0);
            };
            first.set_ssid(b"test-network")?;
            first.channel = 6;
            first.rssi_dbm = -42;
            first.secured = true;
            Ok(self.scan_count)
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
            if now_us > deadline_us {
                return Err(StackError::DeadlineElapsed);
            }
            if credentials.ssid() != b"test-network" {
                return Err(StackError::BackendFault);
            }
            self.joined = true;
            Ok(())
        }

        fn leave(&mut self) -> Result<(), StackError> {
            self.joined = false;
            Ok(())
        }

        fn quiesce_stack(&mut self) -> Result<(), StackError> {
            self.joined = false;
            self.state = StackState::Quiesced;
            Ok(())
        }

        fn recover_stack(&mut self) -> Result<(), StackError> {
            self.joined = false;
            self.state = StackState::Ready;
            Ok(())
        }
    }

    struct MockBle {
        state: StackState,
        advertising: bool,
        event_pending: bool,
    }

    impl MockBle {
        const fn new() -> Self {
            Self {
                state: StackState::Down,
                advertising: false,
                event_pending: true,
            }
        }
    }

    impl WirelessBackend for MockBle {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::BLE_ADV
        }

        fn link_state(&mut self) -> LinkState {
            if self.advertising {
                LinkState::Up
            } else {
                LinkState::Down
            }
        }

        fn send(&mut self, _payload: &[u8]) -> bool {
            self.advertising
        }

        fn recv(&mut self, _buf: &mut [u8]) -> usize {
            0
        }
    }

    impl BleStack for MockBle {
        fn stack_identity(&self) -> StackIdentity {
            StackIdentity {
                backend_id: "test-ble",
                family: StackFamily::Ble,
                mtu: 23,
                rx_queue_slots: 2,
                tx_queue_slots: 2,
                service_slots: 1,
                characteristic_slots: 2,
            }
        }

        fn stack_state(&mut self) -> StackState {
            self.state
        }

        fn mount_stack(&mut self) -> Result<(), StackError> {
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
            if now_us > deadline_us {
                return Err(StackError::DeadlineElapsed);
            }
            if payload.len() > 31 {
                return Err(StackError::InvalidConfig);
            }
            self.advertising = true;
            Ok(())
        }

        fn stop_advertising(&mut self) -> Result<(), StackError> {
            self.advertising = false;
            Ok(())
        }

        fn poll_event<const N: usize>(
            &mut self,
            event: &mut BleEvent<N>,
        ) -> Result<bool, StackError> {
            if !self.event_pending {
                return Ok(false);
            }
            event.kind = BleEventKind::GattWrite;
            event.connection_id = 7;
            event.attribute_handle = 11;
            event.set_payload(b"ok")?;
            self.event_pending = false;
            Ok(true)
        }

        fn respond_gatt(
            &mut self,
            connection_id: u16,
            attribute_handle: u16,
            value: &[u8],
        ) -> Result<(), StackError> {
            if connection_id != 7 || attribute_handle != 11 || value.len() > 23 {
                return Err(StackError::InvalidConfig);
            }
            Ok(())
        }

        fn quiesce_stack(&mut self) -> Result<(), StackError> {
            self.advertising = false;
            self.state = StackState::Quiesced;
            Ok(())
        }

        fn recover_stack(&mut self) -> Result<(), StackError> {
            self.advertising = false;
            self.state = StackState::Ready;
            Ok(())
        }
    }

    #[test]
    fn wifi_mount_is_owned_bounded_and_runtime_configured() {
        let failed = match MountedWifi::mount(MockWifi::new(true)) {
            Ok(_) => panic!("faulting backend mounted"),
            Err(error) => error,
        };
        assert_eq!(failed.error(), StackError::BackendFault);
        assert!(failed.into_backend().fail_mount);

        assert!(matches!(
            WifiCredentials::new(&[], b"secret"),
            Err(StackError::InvalidConfig)
        ));
        let credentials = WifiCredentials::new(b"test-network", b"secret").unwrap();
        let mut mounted = MountedWifi::mount(MockWifi::new(false)).ok().unwrap();
        let mut networks = [WifiNetwork::empty(); 2];
        assert_eq!(mounted.scan(&mut networks).unwrap(), 1);
        assert_eq!(networks[0].ssid(), b"test-network");
        assert_eq!(
            mounted.join(credentials, 101, 100),
            Err(StackError::DeadlineElapsed)
        );
        mounted.join(credentials, 100, 100).unwrap();
        assert_eq!(mounted.backend_mut().link_state(), LinkState::Up);
        mounted.quiesce().unwrap();
        assert_eq!(mounted.state(), StackState::Quiesced);
        mounted.recover().unwrap();
        assert_eq!(mounted.state(), StackState::Ready);

        let mut hostile = MountedWifi::mount(MockWifi::hostile_scan_count())
            .ok()
            .unwrap();
        assert_eq!(hostile.scan(&mut networks), Err(StackError::BackendFault));
    }

    #[test]
    fn ble_mount_gatt_and_callback_queue_are_bounded() {
        let mut mounted = MountedBle::mount(MockBle::new()).ok().unwrap();
        assert_eq!(
            mounted.advertise(b"payload", 11, 10),
            Err(StackError::DeadlineElapsed)
        );
        mounted.advertise(b"payload", 10, 10).unwrap();

        let mut event = BleEvent::<8>::empty();
        assert!(mounted.poll_event(&mut event).unwrap());
        assert_eq!(event.payload(), b"ok");
        mounted
            .respond_gatt(event.connection_id, event.attribute_handle, b"reply")
            .unwrap();

        let mut queue = BleEventQueue::<1, 8>::new();
        queue.push(event).unwrap();
        assert_eq!(queue.push(event), Err(StackError::QueueFull));
        assert_eq!(queue.pop().unwrap().payload(), b"ok");
        assert!(queue.is_empty());

        let mut zero = BleEventQueue::<0, 8>::new();
        assert_eq!(zero.push(event), Err(StackError::QueueFull));

        mounted.quiesce().unwrap();
        assert_eq!(mounted.state(), StackState::Quiesced);
        mounted.recover().unwrap();
        assert_eq!(mounted.state(), StackState::Ready);
    }

    #[test]
    fn wifi_and_ble_instances_are_additive_not_global_selection() {
        let wifi = MountedWifi::mount(MockWifi::new(false)).ok().unwrap();
        let ble = MountedBle::mount(MockBle::new()).ok().unwrap();
        assert_eq!(wifi.backend().stack_identity().family, StackFamily::Wifi);
        assert_eq!(ble.backend().stack_identity().family, StackFamily::Ble);
    }

    #[test]
    fn adv_builder_reproduces_the_on_air_format() {
        // Same identity ble_adv_demo used on air.
        let addr = [0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"NOBRO",
            company_id: 0xFFFF,
        };
        let mut pdu = [0u8; 39];
        let payload = [1u8, 0, 0, 0, 1]; // beat=1, status=1
        let n = b.build(&payload, &mut pdu).unwrap();
        assert_eq!(n, 24); // 2 hdr + 6 AdvA + 7 name AD + 9 mfr AD
        assert_eq!(pdu[0], 0x42); // ADV_NONCONN_IND + random TxAdd
        assert_eq!(pdu[1], 22); // AdvA + ADs
        assert_eq!(&pdu[2..8], &addr);
        assert_eq!(pdu[8], 6); // name AD length
        assert_eq!(pdu[9], 0x09);
        assert_eq!(&pdu[10..15], b"NOBRO");
        assert_eq!(pdu[15], 8); // mfr AD length: type + company(2) + payload(5)
        assert_eq!(pdu[16], 0xFF);
        assert_eq!(&pdu[17..19], &[0xFF, 0xFF]);
        assert_eq!(&pdu[19..24], &payload);
    }

    #[test]
    fn adv_kind_selects_the_on_air_header() {
        let addr = [0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"NOBRO",
            company_id: 0xFFFF,
        };
        let mut pdu = [0u8; 39];
        let payload = [1u8, 0, 0, 0, 1];
        // non-connectable (default) keeps the verified 0x42 header
        assert_eq!(b.build(&payload, &mut pdu).unwrap(), 24);
        assert_eq!(pdu[0], 0x42);
        // connectable ADV_IND
        b.build_as(AdvKind::Connectable, &payload, &mut pdu)
            .unwrap();
        assert_eq!(pdu[0], 0x40);
        // scannable ADV_SCAN_IND
        b.build_as(AdvKind::Scannable, &payload, &mut pdu).unwrap();
        assert_eq!(pdu[0], 0x46);
        // payload + addressing are identical regardless of kind
        assert_eq!(&pdu[2..8], &addr);
        assert_eq!(&pdu[19..24], &payload);
    }

    #[test]
    fn scan_response_carries_extra_ad() {
        let addr = [1u8, 2, 3, 4, 5, 0xC3];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"N",
            company_id: 0,
        };
        let mut rsp = [0u8; 39];
        // one AD: Tx Power Level (type 0x0A) = -4 dBm
        let ad = [0x02, 0x0A, 0xFC];
        let n = b.build_scan_response(&ad, &mut rsp).unwrap();
        assert_eq!(rsp[0], 0x44); // SCAN_RSP
        assert_eq!(n, 2 + 6 + 3);
        assert_eq!(&rsp[8..11], &ad);
    }

    #[test]
    fn adv_builder_enforces_the_31_byte_budget() {
        let addr = [0u8; 6];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"WAY-TOO-LONG-DEVICE-NAME",
            company_id: 0,
        };
        let mut pdu = [0u8; 39];
        assert!(b.build(&[0u8; 8], &mut pdu).is_none());
    }

    #[test]
    fn mac_frame_type_matches_the_host_contract() {
        assert_eq!(mac_frame_type(&[0x02, 0x00]), Some(MacFrameType::Ack));
        assert_eq!(
            mac_frame_type(&[0x03, 0x08]),
            Some(MacFrameType::MacCommand)
        );
        assert_eq!(mac_frame_type(&[0x61, 0x88]), Some(MacFrameType::Data));
        assert_eq!(mac_frame_type(&[0x00, 0x80]), Some(MacFrameType::Beacon));
        assert_eq!(mac_frame_type(&[]), None);
    }

    // Scripted UART: replays firmware bytes and captures what the backend transmits.
    struct FakeUart {
        rx: HeaplessVec,
        rx_pos: usize,
        tx: HeaplessVec,
    }
    // A tiny fixed-capacity byte vec so the test stays no_std/no-heap like the crate.
    struct HeaplessVec {
        data: [u8; 256],
        len: usize,
    }
    impl Default for HeaplessVec {
        fn default() -> Self {
            HeaplessVec {
                data: [0; 256],
                len: 0,
            }
        }
    }
    impl HeaplessVec {
        fn push(&mut self, b: u8) {
            if self.len < self.data.len() {
                self.data[self.len] = b;
                self.len += 1;
            }
        }
        fn extend(&mut self, bs: &[u8]) {
            for &b in bs {
                self.push(b);
            }
        }
    }
    impl ByteIo for FakeUart {
        fn write(&mut self, b: u8) {
            self.tx.push(b);
        }
        fn read(&mut self) -> Option<u8> {
            if self.rx_pos < self.rx.len {
                let b = self.rx.data[self.rx_pos];
                self.rx_pos += 1;
                Some(b)
            } else {
                None
            }
        }
    }

    fn frame(cmd: u8, data: &[u8]) -> ([u8; 8], usize) {
        // build FE LEN CMD DATA FCS for short frames (test helper)
        let len = (data.len() + 1) as u8;
        let mut fcs = len ^ cmd;
        let mut out = [0u8; 8];
        out[0] = 0xFE;
        out[1] = len;
        out[2] = cmd;
        let mut n = 3;
        for &b in data {
            out[n] = b;
            fcs ^= b;
            n += 1;
        }
        out[n] = fcs;
        n += 1;
        (out, n)
    }

    #[test]
    fn cc2530_backend_initializes_and_captures_a_raw_frame() {
        let mut rx = HeaplessVec::default();
        // firmware replies PONG [ver_hi ver_lo], then an RX_FRAME with a beacon-request PSDU
        let (pong, pn) = frame(0x81, &[0, 8]);
        rx.extend(&pong[..pn]);
        // RX_FRAME 0x84 [rssi lqi psdu(03 08 EA FF FF FF FF 07)]
        let (rxf, rn) = {
            let psdu = [0x03u8, 0x08, 0xEA, 0xFF, 0xFF, 0xFF, 0xFF, 0x07];
            let mut data = [0u8; 10];
            data[0] = 200; // rssi
            data[1] = 255; // lqi
            data[2..10].copy_from_slice(&psdu);
            // reuse a bigger frame builder inline
            let len = (data.len() + 1) as u8;
            let mut fcs = len ^ 0x84;
            let mut out = [0u8; 16];
            out[0] = 0xFE;
            out[1] = len;
            out[2] = 0x84;
            for (i, &b) in data.iter().enumerate() {
                out[3 + i] = b;
                fcs ^= b;
            }
            out[3 + data.len()] = fcs;
            (out, 3 + data.len() + 1)
        };
        rx.extend(&rxf[..rn]);

        let uart = FakeUart {
            rx,
            rx_pos: 0,
            tx: HeaplessVec::default(),
        };
        let mut radio = Cc2530::new(uart);
        assert!(radio.initialize(11, 10_000));
        assert_eq!(radio.link_state(), LinkState::Up);
        assert_eq!(radio.descriptor().protocol, Protocol::Ieee802154Raw);
        assert!(!radio.descriptor().requires_join);

        // Initialization transmits PING, SET_CHANNEL(11), SET_PROMISC(0).
        assert!(radio.io.tx.data[..radio.io.tx.len].contains(&11));

        let mut buf = [0u8; 32];
        let (n, ft) = radio.poll_frame(&mut buf).expect("captured frame");
        assert_eq!(ft, MacFrameType::MacCommand);
        assert_eq!(&buf[..n], &[0x03, 0x08, 0xEA, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]);

        radio.dec.state = 2;
        radio.dec.idx = 3;
        assert!(!radio.initialize(11, 0));
        assert_eq!(radio.link_state(), LinkState::Down);
        assert_eq!(radio.dec.state, 0);
        assert_eq!(radio.dec.idx, 0);
    }

    #[test]
    fn cc2530_rejects_payload_max_plus_one_before_writing_or_casting() {
        let descriptor = core::hint::black_box(link_catalog::IEEE802154_RAW);
        assert!(usize::from(descriptor.mtu) <= IEEE802154_MAX_PSDU_BYTES);
        assert!(usize::from(descriptor.mtu) <= CC2530_UART_MAX_DATA_BYTES);
        let uart = FakeUart {
            rx: HeaplessVec::default(),
            rx_pos: 0,
            tx: HeaplessVec::default(),
        };
        let mut radio = Cc2530::new(uart);
        radio.initialized = true;

        let max_plus_one = [0u8; 128];
        assert_eq!(max_plus_one.len(), usize::from(descriptor.mtu) + 1);
        assert!(!radio.send(&max_plus_one));
        assert_eq!(radio.io.tx.len, 0);

        let max_payload = [0u8; 127];
        assert!(radio.send(&max_payload));
        assert_eq!(radio.io.tx.data[1], 128);

        let mut raw = Cc2530::new(FakeUart {
            rx: HeaplessVec::default(),
            rx_pos: 0,
            tx: HeaplessVec::default(),
        });
        let uart_max_plus_one = [0u8; 255];
        assert!(!raw.send_cmd(0x03, &uart_max_plus_one));
        assert_eq!(raw.io.tx.len, 0);
    }

    #[test]
    fn rfid_bcc_checks_anticollision_frames() {
        let uid = [0xDE, 0xAD, 0xBE, 0xEF];
        let bcc = rfid::iso14443a_bcc(&uid);
        assert!(rfid::validate_anticollision(&[0xDE, 0xAD, 0xBE, 0xEF, bcc]));
        assert!(!rfid::validate_anticollision(&[
            0xDE,
            0xAD,
            0xBE,
            0xEF,
            bcc ^ 1
        ]));
        assert!(rfid::has_next_cascade(&[
            rfid::CASCADE_TAG,
            1,
            2,
            3,
            0x88 ^ 1 ^ 2 ^ 3
        ]));
    }

    #[derive(Clone, Copy)]
    enum RfidReply {
        Atqa,
        Uid,
        None,
    }

    struct FakeSpi {
        regs: [u8; 64],
        fifo: [u8; 64],
        fifo_len: usize,
        wrote_request_a: bool,
        wrote_anticoll: bool,
        transfer_calls: usize,
        fail_on_call: usize,
    }

    impl Default for FakeSpi {
        fn default() -> Self {
            Self {
                regs: [0; 64],
                fifo: [0; 64],
                fifo_len: 0,
                wrote_request_a: false,
                wrote_anticoll: false,
                transfer_calls: 0,
                fail_on_call: 0,
            }
        }
    }

    impl FakeSpi {
        fn prepare_reply(&mut self, reply: RfidReply) {
            match reply {
                RfidReply::Atqa => {
                    self.fifo[..2].copy_from_slice(&[0x04, 0x00]);
                    self.fifo_len = 2;
                }
                RfidReply::Uid => {
                    let uid = [0xDE, 0xAD, 0xBE, 0xEF];
                    self.fifo[..4].copy_from_slice(&uid);
                    self.fifo[4] = rfid::iso14443a_bcc(&uid);
                    self.fifo_len = 5;
                }
                RfidReply::None => self.fifo_len = 0,
            }
            self.regs[reg::FIFO_LEVEL as usize] = self.fifo_len as u8;
            self.regs[reg::COMM_IRQ as usize] = 0x30;
            self.regs[reg::ERROR as usize] = 0;
        }
    }

    impl SpiIo for FakeSpi {
        fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> bool {
            self.transfer_calls += 1;
            if self.fail_on_call == self.transfer_calls {
                return false;
            }
            if write.len() != 2 || read.len() != 2 {
                return false;
            }
            let reg = (write[0] >> 1) & 0x3F;
            if write[0] & 0x80 != 0 {
                read[1] = if reg == super::reg::FIFO_DATA {
                    let b = self.fifo[0];
                    if self.fifo_len > 0 {
                        let mut i = 1;
                        while i < self.fifo_len {
                            self.fifo[i - 1] = self.fifo[i];
                            i += 1;
                        }
                        self.fifo_len -= 1;
                        self.regs[super::reg::FIFO_LEVEL as usize] = self.fifo_len as u8;
                    }
                    b
                } else {
                    self.regs[reg as usize]
                };
            } else {
                self.regs[reg as usize] = write[1];
                if reg == super::reg::FIFO_LEVEL && write[1] & 0x80 != 0 {
                    self.fifo_len = 0;
                    self.wrote_request_a = false;
                    self.wrote_anticoll = false;
                }
                if reg == super::reg::FIFO_DATA {
                    self.fifo[self.fifo_len] = write[1];
                    self.fifo_len += 1;
                    if write[1] == 0x26 {
                        self.wrote_request_a = true;
                    }
                    if self.fifo_len >= 2 && self.fifo[0] == 0x93 && self.fifo[1] == 0x20 {
                        self.wrote_anticoll = true;
                    }
                }
                if reg == super::reg::COMMAND && write[1] == super::cmd::TRANSCEIVE {
                    if self.wrote_anticoll {
                        self.prepare_reply(RfidReply::Uid);
                    } else if self.wrote_request_a {
                        self.prepare_reply(RfidReply::Atqa);
                    } else {
                        self.prepare_reply(RfidReply::None);
                    }
                }
            }
            true
        }
    }

    #[test]
    fn mfrc522_backend_polls_uid_and_mounts_as_wireless_backend() {
        let spi = FakeSpi::default();
        let mut reader = Mfrc522::new(spi);
        assert_eq!(reader.link_state(), LinkState::Down);
        let mut out = [0u8; 10];
        assert!(!reader.send(&[0x26]));
        assert_eq!(reader.recv(&mut out), 0);
        assert_eq!(reader.spi.transfer_calls, 0);
        assert!(reader.init().is_ok());
        assert_eq!(reader.link_state(), LinkState::Up);
        assert_eq!(reader.descriptor().protocol, Protocol::Rfid);
        assert_eq!(reader.reader_descriptor(), rfid_readers::MFRC522_SPI);
        let calls_before_oversized_send = reader.spi.transfer_calls;
        assert!(!reader.send(&[0u8; 19]));
        assert_eq!(reader.spi.transfer_calls, calls_before_oversized_send);

        let uid = reader.poll_uid(1).expect("uid");
        assert_eq!(uid.as_slice(), &[0xDE, 0xAD, 0xBE, 0xEF]);

        let mut too_small = [0u8; 2];
        assert_eq!(reader.recv(&mut too_small), 0);
        assert_eq!(reader.rx_len, 4);
        let calls_after_poll = reader.spi.transfer_calls;
        assert_eq!(reader.recv(&mut out), 4);
        assert_eq!(&out[..4], &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(reader.spi.transfer_calls, calls_after_poll);

        reader.rx_cache[..3].copy_from_slice(&[1, 2, 3]);
        reader.rx_len = 3;
        reader.spi.fail_on_call = reader.spi.transfer_calls + 1;
        assert_eq!(reader.init(), Err(RfidError::Bus));
        assert_eq!(reader.link_state(), LinkState::Down);
        assert_eq!(reader.rx_len, 0);
        assert_eq!(reader.rx_cache, [0; 18]);
        let calls_after_failed_init = reader.spi.transfer_calls;
        assert!(!reader.send(&[0x26]));
        assert_eq!(reader.recv(&mut out), 0);
        assert_eq!(reader.spi.transfer_calls, calls_after_failed_init);
    }
    // A tiny in-memory transport proves the trait surface composes.
    struct LoopbackRadio {
        buf: [u8; 64],
        len: usize,
    }
    impl WirelessBackend for LoopbackRadio {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::NRF_PROPRIETARY
        }
        fn link_state(&mut self) -> LinkState {
            LinkState::Up
        }
        fn send(&mut self, payload: &[u8]) -> bool {
            if payload.len() > usize::from(self.descriptor().mtu) {
                return false;
            }
            self.buf[..payload.len()].copy_from_slice(payload);
            self.len = payload.len();
            true
        }
        fn recv(&mut self, buf: &mut [u8]) -> usize {
            let n = self.len.min(buf.len());
            buf[..n].copy_from_slice(&self.buf[..n]);
            self.len = 0;
            n
        }
    }

    struct HostileCountRadio;

    impl WirelessBackend for HostileCountRadio {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::NRF_PROPRIETARY
        }

        fn link_state(&mut self) -> LinkState {
            LinkState::Up
        }

        fn send(&mut self, _payload: &[u8]) -> bool {
            true
        }

        fn recv(&mut self, buf: &mut [u8]) -> usize {
            buf.len().saturating_add(1)
        }
    }

    #[test]
    fn transport_trait_round_trips() {
        let mut radio = LoopbackRadio {
            buf: [0; 64],
            len: 0,
        };
        assert_eq!(radio.link_state(), LinkState::Up);
        assert!(radio.send(b"nobro-wireless"));
        assert!(!radio.send(&[0u8; 61])); // over the proprietary MTU
        let mut rx = [0u8; 64];
        assert_eq!(radio.recv(&mut rx), 14);
        assert_eq!(&rx[..14], b"nobro-wireless");
        assert_eq!(radio.recv(&mut rx), 0); // drained
    }

    #[test]
    fn managed_link_enforces_deadline_mtu_and_window() {
        let backend = LoopbackRadio {
            buf: [0; 64],
            len: 0,
        };
        let mut link = ManagedLink::new(backend, LinkBudget::new(32, 1, 8));
        assert_eq!(
            link.send_at(11, TxContract::by(10), b"late"),
            Err(LinkError::DeadlineElapsed)
        );
        assert_eq!(
            link.send_at(1, TxContract::by(10), &[0; 33]),
            Err(LinkError::PayloadTooLarge)
        );
        assert!(link.send_at(1, TxContract::by(10), b"12345678").is_ok());
        assert_eq!(
            link.send_at(1, TxContract::by(10), b"x"),
            Err(LinkError::WindowExhausted)
        );
        link.reset_window();
        assert!(link.send_at(2, TxContract::by(10), b"x").is_ok());
        let mut received = [0u8; 8];
        assert_eq!(link.recv(&mut received), 1);
        assert_eq!(link.diagnostics().tx_accepted, 2);
        assert_eq!(link.diagnostics().tx_rejected, 3);
        assert_eq!(link.diagnostics().rx_packets, 1);
        assert_eq!(link.diagnostics().rx_rejected, 0);
    }

    #[test]
    fn managed_link_rejects_hostile_receive_counts() {
        let mut link = ManagedLink::new(HostileCountRadio, LinkBudget::new(8, 1, 8));
        let mut destination = [0u8; 4];
        assert_eq!(link.recv(&mut destination), 0);
        assert_eq!(link.diagnostics().rx_packets, 0);
        assert_eq!(link.diagnostics().rx_rejected, 1);
    }
}
