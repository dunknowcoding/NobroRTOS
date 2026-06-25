//! Bounded ROS-style IMU topic bridge (`RosBridgeSal`).
//!
//! Demonstrates NobroRTOS's "ROS bridges stay bounded and off the hard-realtime
//! path" pillar with a real, no-heap implementation: `publish` copies a bounded
//! message into a fixed-capacity ring (dropping + counting on overflow rather than
//! blocking or allocating), and `pump` hands one queued message to the transport.
//! It declares a `RosBridgeContract` (one topic, fixed depth and message size) so a
//! host can inspect the bridge's total buffer demand before it runs.
//!
//! This is NOT a full micro-ROS / XRCE-DDS agent client; it is the bounded bridge
//! contract + queue discipline a transport plugs into. Portable: SAL-only deps.
#![no_std]

use nobro_sal::{RosBridgeContract, RosBridgeSal, RosBridgeTransport, RosTopicContract};

/// Topic + message-type identity for the IMU stream.
pub const TOPIC_IMU: u32 = 0x494D_5530; // "IMU0"
pub const MSG_IMU_TYPE: u32 = 0x494D_5554; // "IMUT"
pub const BRIDGE_ID: u32 = 0x4E42_524F; // "NBRO"

pub const DEPTH: usize = 8;
pub const MAX_MSG: usize = 24;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RosError {
    UnknownTopic,
    PayloadTooLarge,
    QueueFull,
    NoService,
}

pub struct RosImuBridge {
    buf: [[u8; MAX_MSG]; DEPTH],
    len: [u8; DEPTH],
    head: usize,
    tail: usize,
    count: usize,
    published: u32,
    transmitted: u32,
    dropped: u32,
    max_depth: u32,
}

impl Default for RosImuBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl RosImuBridge {
    pub const fn new() -> Self {
        Self {
            buf: [[0u8; MAX_MSG]; DEPTH],
            len: [0u8; DEPTH],
            head: 0,
            tail: 0,
            count: 0,
            published: 0,
            transmitted: 0,
            dropped: 0,
            max_depth: 0,
        }
    }

    pub fn published(&self) -> u32 {
        self.published
    }
    pub fn transmitted(&self) -> u32 {
        self.transmitted
    }
    pub fn dropped(&self) -> u32 {
        self.dropped
    }
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Hand one queued message to the transport. Returns the message length if one
    /// was sent, or 0 if the queue was empty. A real transport would write
    /// `msg` to the wire here; the demo counts it as transmitted.
    pub fn pump(&mut self) -> usize {
        if self.count == 0 {
            return 0;
        }
        let slot = self.head;
        let n = self.len[slot] as usize;
        // let _wire = &self.buf[slot][..n]; // transport write goes here
        self.head = (self.head + 1) % DEPTH;
        self.count -= 1;
        self.transmitted += 1;
        n
    }
}

impl RosBridgeSal for RosImuBridge {
    type Error = RosError;

    fn contract(&self) -> RosBridgeContract {
        RosBridgeContract::from_parts(
            RosBridgeTransport::Serial,
            BRIDGE_ID,
            &[RosTopicContract::new(
                TOPIC_IMU,
                MSG_IMU_TYPE,
                DEPTH as u8,
                MAX_MSG as u16,
            )],
            &[],
            &[],
            &[],
        )
    }

    fn publish(
        &mut self,
        topic_hash: u32,
        payload: &[u8],
        _deadline_us: u64,
    ) -> Result<(), Self::Error> {
        if topic_hash != TOPIC_IMU {
            return Err(RosError::UnknownTopic);
        }
        if payload.len() > MAX_MSG {
            return Err(RosError::PayloadTooLarge);
        }
        if self.count >= DEPTH {
            self.dropped += 1;
            return Err(RosError::QueueFull);
        }
        let slot = self.tail;
        self.buf[slot][..payload.len()].copy_from_slice(payload);
        self.len[slot] = payload.len() as u8;
        self.tail = (self.tail + 1) % DEPTH;
        self.count += 1;
        self.published += 1;
        if self.count as u32 > self.max_depth {
            self.max_depth = self.count as u32;
        }
        Ok(())
    }

    fn request(
        &mut self,
        _service_hash: u32,
        _request: &[u8],
        _response: &mut [u8],
        _deadline_us: u64,
    ) -> Result<usize, Self::Error> {
        Err(RosError::NoService)
    }
}
