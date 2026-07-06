//! AI deployment + cloud-API management for NobroRTOS.
//!
//! **Deploy half**: a [`ModelManifest`] describes an already-trained, quantized model
//! exported by host tools (torch/tensorflow/etc. -> flat int8 weights + scale/zero-point)
//! and is validated (magic, shape sanity, checksum) before any runtime binds to the
//! weight blob - a bad export fails loudly at load, not silently at inference.
//!
//! **Cloud half**: [`CloudSession`] manages a remote AI endpoint over any byte transport:
//! API keys are referenced by [`ApiKeyRef`] handles (slots in `nobro_secure`'s KeyStore -
//! raw secrets never pass through this crate), reconnects use bounded exponential
//! backoff, and a request budget keeps a chatty node from starving the link.
#![cfg_attr(not(test), no_std)]

// ---------------------------------------------------------------- model deployment

/// Supported weight encodings for deployed models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightFormat {
    /// Symmetric per-tensor int8 quantization (scale in milli-units).
    Int8 { scale_milli: u32 },
    /// Raw little-endian f32 (development / high-accuracy paths).
    F32,
}

/// Manifest a host exporter writes next to the weight blob. The device validates it
/// before binding a runtime.
#[derive(Clone, Copy, Debug)]
pub struct ModelManifest {
    pub magic: u32,
    pub name: &'static str,
    pub version: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub format: WeightFormat,
    /// FNV-1a of the weight blob.
    pub weights_crc: u32,
    pub weights_len: u32,
}

pub const MODEL_MAGIC: u32 = 0x4E42_4D4C; // "NBML"

/// FNV-1a checksum (shared with the exporter).
pub fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &b in bytes {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h
}

/// Why a deployment was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeployError {
    BadMagic,
    EmptyShape,
    LengthMismatch,
    ChecksumMismatch,
}

impl ModelManifest {
    /// Validate the manifest against the actual weight bytes.
    pub fn validate(&self, weights: &[u8]) -> Result<(), DeployError> {
        if self.magic != MODEL_MAGIC {
            return Err(DeployError::BadMagic);
        }
        if self.input_len == 0 || self.output_len == 0 {
            return Err(DeployError::EmptyShape);
        }
        if weights.len() != self.weights_len as usize {
            return Err(DeployError::LengthMismatch);
        }
        if fnv1a(weights) != self.weights_crc {
            return Err(DeployError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Dequantize one int8 weight according to the manifest format.
    pub fn dequant(&self, w: i8) -> f32 {
        match self.format {
            WeightFormat::Int8 { scale_milli } => f32::from(w) * (scale_milli as f32 / 1000.0),
            WeightFormat::F32 => f32::from(w),
        }
    }
}

// ---------------------------------------------------------------- cloud sessions

/// Handle to an API key held in a secure store (e.g. nobro_secure::KeyStore slot).
/// The raw secret never passes through this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiKeyRef {
    pub store_slot: u8,
}

/// Connection state of a cloud endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Idle,
    Connecting,
    Ready,
    /// Waiting out a backoff period (ms remaining is tracked by the session).
    Backoff,
}

/// Events the transport feeds into the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    ConnectStarted,
    ConnectOk,
    ConnectFailed,
    RemoteClosed,
}

/// Reconnect/backoff/budget policy for one cloud endpoint.
#[derive(Clone, Copy, Debug)]
pub struct CloudPolicy {
    pub backoff_base_ms: u32,
    pub backoff_max_ms: u32,
    /// Requests allowed per budget window.
    pub budget_per_window: u16,
}

impl Default for CloudPolicy {
    fn default() -> Self {
        CloudPolicy {
            backoff_base_ms: 500,
            backoff_max_ms: 60_000,
            budget_per_window: 30,
        }
    }
}

/// Cloud-endpoint session: state machine + backoff + request budget. Transport-agnostic:
/// the owner performs the actual I/O and reports [`LinkEvent`]s.
pub struct CloudSession {
    key: ApiKeyRef,
    policy: CloudPolicy,
    state: LinkState,
    consecutive_failures: u32,
    backoff_left_ms: u32,
    budget_used: u16,
}

impl CloudSession {
    pub fn new(key: ApiKeyRef, policy: CloudPolicy) -> Self {
        CloudSession {
            key,
            policy,
            state: LinkState::Idle,
            consecutive_failures: 0,
            backoff_left_ms: 0,
            budget_used: 0,
        }
    }

    pub fn state(&self) -> LinkState {
        self.state
    }
    pub fn key(&self) -> ApiKeyRef {
        self.key
    }

    /// Current backoff for the Nth consecutive failure: base * 2^(n-1), capped.
    pub fn backoff_ms(&self) -> u32 {
        if self.consecutive_failures == 0 {
            return 0;
        }
        let shift = (self.consecutive_failures - 1).min(16);
        (self.policy.backoff_base_ms << shift).min(self.policy.backoff_max_ms)
    }

    /// Feed a transport event; returns the new state.
    pub fn on_event(&mut self, ev: LinkEvent) -> LinkState {
        self.state = match ev {
            LinkEvent::ConnectStarted => LinkState::Connecting,
            LinkEvent::ConnectOk => {
                self.consecutive_failures = 0;
                self.backoff_left_ms = 0;
                LinkState::Ready
            }
            LinkEvent::ConnectFailed | LinkEvent::RemoteClosed => {
                self.consecutive_failures += 1;
                self.backoff_left_ms = self.backoff_ms();
                LinkState::Backoff
            }
        };
        self.state
    }

    /// Advance time; when a backoff expires the session returns to Idle so the owner
    /// may reconnect. Returns true when a reconnect attempt is now allowed.
    pub fn tick(&mut self, elapsed_ms: u32) -> bool {
        if self.state == LinkState::Backoff {
            self.backoff_left_ms = self.backoff_left_ms.saturating_sub(elapsed_ms);
            if self.backoff_left_ms == 0 {
                self.state = LinkState::Idle;
            }
        }
        self.state == LinkState::Idle
    }

    /// Try to take one request from the budget window (false = throttled).
    pub fn take_request(&mut self) -> bool {
        if self.state != LinkState::Ready || self.budget_used >= self.policy.budget_per_window {
            return false;
        }
        self.budget_used += 1;
        true
    }

    /// Start a new budget window (call on the window boundary, e.g. each minute).
    pub fn reset_budget(&mut self) {
        self.budget_used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(weights: &[u8]) -> ModelManifest {
        ModelManifest {
            magic: MODEL_MAGIC,
            name: "gesture-int8",
            version: 1,
            input_len: 96,
            output_len: 3,
            format: WeightFormat::Int8 { scale_milli: 39 },
            weights_crc: fnv1a(weights),
            weights_len: weights.len() as u32,
        }
    }

    #[test]
    fn manifest_accepts_good_and_rejects_bad() {
        let w = [1u8, 2, 3, 4, 5];
        let m = manifest(&w);
        assert!(m.validate(&w).is_ok());
        assert_eq!(m.validate(&w[..4]), Err(DeployError::LengthMismatch));
        let mut corrupted = w;
        corrupted[0] ^= 0xFF;
        assert_eq!(m.validate(&corrupted), Err(DeployError::ChecksumMismatch));
        let mut bad = m;
        bad.magic = 0;
        assert_eq!(bad.validate(&w), Err(DeployError::BadMagic));
        let mut empty = m;
        empty.output_len = 0;
        assert_eq!(empty.validate(&w), Err(DeployError::EmptyShape));
    }

    #[test]
    fn fnv1a_parity_with_the_host_exporter() {
        // bindings/python nn_export pins the same vector - the two sides must agree
        // or every export would be rejected at ChecksumMismatch.
        assert_eq!(fnv1a(b"nobro"), 0xA767_00F3);
    }

    #[test]
    fn dequant_applies_scale() {
        let m = manifest(&[0]);
        let v = m.dequant(100); // 100 * 0.039
        assert!((v - 3.9).abs() < 1e-4);
    }

    #[test]
    fn session_backoff_doubles_and_caps() {
        let mut s = CloudSession::new(
            ApiKeyRef { store_slot: 2 },
            CloudPolicy {
                backoff_base_ms: 100,
                backoff_max_ms: 800,
                budget_per_window: 5,
            },
        );
        assert_eq!(s.on_event(LinkEvent::ConnectFailed), LinkState::Backoff);
        assert_eq!(s.backoff_ms(), 100);
        s.on_event(LinkEvent::ConnectFailed);
        assert_eq!(s.backoff_ms(), 200);
        s.on_event(LinkEvent::ConnectFailed);
        s.on_event(LinkEvent::ConnectFailed);
        s.on_event(LinkEvent::ConnectFailed);
        assert_eq!(s.backoff_ms(), 800); // capped
                                         // success resets the failure streak
        s.on_event(LinkEvent::ConnectOk);
        assert_eq!(s.state(), LinkState::Ready);
        assert_eq!(s.backoff_ms(), 0);
    }

    #[test]
    fn session_waits_out_backoff_then_allows_reconnect() {
        let mut s = CloudSession::new(ApiKeyRef { store_slot: 0 }, CloudPolicy::default());
        s.on_event(LinkEvent::RemoteClosed); // 500 ms backoff
        assert!(!s.tick(200));
        assert!(!s.tick(200));
        assert!(s.tick(200)); // 600 ms elapsed -> Idle, reconnect allowed
        assert_eq!(s.state(), LinkState::Idle);
    }

    #[test]
    fn request_budget_throttles_until_reset() {
        let mut s = CloudSession::new(
            ApiKeyRef { store_slot: 1 },
            CloudPolicy {
                backoff_base_ms: 1,
                backoff_max_ms: 1,
                budget_per_window: 2,
            },
        );
        assert!(!s.take_request()); // not Ready yet
        s.on_event(LinkEvent::ConnectOk);
        assert!(s.take_request());
        assert!(s.take_request());
        assert!(!s.take_request()); // budget spent
        s.reset_budget();
        assert!(s.take_request());
    }
}
