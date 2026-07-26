//! Optional bounded Zigbee APS data service.
//!
//! This module implements unfragmented, unsecured APS data-frame encoding,
//! decoding, delivery-mode validation, APS counters, and duplicate rejection.
//! It deliberately requires an already joined Zigbee NWK provider. The
//! [`crate::Cc2530`] backend carries raw IEEE 802.15.4 PSDUs and does not
//! implement [`ApsNetworkBackend`], so enabling this feature cannot relabel that
//! raw transport as a Zigbee stack.

const APS_FRAME_TYPE_DATA: u8 = 0;
const APS_DELIVERY_UNICAST: u8 = 0;
const APS_DELIVERY_BROADCAST: u8 = 2;
const APS_DELIVERY_GROUP: u8 = 3;
const APS_SECURITY: u8 = 1 << 5;
const APS_ACK_REQUEST: u8 = 1 << 6;
const APS_EXTENDED_HEADER: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApsNetworkDestination {
    Unicast(u16),
    Broadcast(u16),
    Group(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsNetworkIndication {
    pub source: u16,
    pub destination: u16,
}

/// Joined Zigbee NWK data service required beneath APS.
///
/// A raw IEEE 802.15.4 MAC/PSDU driver is insufficient: network formation,
/// addressing, routing, and NWK security remain the provider's responsibility.
pub trait ApsNetworkBackend {
    type Error;

    fn backend_id(&self) -> &'static str;
    fn max_nsdu_bytes(&self) -> u16;
    fn joined(&mut self) -> bool;
    fn send_nsdu(
        &mut self,
        destination: ApsNetworkDestination,
        nsdu: &[u8],
    ) -> Result<(), Self::Error>;
    fn receive_nsdu(
        &mut self,
        output: &mut [u8],
    ) -> Result<Option<(usize, ApsNetworkIndication)>, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApsDelivery {
    Unicast { network: u16, endpoint: u8 },
    Broadcast { network: u16, endpoint: u8 },
    Group { group: u16 },
}

impl ApsDelivery {
    fn mode(self) -> u8 {
        match self {
            Self::Unicast { .. } => APS_DELIVERY_UNICAST,
            Self::Broadcast { .. } => APS_DELIVERY_BROADCAST,
            Self::Group { .. } => APS_DELIVERY_GROUP,
        }
    }

    fn network_destination(self) -> ApsNetworkDestination {
        match self {
            Self::Unicast { network, .. } => ApsNetworkDestination::Unicast(network),
            Self::Broadcast { network, .. } => ApsNetworkDestination::Broadcast(network),
            Self::Group { group } => ApsNetworkDestination::Group(group),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsDataRequest<'a> {
    pub delivery: ApsDelivery,
    pub profile_id: u16,
    pub cluster_id: u16,
    pub source_endpoint: u8,
    pub acknowledgement: bool,
    pub payload: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsCapabilities {
    pub backend_id: &'static str,
    pub max_nsdu_bytes: u16,
    pub duplicate_slots: u16,
    pub unicast: bool,
    pub broadcast: bool,
    pub group: bool,
    pub acknowledgement_request: bool,
    pub security: bool,
    pub fragmentation: bool,
}

impl ApsCapabilities {
    fn valid(self) -> bool {
        !self.backend_id.is_empty() && self.max_nsdu_bytes >= 8 && self.duplicate_slots != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsInstanceId(u16);

impl ApsInstanceId {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsMountReceipt {
    pub instance: ApsInstanceId,
    pub capabilities: ApsCapabilities,
    pub lifecycle_generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsSendReceipt {
    pub counter: u8,
    pub nsdu_bytes: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApsMessage {
    pub source_network: u16,
    pub destination_network: u16,
    pub delivery: ApsDelivery,
    pub profile_id: u16,
    pub cluster_id: u16,
    pub source_endpoint: u8,
    pub counter: u8,
    pub payload_bytes: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApsPoll {
    Empty,
    Duplicate,
    Message(ApsMessage),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApsError<E> {
    InvalidCapabilities,
    NotJoined,
    InvalidAddress,
    InvalidFrame,
    BufferTooSmall,
    PayloadTooLarge,
    UnsupportedSecurity,
    UnsupportedFragmentation,
    UnsupportedAcknowledgement,
    Backend(E),
}

#[derive(Debug)]
pub struct ApsMountError<B: ApsNetworkBackend> {
    backend: B,
    error: ApsError<B::Error>,
}

impl<B: ApsNetworkBackend> ApsMountError<B> {
    pub fn error(&self) -> &ApsError<B::Error> {
        &self.error
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[derive(Clone, Copy)]
struct DuplicateEntry {
    valid: bool,
    source: u16,
    counter: u8,
    seen_at_us: u64,
}

impl DuplicateEntry {
    const EMPTY: Self = Self {
        valid: false,
        source: 0,
        counter: 0,
        seen_at_us: 0,
    };
}

/// Mounted APS data service with a caller-selected fixed duplicate table.
pub struct ApsDataService<B: ApsNetworkBackend, const DUPLICATES: usize> {
    backend: B,
    next_counter: u8,
    duplicates: [DuplicateEntry; DUPLICATES],
    duplicate_head: usize,
    capabilities: ApsCapabilities,
}

impl<B: ApsNetworkBackend, const DUPLICATES: usize> ApsDataService<B, DUPLICATES> {
    pub fn mount(
        instance: ApsInstanceId,
        mut backend: B,
    ) -> Result<(Self, ApsMountReceipt), ApsMountError<B>> {
        let capabilities = ApsCapabilities {
            backend_id: backend.backend_id(),
            max_nsdu_bytes: backend.max_nsdu_bytes(),
            duplicate_slots: DUPLICATES.min(usize::from(u16::MAX)) as u16,
            unicast: true,
            broadcast: true,
            group: true,
            acknowledgement_request: true,
            security: false,
            fragmentation: false,
        };
        if !capabilities.valid() {
            return Err(ApsMountError {
                backend,
                error: ApsError::InvalidCapabilities,
            });
        }
        if !backend.joined() {
            return Err(ApsMountError {
                backend,
                error: ApsError::NotJoined,
            });
        }
        Ok((
            Self {
                backend,
                next_counter: 0,
                duplicates: [DuplicateEntry::EMPTY; DUPLICATES],
                duplicate_head: 0,
                capabilities,
            },
            ApsMountReceipt {
                instance,
                capabilities,
                lifecycle_generation: 1,
            },
        ))
    }

    pub const fn capabilities(&self) -> ApsCapabilities {
        self.capabilities
    }

    pub fn send(
        &mut self,
        request: ApsDataRequest<'_>,
        scratch: &mut [u8],
    ) -> Result<ApsSendReceipt, ApsError<B::Error>> {
        if !self.backend.joined() {
            return Err(ApsError::NotJoined);
        }
        if request.acknowledgement && !matches!(request.delivery, ApsDelivery::Unicast { .. }) {
            return Err(ApsError::UnsupportedAcknowledgement);
        }
        Self::validate_delivery(request.delivery, request.source_endpoint)?;

        let address_bytes = if matches!(request.delivery, ApsDelivery::Group { .. }) {
            2
        } else {
            1
        };
        let header_bytes = 1 + address_bytes + 2 + 2 + 1 + 1;
        let nsdu_bytes = header_bytes + request.payload.len();
        if nsdu_bytes > usize::from(self.capabilities.max_nsdu_bytes) {
            return Err(ApsError::PayloadTooLarge);
        }
        if nsdu_bytes > scratch.len() || nsdu_bytes > usize::from(u16::MAX) {
            return Err(ApsError::BufferTooSmall);
        }

        let mut frame_control = APS_FRAME_TYPE_DATA | (request.delivery.mode() << 2);
        if request.acknowledgement {
            frame_control |= APS_ACK_REQUEST;
        }
        scratch[0] = frame_control;
        let mut cursor = 1;
        match request.delivery {
            ApsDelivery::Unicast { endpoint, .. } | ApsDelivery::Broadcast { endpoint, .. } => {
                scratch[cursor] = endpoint;
                cursor += 1;
            }
            ApsDelivery::Group { group } => {
                scratch[cursor..cursor + 2].copy_from_slice(&group.to_le_bytes());
                cursor += 2;
            }
        }
        scratch[cursor..cursor + 2].copy_from_slice(&request.cluster_id.to_le_bytes());
        cursor += 2;
        scratch[cursor..cursor + 2].copy_from_slice(&request.profile_id.to_le_bytes());
        cursor += 2;
        scratch[cursor] = request.source_endpoint;
        cursor += 1;
        let counter = self.next_counter;
        self.next_counter = self.next_counter.wrapping_add(1);
        scratch[cursor] = counter;
        cursor += 1;
        scratch[cursor..nsdu_bytes].copy_from_slice(request.payload);

        self.backend
            .send_nsdu(
                request.delivery.network_destination(),
                &scratch[..nsdu_bytes],
            )
            .map_err(ApsError::Backend)?;
        Ok(ApsSendReceipt {
            counter,
            nsdu_bytes: nsdu_bytes as u16,
        })
    }

    pub fn poll(
        &mut self,
        now_us: u64,
        duplicate_ttl_us: u64,
        frame_scratch: &mut [u8],
        payload_output: &mut [u8],
    ) -> Result<ApsPoll, ApsError<B::Error>> {
        if duplicate_ttl_us == 0 {
            return Err(ApsError::InvalidFrame);
        }
        if !self.backend.joined() {
            return Err(ApsError::NotJoined);
        }
        let Some((frame_bytes, indication)) = self
            .backend
            .receive_nsdu(frame_scratch)
            .map_err(ApsError::Backend)?
        else {
            return Ok(ApsPoll::Empty);
        };
        if frame_bytes > frame_scratch.len() || frame_bytes < 8 {
            return Err(ApsError::InvalidFrame);
        }
        let frame = &frame_scratch[..frame_bytes];
        let control = frame[0];
        if control & APS_SECURITY != 0 {
            return Err(ApsError::UnsupportedSecurity);
        }
        if control & APS_EXTENDED_HEADER != 0 {
            return Err(ApsError::UnsupportedFragmentation);
        }
        if control & 0x03 != APS_FRAME_TYPE_DATA {
            return Err(ApsError::InvalidFrame);
        }
        let delivery_mode = (control >> 2) & 0x03;
        if delivery_mode == 1 {
            return Err(ApsError::InvalidFrame);
        }
        if delivery_mode != APS_DELIVERY_UNICAST && control & APS_ACK_REQUEST != 0 {
            return Err(ApsError::InvalidFrame);
        }

        let mut cursor = 1;
        let delivery = match delivery_mode {
            APS_DELIVERY_UNICAST => {
                let endpoint = frame[cursor];
                cursor += 1;
                ApsDelivery::Unicast {
                    network: indication.destination,
                    endpoint,
                }
            }
            APS_DELIVERY_BROADCAST => {
                let endpoint = frame[cursor];
                cursor += 1;
                ApsDelivery::Broadcast {
                    network: indication.destination,
                    endpoint,
                }
            }
            APS_DELIVERY_GROUP => {
                if frame_bytes < 9 {
                    return Err(ApsError::InvalidFrame);
                }
                let group = u16::from_le_bytes([frame[cursor], frame[cursor + 1]]);
                cursor += 2;
                ApsDelivery::Group { group }
            }
            _ => return Err(ApsError::InvalidFrame),
        };
        if cursor + 6 > frame_bytes {
            return Err(ApsError::InvalidFrame);
        }
        let cluster_id = u16::from_le_bytes([frame[cursor], frame[cursor + 1]]);
        cursor += 2;
        let profile_id = u16::from_le_bytes([frame[cursor], frame[cursor + 1]]);
        cursor += 2;
        let source_endpoint = frame[cursor];
        cursor += 1;
        let counter = frame[cursor];
        cursor += 1;
        Self::validate_delivery(delivery, source_endpoint)?;
        let payload = &frame[cursor..];
        if payload.len() > payload_output.len() || payload.len() > usize::from(u16::MAX) {
            return Err(ApsError::BufferTooSmall);
        }
        if self.observe_duplicate(indication.source, counter, now_us, duplicate_ttl_us) {
            return Ok(ApsPoll::Duplicate);
        }
        payload_output[..payload.len()].copy_from_slice(payload);
        Ok(ApsPoll::Message(ApsMessage {
            source_network: indication.source,
            destination_network: indication.destination,
            delivery,
            profile_id,
            cluster_id,
            source_endpoint,
            counter,
            payload_bytes: payload.len() as u16,
        }))
    }

    fn validate_delivery(
        delivery: ApsDelivery,
        source_endpoint: u8,
    ) -> Result<(), ApsError<B::Error>> {
        // Endpoint 0 is the ZDO data interface, 1..=240 are application
        // endpoints, and 241..=254 are reserved. A source can never use the
        // broadcast endpoint (255).
        if source_endpoint > 0xF0 {
            return Err(ApsError::InvalidAddress);
        }
        match delivery {
            ApsDelivery::Unicast { network, endpoint }
                if network <= 0xFFF7 && (endpoint <= 0xF0 || endpoint == 0xFF) =>
            {
                Ok(())
            }
            ApsDelivery::Broadcast { network, endpoint }
                if (0xFFFC..=0xFFFF).contains(&network) && (1..=0xFF).contains(&endpoint) =>
            {
                Ok(())
            }
            ApsDelivery::Group { .. } => Ok(()),
            _ => Err(ApsError::InvalidAddress),
        }
    }

    fn observe_duplicate(&mut self, source: u16, counter: u8, now_us: u64, ttl_us: u64) -> bool {
        for entry in &mut self.duplicates {
            if entry.valid && now_us.saturating_sub(entry.seen_at_us) > ttl_us {
                entry.valid = false;
            }
            if entry.valid && entry.source == source && entry.counter == counter {
                return true;
            }
        }
        self.duplicates[self.duplicate_head] = DuplicateEntry {
            valid: true,
            source,
            counter,
            seen_at_us: now_us,
        };
        self.duplicate_head = (self.duplicate_head + 1) % DUPLICATES;
        false
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FakeNetwork {
        joined: bool,
        sent: [u8; 32],
        sent_len: usize,
        sent_to: Option<ApsNetworkDestination>,
        receive: [u8; 32],
        receive_len: usize,
        receive_ready: bool,
    }

    impl FakeNetwork {
        fn joined() -> Self {
            Self {
                joined: true,
                sent: [0; 32],
                sent_len: 0,
                sent_to: None,
                receive: [0; 32],
                receive_len: 0,
                receive_ready: false,
            }
        }
    }

    impl ApsNetworkBackend for FakeNetwork {
        type Error = ();

        fn backend_id(&self) -> &'static str {
            "joined-zigbee-nwk"
        }

        fn max_nsdu_bytes(&self) -> u16 {
            32
        }

        fn joined(&mut self) -> bool {
            self.joined
        }

        fn send_nsdu(
            &mut self,
            destination: ApsNetworkDestination,
            nsdu: &[u8],
        ) -> Result<(), Self::Error> {
            self.sent[..nsdu.len()].copy_from_slice(nsdu);
            self.sent_len = nsdu.len();
            self.sent_to = Some(destination);
            Ok(())
        }

        fn receive_nsdu(
            &mut self,
            output: &mut [u8],
        ) -> Result<Option<(usize, ApsNetworkIndication)>, Self::Error> {
            if !self.receive_ready {
                return Ok(None);
            }
            output[..self.receive_len].copy_from_slice(&self.receive[..self.receive_len]);
            self.receive_ready = false;
            Ok(Some((
                self.receive_len,
                ApsNetworkIndication {
                    source: 0x1234,
                    destination: 0x5678,
                },
            )))
        }
    }

    fn mount() -> ApsDataService<FakeNetwork, 2> {
        ApsDataService::mount(ApsInstanceId::new(7), FakeNetwork::joined())
            .unwrap()
            .0
    }

    #[test]
    fn unicast_data_frame_has_exact_field_order_and_receipt() {
        let mut aps = mount();
        let mut scratch = [0; 32];
        let receipt = aps
            .send(
                ApsDataRequest {
                    delivery: ApsDelivery::Unicast {
                        network: 0x3344,
                        endpoint: 2,
                    },
                    profile_id: 0x0104,
                    cluster_id: 0x0006,
                    source_endpoint: 1,
                    acknowledgement: true,
                    payload: &[0xAA, 0x55],
                },
                &mut scratch,
            )
            .unwrap();
        assert_eq!(receipt.counter, 0);
        assert_eq!(receipt.nsdu_bytes, 10);
        let backend = aps.into_backend();
        assert_eq!(
            backend.sent_to,
            Some(ApsNetworkDestination::Unicast(0x3344))
        );
        assert_eq!(
            &backend.sent[..backend.sent_len],
            &[0x40, 2, 0x06, 0x00, 0x04, 0x01, 1, 0, 0xAA, 0x55]
        );
    }

    #[test]
    fn group_ack_is_rejected_and_raw_transport_cannot_be_mounted_as_aps() {
        let mut aps = mount();
        let mut scratch = [0; 32];
        assert_eq!(
            aps.send(
                ApsDataRequest {
                    delivery: ApsDelivery::Group { group: 0x2222 },
                    profile_id: 0x0104,
                    cluster_id: 6,
                    source_endpoint: 1,
                    acknowledgement: true,
                    payload: &[1],
                },
                &mut scratch,
            ),
            Err(ApsError::UnsupportedAcknowledgement)
        );
        // Compile-time separation is the important assertion: Cc2530 does not
        // implement ApsNetworkBackend, so there is no conversion tested here.
    }

    #[test]
    fn address_rules_and_zero_length_asdu_fail_closed_correctly() {
        let mut aps = mount();
        let mut scratch = [0; 32];
        let receipt = aps
            .send(
                ApsDataRequest {
                    delivery: ApsDelivery::Unicast {
                        network: 0x1234,
                        endpoint: 2,
                    },
                    profile_id: 0x0104,
                    cluster_id: 6,
                    source_endpoint: 1,
                    acknowledgement: false,
                    payload: &[],
                },
                &mut scratch,
            )
            .unwrap();
        assert_eq!(receipt.nsdu_bytes, 8);
        for request in [
            ApsDataRequest {
                delivery: ApsDelivery::Unicast {
                    network: 0xFFF8,
                    endpoint: 2,
                },
                profile_id: 0x0104,
                cluster_id: 6,
                source_endpoint: 1,
                acknowledgement: false,
                payload: &[1],
            },
            ApsDataRequest {
                delivery: ApsDelivery::Broadcast {
                    network: 0x1234,
                    endpoint: 0xFF,
                },
                profile_id: 0x0104,
                cluster_id: 6,
                source_endpoint: 1,
                acknowledgement: false,
                payload: &[1],
            },
            ApsDataRequest {
                delivery: ApsDelivery::Group { group: 1 },
                profile_id: 0x0104,
                cluster_id: 6,
                source_endpoint: 0xFF,
                acknowledgement: false,
                payload: &[1],
            },
            ApsDataRequest {
                delivery: ApsDelivery::Unicast {
                    network: 0x1234,
                    endpoint: 0xF1,
                },
                profile_id: 0x0104,
                cluster_id: 6,
                source_endpoint: 1,
                acknowledgement: false,
                payload: &[1],
            },
            ApsDataRequest {
                delivery: ApsDelivery::Broadcast {
                    network: 0xFFFB,
                    endpoint: 1,
                },
                profile_id: 0x0104,
                cluster_id: 6,
                source_endpoint: 1,
                acknowledgement: false,
                payload: &[1],
            },
        ] {
            assert_eq!(
                aps.send(request, &mut scratch),
                Err(ApsError::InvalidAddress)
            );
        }
    }

    #[test]
    fn receive_decodes_and_rejects_duplicate_until_ttl_expires() {
        let mut fake = FakeNetwork::joined();
        fake.receive[..10].copy_from_slice(&[0x00, 2, 0x06, 0, 0x04, 1, 1, 9, 0xAB, 0xCD]);
        fake.receive_len = 10;
        fake.receive_ready = true;
        let (mut aps, receipt) =
            ApsDataService::<_, 2>::mount(ApsInstanceId::new(3), fake).unwrap();
        assert_eq!(receipt.capabilities.duplicate_slots, 2);
        assert!(!receipt.capabilities.security);
        let mut frame = [0; 32];
        let mut payload = [0; 8];
        let message = aps.poll(100, 1_000, &mut frame, &mut payload).unwrap();
        assert!(matches!(
            message,
            ApsPoll::Message(ApsMessage {
                source_network: 0x1234,
                profile_id: 0x0104,
                cluster_id: 0x0006,
                counter: 9,
                payload_bytes: 2,
                ..
            })
        ));
        assert_eq!(&payload[..2], &[0xAB, 0xCD]);

        let mut fake = aps.into_backend();
        fake.receive_ready = true;
        let (mut aps, _) = ApsDataService::<_, 2>::mount(ApsInstanceId::new(3), fake).unwrap();
        // Seed the new service once, then replay the same source/counter.
        assert!(matches!(
            aps.poll(100, 1_000, &mut frame, &mut payload).unwrap(),
            ApsPoll::Message(_)
        ));
        aps.backend.receive_ready = true;
        assert_eq!(
            aps.poll(200, 1_000, &mut frame, &mut payload),
            Ok(ApsPoll::Duplicate)
        );
        aps.backend.receive_ready = true;
        assert!(matches!(
            aps.poll(1_202, 1_000, &mut frame, &mut payload).unwrap(),
            ApsPoll::Message(_)
        ));
    }

    #[test]
    fn short_payload_buffer_does_not_consume_duplicate_identity() {
        let mut fake = FakeNetwork::joined();
        fake.receive[..10].copy_from_slice(&[0x00, 2, 0x06, 0, 0x04, 1, 1, 9, 0xAB, 0xCD]);
        fake.receive_len = 10;
        fake.receive_ready = true;
        let (mut aps, _) = ApsDataService::<_, 2>::mount(ApsInstanceId::new(3), fake).unwrap();
        let mut frame = [0; 32];
        let mut too_short = [0; 1];
        assert_eq!(
            aps.poll(100, 1_000, &mut frame, &mut too_short),
            Err(ApsError::BufferTooSmall)
        );
        aps.backend.receive_ready = true;
        let mut payload = [0; 2];
        assert!(matches!(
            aps.poll(101, 1_000, &mut frame, &mut payload).unwrap(),
            ApsPoll::Message(_)
        ));
        assert_eq!(payload, [0xAB, 0xCD]);
    }

    #[test]
    fn mount_fails_closed_without_joined_nwk_or_duplicate_capacity() {
        let mut not_joined = FakeNetwork::joined();
        not_joined.joined = false;
        let err = ApsDataService::<_, 1>::mount(ApsInstanceId::new(0), not_joined)
            .err()
            .unwrap();
        assert_eq!(err.error(), &ApsError::NotJoined);

        let err = ApsDataService::<_, 0>::mount(ApsInstanceId::new(0), FakeNetwork::joined())
            .err()
            .unwrap();
        assert_eq!(err.error(), &ApsError::InvalidCapabilities);
    }
}
