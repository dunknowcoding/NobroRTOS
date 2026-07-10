//! No-heap networking primitives for multi-board NobroRTOS meshes.
//! **Scope, honestly:** this is a mesh/link-layer toolkit (routing, sync, rollup,
//! store-and-forward, OTA chunking, formation) - NOT an IP stack. There is no
//! TCP/UDP/DHCP/socket layer here; IP-facing nodes pair a NobroRTOS radio node with
//! a bridge (see tools/dev/radio_wifi_bridge.py) or an IP-capable co-processor.
//!
//! - [`RoutingTable`] - bounded distance-vector routing (next hop by lowest cost) (M52)
//! - [`TimeSync`] - round-trip clock offset/delay estimation (NTP-style) (M53)
//! - [`Aggregator`] - bounded rollup of node readings (count/sum/min/max/mean) (M54)
#![cfg_attr(not(test), no_std)]

/// One routing entry: reach `dest` via `next_hop` at `cost` hops, freshness `seq`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub dest: u16,
    pub next_hop: u16,
    pub cost: u8,
    pub seq: u8,
}

/// Fixed-capacity distance-vector routing table.
pub struct RoutingTable<const N: usize> {
    routes: [Option<Route>; N],
}

impl<const N: usize> Default for RoutingTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RoutingTable<N> {
    pub const fn new() -> Self {
        Self { routes: [None; N] }
    }

    fn find(&self, dest: u16) -> Option<usize> {
        self.routes
            .iter()
            .position(|r| matches!(r, Some(rt) if rt.dest == dest))
    }

    /// Offer a route; accept it if new, strictly cheaper, or fresher (higher seq).
    pub fn update(&mut self, dest: u16, next_hop: u16, cost: u8, seq: u8) -> bool {
        let cand = Route {
            dest,
            next_hop,
            cost,
            seq,
        };
        if let Some(i) = self.find(dest) {
            let cur = self.routes[i].unwrap();
            let better = seq > cur.seq || (seq == cur.seq && cost < cur.cost);
            if better {
                self.routes[i] = Some(cand);
            }
            return better;
        }
        if let Some(slot) = self.routes.iter_mut().find(|r| r.is_none()) {
            *slot = Some(cand);
            return true;
        }
        false
    }

    /// Integrate a neighbor's advertised routes: each becomes reachable via that
    /// neighbor at cost+1.
    pub fn integrate_from(&mut self, neighbor: u16, adverts: &[Route]) -> u32 {
        let mut changed = 0;
        for a in adverts {
            if self.update(a.dest, neighbor, a.cost.saturating_add(1), a.seq) {
                changed += 1;
            }
        }
        changed
    }

    pub fn next_hop(&self, dest: u16) -> Option<u16> {
        self.find(dest).map(|i| self.routes[i].unwrap().next_hop)
    }

    pub fn cost(&self, dest: u16) -> Option<u8> {
        self.find(dest).map(|i| self.routes[i].unwrap().cost)
    }

    pub fn len(&self) -> usize {
        self.routes.iter().filter(|r| r.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Round-trip clock synchronization (NTP/PTP style).
pub struct TimeSync;

impl TimeSync {
    /// From a 4-timestamp exchange (local send t0, remote recv t1, remote send t2,
    /// local recv t3), the clock `offset` (remote - local) and one-way `delay`.
    pub fn estimate(t0: i64, t1: i64, t2: i64, t3: i64) -> (i64, i64) {
        let offset = ((t1 - t0) + (t2 - t3)) / 2;
        let delay = (t3 - t0) - (t2 - t1);
        (offset, delay)
    }
}

/// Bounded streaming aggregate of node readings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aggregator {
    pub count: u32,
    pub sum: i64,
    pub min: i64,
    pub max: i64,
}

impl Aggregator {
    pub const fn new() -> Self {
        Self {
            count: 0,
            sum: 0,
            min: i64::MAX,
            max: i64::MIN,
        }
    }
    pub fn add(&mut self, v: i64) {
        self.count += 1;
        self.sum += v;
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
    }
    pub fn mean(&self) -> i64 {
        if self.count == 0 {
            0
        } else {
            self.sum / i64::from(self.count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_converges_multihop_and_prefers_cheaper() {
        // collector=0; node2 advertises dest 5 at cost 0 (itself); we reach via node2.
        let mut t = RoutingTable::<8>::new();
        t.integrate_from(
            2,
            &[Route {
                dest: 5,
                next_hop: 5,
                cost: 0,
                seq: 1,
            }],
        );
        assert_eq!(t.next_hop(5), Some(2));
        assert_eq!(t.cost(5), Some(1)); // +1 hop via node2
                                        // a cheaper direct route (cost 0 via node5) wins.
        assert!(t.update(5, 5, 0, 1));
        assert_eq!(t.next_hop(5), Some(5));
        // a stale, cheaper route is rejected; a fresher one is accepted.
        assert!(!t.update(5, 9, 0, 0));
        assert!(t.update(5, 9, 3, 2));
        assert_eq!(t.next_hop(5), Some(9));
    }

    #[test]
    fn timesync_recovers_offset_and_delay() {
        // remote clock +100 ahead; 10-unit one-way (20 round-trip), no processing.
        let (off, delay) = TimeSync::estimate(0, 110, 110, 20);
        assert_eq!(off, 100);
        assert_eq!(delay, 20); // round-trip; one-way = delay/2 = 10
    }

    #[test]
    fn aggregator_rolls_up() {
        let mut a = Aggregator::new();
        for v in [25_600i64, 22_400, 96_000] {
            a.add(v);
        }
        assert_eq!(a.count, 3);
        assert_eq!(a.min, 22_400);
        assert_eq!(a.max, 96_000);
        assert_eq!(a.mean(), 48_000);
    }
}

/// Broadcast/gossip dedup: a bounded set of recently-seen message ids, so a relay
/// forwards each broadcast at most once (loop suppression). (M58)
pub struct SeenSet<const N: usize> {
    ids: [u32; N],
    head: usize,
    len: usize,
}

impl<const N: usize> Default for SeenSet<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> SeenSet<N> {
    pub const fn new() -> Self {
        Self {
            ids: [0; N],
            head: 0,
            len: 0,
        }
    }
    /// Record `id`; returns true if it is NEW (should be forwarded), false if a dup.
    pub fn observe(&mut self, id: u32) -> bool {
        if self.ids[..self.len].contains(&id) {
            return false;
        }
        self.ids[self.head] = id;
        self.head = (self.head + 1) % N;
        if self.len < N {
            self.len += 1;
        }
        true
    }
}

/// Bounded priority queue for outgoing frames - higher `prio` leaves first (M59 QoS).
pub struct PrioQueue<T: Copy, const N: usize> {
    items: [Option<(u8, T)>; N],
    len: usize,
}

impl<T: Copy, const N: usize> Default for PrioQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> PrioQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub fn push(&mut self, prio: u8, item: T) -> bool {
        if self.len >= N {
            return false;
        }
        self.items[self.len] = Some((prio, item));
        self.len += 1;
        true
    }
    /// Pop the highest-priority item.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let mut best = 0usize;
        for i in 1..self.len {
            if self.items[i].unwrap().0 > self.items[best].unwrap().0 {
                best = i;
            }
        }
        let (_, item) = self.items[best].unwrap();
        self.items[best] = self.items[self.len - 1];
        self.items[self.len - 1] = None;
        self.len -= 1;
        Some(item)
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod net_extra_tests {
    use super::*;

    #[test]
    fn seen_set_dedups_broadcasts() {
        let mut s = SeenSet::<4>::new();
        assert!(s.observe(100)); // new -> forward
        assert!(!s.observe(100)); // dup -> suppress
        assert!(s.observe(101));
    }

    #[test]
    fn prio_queue_serves_highest_first() {
        let mut q = PrioQueue::<u16, 4>::new();
        q.push(1, 0xAA);
        q.push(9, 0xBB); // urgent
        q.push(5, 0xCC);
        assert_eq!(q.pop(), Some(0xBB));
        assert_eq!(q.pop(), Some(0xCC));
        assert_eq!(q.pop(), Some(0xAA));
        assert_eq!(q.pop(), None);
    }
}

/// Link-liveness events for mesh partition/reconnect handling (M57).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    None,
    Joined,
    Reconnected,
}

#[derive(Clone, Copy)]
struct LinkState {
    id: u16,
    last_us: u64,
    up: bool,
}

/// Per-neighbor link monitor: a neighbor is up while heard within `timeout_us`; missing
/// it past the timeout is a partition, and hearing it again is a reconnect. (M57)
pub struct LinkMonitor<const N: usize> {
    nodes: [Option<LinkState>; N],
    timeout_us: u64,
}

impl<const N: usize> LinkMonitor<N> {
    pub const fn new(timeout_us: u64) -> Self {
        Self {
            nodes: [None; N],
            timeout_us,
        }
    }

    fn find(&mut self, id: u16) -> Option<&mut LinkState> {
        self.nodes
            .iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|s| s.id == id)
    }

    /// Record a heartbeat from `id`; returns the resulting link event.
    pub fn heard(&mut self, id: u16, now_us: u64) -> LinkEvent {
        if let Some(st) = self.find(id) {
            let ev = if st.up {
                LinkEvent::None
            } else {
                LinkEvent::Reconnected
            };
            st.last_us = now_us;
            st.up = true;
            return ev;
        }
        if let Some(slot) = self.nodes.iter_mut().find(|s| s.is_none()) {
            *slot = Some(LinkState {
                id,
                last_us: now_us,
                up: true,
            });
            return LinkEvent::Joined;
        }
        LinkEvent::None
    }

    /// Mark any up node not heard within the timeout as down (partitioned); returns the
    /// count newly partitioned.
    pub fn tick(&mut self, now_us: u64) -> u32 {
        let mut downed = 0;
        for s in self.nodes.iter_mut().filter_map(|s| s.as_mut()) {
            if s.up && now_us.saturating_sub(s.last_us) > self.timeout_us {
                s.up = false;
                downed += 1;
            }
        }
        downed
    }

    pub fn is_up(&self, id: u16) -> bool {
        self.nodes
            .iter()
            .filter_map(|s| *s)
            .any(|s| s.id == id && s.up)
    }

    pub fn up_count(&self) -> usize {
        self.nodes
            .iter()
            .filter_map(|s| *s)
            .filter(|s| s.up)
            .count()
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn link_monitor_tracks_partition_and_reconnect() {
        let mut lm = LinkMonitor::<4>::new(1_000); // 1 ms liveness window
        assert_eq!(lm.heard(7, 0), LinkEvent::Joined);
        assert_eq!(lm.heard(7, 500), LinkEvent::None); // still up
        assert!(lm.is_up(7));
        assert_eq!(lm.tick(2_000), 1); // silent past timeout -> partitioned
        assert!(!lm.is_up(7));
        assert_eq!(lm.heard(7, 2_100), LinkEvent::Reconnected);
        assert!(lm.is_up(7));
        assert_eq!(lm.up_count(), 1);
    }
}

/// A unified reading from any node type, so heterogeneous boards (power, motion,
/// pressure, temperature) fuse into one schema. (M56)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeReading {
    Power { milliwatts: u32 },
    Motion { milli_g: u32 },
    Pressure { pascals: u32 },
    Temperature { milli_c: i32 },
}

/// Fused view of a heterogeneous mesh: a single rollup over mixed node readings. (M56)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshSnapshot {
    pub node_count: u32,
    pub total_power_mw: u32,
    pub motion_nodes: u32,
    pub max_motion_mg: u32,
}

impl MeshSnapshot {
    pub fn from_readings(readings: &[(u16, NodeReading)]) -> Self {
        let mut s = MeshSnapshot::default();
        for (_, r) in readings {
            s.node_count += 1;
            match r {
                NodeReading::Power { milliwatts } => {
                    s.total_power_mw = s.total_power_mw.saturating_add(*milliwatts);
                }
                NodeReading::Motion { milli_g } => {
                    s.motion_nodes += 1;
                    if *milli_g > s.max_motion_mg {
                        s.max_motion_mg = *milli_g;
                    }
                }
                NodeReading::Pressure { .. } | NodeReading::Temperature { .. } => {}
            }
        }
        s
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn heterogeneous_readings_fuse_into_one_snapshot() {
        let readings = [
            (1, NodeReading::Power { milliwatts: 1100 }),
            (2, NodeReading::Motion { milli_g: 1035 }),
            (3, NodeReading::Power { milliwatts: 950 }),
            (4, NodeReading::Pressure { pascals: 101_325 }),
            (5, NodeReading::Motion { milli_g: 1200 }),
        ];
        let snap = MeshSnapshot::from_readings(&readings);
        assert_eq!(snap.node_count, 5);
        assert_eq!(snap.total_power_mw, 2050);
        assert_eq!(snap.motion_nodes, 2);
        assert_eq!(snap.max_motion_mg, 1200);
    }
}

#[cfg(test)]
mod fault_injection_tests {
    use super::*;

    #[test]
    fn mesh_reroutes_around_injected_node_fault_and_recovers() {
        // reach dest 5 cheaply via node 2; inject a fault on node 2 (a fresher advert
        // installs a costlier backup via node 9); then node 2 recovers and is preferred
        // again. (M69)
        let mut rt = RoutingTable::<8>::new();
        rt.update(5, 2, 1, 1);
        assert_eq!(rt.next_hop(5), Some(2));
        // fault injected: node 2 path lost, backup via node 9 (newer seq) takes over
        assert!(rt.update(5, 9, 3, 2));
        assert_eq!(rt.next_hop(5), Some(9));
        // node 2 recovers with a fresh cheap route -> preferred again
        assert!(rt.update(5, 2, 1, 3));
        assert_eq!(rt.next_hop(5), Some(2));
        assert_eq!(rt.cost(5), Some(1));
    }
}

/// Link-key secured mesh frames (M133): AES-CCM authenticated encryption per link, with
/// a monotonically increasing sequence number folded into the nonce for anti-replay.
/// Wire layout: [src:2][dst:2][seq:4][ciphertext][tag:8]; src/dst/seq ride as AAD.
pub mod secure_link {
    use nobro_crypto::ccm;

    pub const HEADER_LEN: usize = 8;
    pub const OVERHEAD: usize = HEADER_LEN + ccm::TAG_LEN;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LinkError {
        BadLength,
        BadTag,
        Replay,
    }

    fn nonce(src: u16, dst: u16, seq: u32) -> [u8; ccm::NONCE_LEN] {
        let mut n = [0u8; ccm::NONCE_LEN];
        n[..2].copy_from_slice(&src.to_le_bytes());
        n[2..4].copy_from_slice(&dst.to_le_bytes());
        n[4..8].copy_from_slice(&seq.to_le_bytes());
        n
    }

    /// Seal `payload` from `src` to `dst` under `key` with sequence `seq`.
    pub fn seal(
        key: &[u8; 16],
        src: u16,
        dst: u16,
        seq: u32,
        payload: &[u8],
        out: &mut [u8],
    ) -> Result<usize, LinkError> {
        if out.len() < payload.len() + OVERHEAD {
            return Err(LinkError::BadLength);
        }
        out[..2].copy_from_slice(&src.to_le_bytes());
        out[2..4].copy_from_slice(&dst.to_le_bytes());
        out[4..8].copy_from_slice(&seq.to_le_bytes());
        let (aad, body) = out.split_at_mut(HEADER_LEN);
        let n = ccm::encrypt(key, &nonce(src, dst, seq), aad, payload, body)
            .map_err(|_| LinkError::BadLength)?;
        Ok(HEADER_LEN + n)
    }

    /// Open a sealed frame; `last_seq` is the replay floor for this link (frames with
    /// seq <= last_seq are rejected). Returns (src, seq, plaintext len).
    pub fn open(
        key: &[u8; 16],
        frame: &[u8],
        last_seq: u32,
        out: &mut [u8],
    ) -> Result<(u16, u32, usize), LinkError> {
        if frame.len() < OVERHEAD {
            return Err(LinkError::BadLength);
        }
        let src = u16::from_le_bytes([frame[0], frame[1]]);
        let dst = u16::from_le_bytes([frame[2], frame[3]]);
        let seq = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        let _ = dst;
        if seq <= last_seq {
            return Err(LinkError::Replay);
        }
        let n = ccm::decrypt(
            key,
            &nonce(src, dst, seq),
            &frame[..HEADER_LEN],
            &frame[HEADER_LEN..],
            out,
        )
        .map_err(|e| match e {
            nobro_crypto::ccm::CcmError::BadTag => LinkError::BadTag,
            _ => LinkError::BadLength,
        })?;
        Ok((src, seq, n))
    }
}

/// Telemetry compression (M186): delta + zigzag + LEB128 varint packing for sample
/// streams - typical slowly-varying sensor series compress 3-5x losslessly.
pub mod telemetry_pack {
    fn zigzag(v: i32) -> u32 {
        ((v << 1) ^ (v >> 31)) as u32
    }
    fn unzigzag(v: u32) -> i32 {
        ((v >> 1) as i32) ^ -((v & 1) as i32)
    }

    /// Pack samples as first-value + zigzag-varint deltas. Returns bytes written.
    pub fn pack(samples: &[i32], out: &mut [u8]) -> Option<usize> {
        let mut w = 0usize;
        let mut prev = 0i32;
        for (i, &s) in samples.iter().enumerate() {
            let mut v = zigzag(if i == 0 { s } else { s.wrapping_sub(prev) });
            prev = s;
            loop {
                let byte = (v & 0x7F) as u8;
                v >>= 7;
                if w >= out.len() {
                    return None;
                }
                out[w] = if v != 0 { byte | 0x80 } else { byte };
                w += 1;
                if v == 0 {
                    break;
                }
            }
        }
        Some(w)
    }

    /// Unpack into `out`; returns the number of samples recovered.
    pub fn unpack(data: &[u8], out: &mut [i32]) -> Option<usize> {
        let mut r = 0usize;
        let mut n = 0usize;
        let mut prev = 0i32;
        while r < data.len() && n < out.len() {
            let mut v = 0u32;
            let mut shift = 0;
            loop {
                if r >= data.len() {
                    return None;
                }
                let b = data[r];
                r += 1;
                v |= u32::from(b & 0x7F) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            let d = unzigzag(v);
            let s = if n == 0 { d } else { prev.wrapping_add(d) };
            out[n] = s;
            prev = s;
            n += 1;
        }
        Some(n)
    }
}

#[cfg(test)]
mod secure_and_pack_tests {
    use super::*;

    #[test]
    fn secure_link_roundtrip_replay_and_tamper() {
        let key = [7u8; 16];
        let mut frame = [0u8; 64];
        let n = secure_link::seal(&key, 1, 2, 10, b"hello mesh", &mut frame).unwrap();
        let mut out = [0u8; 32];
        let (src, seq, len) = secure_link::open(&key, &frame[..n], 9, &mut out).unwrap();
        assert_eq!((src, seq, &out[..len]), (1, 10, &b"hello mesh"[..]));
        assert_eq!(
            secure_link::open(&key, &frame[..n], 10, &mut out),
            Err(secure_link::LinkError::Replay)
        );
        let mut evil = frame;
        evil[secure_link::HEADER_LEN] ^= 1;
        assert_eq!(
            secure_link::open(&key, &evil[..n], 9, &mut out),
            Err(secure_link::LinkError::BadTag)
        );
    }

    #[test]
    fn telemetry_pack_roundtrip_and_compresses() {
        let mut series = [0i32; 64];
        for (i, s) in series.iter_mut().enumerate() {
            *s = 1000 + ((i as i32) % 7) - 3;
        }
        let mut packed = [0u8; 256];
        let n = telemetry_pack::pack(&series, &mut packed).unwrap();
        assert!(n < 64 * 4 / 3, "packed {n} bytes - not compressing");
        let mut out = [0i32; 64];
        let m = telemetry_pack::unpack(&packed[..n], &mut out).unwrap();
        assert_eq!(m, 64);
        assert_eq!(out, series);
    }
}

/// Synchronized capture across time-synced nodes (M129): given a node's clock offset
/// (from [`TimeSync`]) and a shared capture instant in coordinator time, compute the
/// local time at which this node should sample so all nodes capture together.
pub fn synced_capture_local_us(coordinator_capture_us: i64, clock_offset_us: i64) -> i64 {
    // local = coordinator - offset  (offset = remote_clock - local_clock)
    coordinator_capture_us - clock_offset_us
}

/// Multi-hop latency estimate (M134): sum the per-hop one-way delays along a path, plus a
/// per-hop processing budget. Returns total microseconds (saturating).
pub fn path_latency_us(per_hop_delay_us: &[u32], hop_processing_us: u32) -> u64 {
    let mut total = 0u64;
    for &d in per_hop_delay_us {
        total = total.saturating_add(u64::from(d));
        total = total.saturating_add(u64::from(hop_processing_us));
    }
    total
}

/// Tele-operation over the mesh (M157): an authenticated, replay-protected actuator
/// command sealed exactly like a secure link frame. A tele-op command is (channel, value)
/// which we pack and seal so only a peer holding the link key can drive the actuator, and
/// stale commands are rejected by sequence.
pub mod teleop {
    use super::secure_link::{self, LinkError};

    pub fn command(
        key: &[u8; 16],
        src: u16,
        dst: u16,
        seq: u32,
        channel: u8,
        value: i16,
        out: &mut [u8],
    ) -> Result<usize, LinkError> {
        let payload = [channel, (value >> 8) as u8, (value & 0xFF) as u8];
        secure_link::seal(key, src, dst, seq, &payload, out)
    }

    /// Returns (channel, value) if authentic + fresh.
    pub fn apply(key: &[u8; 16], frame: &[u8], last_seq: u32) -> Result<(u8, i16), LinkError> {
        let mut buf = [0u8; 8];
        let (_src, _seq, n) = secure_link::open(key, frame, last_seq, &mut buf)?;
        if n < 3 {
            return Err(LinkError::BadLength);
        }
        let value = ((buf[1] as i16) << 8) | (buf[2] as i16 & 0xFF);
        Ok((buf[0], value))
    }
}

#[cfg(test)]
mod final_net_tests {
    use super::*;

    #[test]
    fn synced_capture_aligns_nodes() {
        // coordinator wants a capture at t=1_000_000; a node whose clock is +250 ahead
        // must fire at local 999_750 so the instants coincide.
        assert_eq!(synced_capture_local_us(1_000_000, 250), 999_750);
    }

    #[test]
    fn path_latency_sums_hops() {
        // 3 hops of 500 us link + 100 us processing each = 1800 us
        assert_eq!(path_latency_us(&[500, 500, 500], 100), 1800);
        assert_eq!(path_latency_us(&[], 100), 0);
    }

    #[test]
    fn teleop_command_is_authenticated_and_replay_protected() {
        let key = [0x33u8; 16];
        let mut frame = [0u8; 32];
        let n = teleop::command(&key, 1, 9, 5, 2, -1200, &mut frame).unwrap();
        // authentic + fresh
        assert_eq!(teleop::apply(&key, &frame[..n], 4), Ok((2, -1200)));
        // replay (seq <= floor) rejected
        assert!(teleop::apply(&key, &frame[..n], 5).is_err());
        // wrong key rejected
        assert!(teleop::apply(&[0u8; 16], &frame[..n], 4).is_err());
    }
}

// ---- wireless protocol layer (M130/M131/M132/M135) ----

/// RSSI/LQI-aware next-hop selection (M130): among candidate neighbors that can reach the
/// destination, pick the one with the best link quality, breaking ties by hop cost. Link
/// quality is a 0..255 score (higher = better); returns the chosen neighbor id.
pub fn rssi_best_next_hop(candidates: &[(u16, u8, u8)]) -> Option<u16> {
    // (neighbor_id, link_quality, hop_cost)
    candidates
        .iter()
        .copied()
        .max_by(|a, b| {
            a.1.cmp(&b.1) // higher quality first
                .then(b.2.cmp(&a.2)) // then lower hop cost
        })
        .map(|(id, _, _)| id)
}

/// OTA image chunking for mesh delivery (M131): split by fixed chunk size, track which
/// chunks a receiver has, and report completion. Fixed capacity, no heap.
pub struct OtaReassembler<const CHUNKS: usize> {
    have: [bool; CHUNKS],
    total: usize,
    received: usize,
}

impl<const CHUNKS: usize> OtaReassembler<CHUNKS> {
    pub fn new(total_chunks: usize) -> Self {
        Self {
            have: [false; CHUNKS],
            total: total_chunks.min(CHUNKS),
            received: 0,
        }
    }
    /// Number of chunks an image of `image_len` bytes needs at `chunk_size`.
    pub fn chunk_count(image_len: usize, chunk_size: usize) -> usize {
        image_len.div_ceil(chunk_size.max(1))
    }
    /// Record a received chunk; returns true if it was new.
    pub fn receive(&mut self, index: usize) -> bool {
        if index >= self.total || self.have[index] {
            return false;
        }
        self.have[index] = true;
        self.received += 1;
        true
    }
    pub fn is_complete(&self) -> bool {
        self.received == self.total && self.total > 0
    }
    /// The first missing chunk index (for a NACK-driven retransmit).
    pub fn first_missing(&self) -> Option<usize> {
        (0..self.total).find(|&i| !self.have[i])
    }
    pub fn progress_percent(&self) -> u8 {
        ((self.received * 100).checked_div(self.total).unwrap_or(0)) as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetOtaPhase {
    Idle,
    Canary,
    Staged,
    Installing,
    Confirmed,
    RolledBack,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetOtaNode {
    pub id: u16,
    pub current_version: u32,
    pub target_version: u32,
    pub phase: FleetOtaPhase,
    pub healthy: bool,
    pub failures: u8,
}

impl FleetOtaNode {
    pub const fn new(id: u16, current_version: u32) -> Self {
        Self {
            id,
            current_version,
            target_version: 0,
            phase: FleetOtaPhase::Idle,
            healthy: true,
            failures: 0,
        }
    }

    pub const fn is_active(self) -> bool {
        matches!(
            self.phase,
            FleetOtaPhase::Canary | FleetOtaPhase::Staged | FleetOtaPhase::Installing
        )
    }

    pub const fn is_eligible(self, target_version: u32, max_failures: u8) -> bool {
        self.healthy
            && self.current_version != target_version
            && self.failures < max_failures
            && matches!(self.phase, FleetOtaPhase::Idle | FleetOtaPhase::RolledBack)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetOtaPolicy {
    pub max_parallel: usize,
    pub canary_count: usize,
    pub min_healthy_percent: u8,
    pub max_failures: u8,
}

impl FleetOtaPolicy {
    pub const DEFAULT: Self = Self {
        max_parallel: 2,
        canary_count: 1,
        min_healthy_percent: 80,
        max_failures: 2,
    };
}

impl Default for FleetOtaPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FleetOtaError {
    Full,
    DuplicateNode(u16),
    MissingNode(u16),
    NoCapacity,
    FleetHealthTooLow { healthy_percent: u8, required: u8 },
    NoEligibleNodes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FleetOtaWave<const N: usize> {
    pub target_version: u32,
    pub selected: [Option<u16>; N],
    pub len: usize,
    pub phase: FleetOtaPhase,
}

impl<const N: usize> FleetOtaWave<N> {
    pub const fn new(target_version: u32, phase: FleetOtaPhase) -> Self {
        Self {
            target_version,
            selected: [None; N],
            len: 0,
            phase,
        }
    }

    fn push(&mut self, id: u16) -> bool {
        if self.len >= N {
            return false;
        }
        self.selected[self.len] = Some(id);
        self.len += 1;
        true
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

pub struct FleetOtaOrchestrator<const N: usize> {
    nodes: [Option<FleetOtaNode>; N],
}

impl<const N: usize> Default for FleetOtaOrchestrator<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FleetOtaOrchestrator<N> {
    pub const fn new() -> Self {
        Self { nodes: [None; N] }
    }

    pub fn register(&mut self, node: FleetOtaNode) -> Result<(), FleetOtaError> {
        if self.find_index(node.id).is_some() {
            return Err(FleetOtaError::DuplicateNode(node.id));
        }
        let Some(slot) = self.nodes.iter_mut().find(|slot| slot.is_none()) else {
            return Err(FleetOtaError::Full);
        };
        *slot = Some(node);
        Ok(())
    }

    pub fn set_health(&mut self, id: u16, healthy: bool) -> Result<(), FleetOtaError> {
        let Some(index) = self.find_index(id) else {
            return Err(FleetOtaError::MissingNode(id));
        };
        if let Some(node) = self.nodes[index].as_mut() {
            node.healthy = healthy;
        }
        Ok(())
    }

    pub fn mark_installing(&mut self, id: u16) -> Result<(), FleetOtaError> {
        let Some(index) = self.find_index(id) else {
            return Err(FleetOtaError::MissingNode(id));
        };
        if let Some(node) = self.nodes[index].as_mut() {
            if matches!(node.phase, FleetOtaPhase::Canary | FleetOtaPhase::Staged) {
                node.phase = FleetOtaPhase::Installing;
            }
        }
        Ok(())
    }

    pub fn complete_node(&mut self, id: u16, success: bool) -> Result<(), FleetOtaError> {
        self.complete_node_with_policy(id, success, FleetOtaPolicy::DEFAULT)
    }

    pub fn complete_node_with_policy(
        &mut self,
        id: u16,
        success: bool,
        policy: FleetOtaPolicy,
    ) -> Result<(), FleetOtaError> {
        let Some(index) = self.find_index(id) else {
            return Err(FleetOtaError::MissingNode(id));
        };
        if let Some(node) = self.nodes[index].as_mut() {
            if success {
                node.current_version = node.target_version;
                node.target_version = 0;
                node.phase = FleetOtaPhase::Confirmed;
                node.failures = 0;
            } else {
                node.failures = node.failures.saturating_add(1);
                node.target_version = 0;
                node.phase = if node.failures >= policy.max_failures {
                    FleetOtaPhase::Blocked
                } else {
                    FleetOtaPhase::RolledBack
                };
            }
        }
        Ok(())
    }

    pub fn stage_next_wave(
        &mut self,
        target_version: u32,
        policy: FleetOtaPolicy,
    ) -> Result<FleetOtaWave<N>, FleetOtaError> {
        let healthy_percent = self.healthy_percent();
        if healthy_percent < policy.min_healthy_percent {
            return Err(FleetOtaError::FleetHealthTooLow {
                healthy_percent,
                required: policy.min_healthy_percent,
            });
        }

        let active = self.active_count();
        if active >= policy.max_parallel {
            return Err(FleetOtaError::NoCapacity);
        }

        let confirmed = self.confirmed_count(target_version);
        if confirmed == 0 && active > 0 {
            return Err(FleetOtaError::NoCapacity);
        }
        let phase = if confirmed == 0 {
            FleetOtaPhase::Canary
        } else {
            FleetOtaPhase::Staged
        };
        let desired = if phase == FleetOtaPhase::Canary {
            policy.canary_count.max(1)
        } else {
            policy.max_parallel
        };
        let limit = desired.min(policy.max_parallel - active);
        let mut wave = FleetOtaWave::new(target_version, phase);

        for slot in &mut self.nodes {
            if wave.len >= limit {
                break;
            }
            let Some(node) = slot.as_mut() else {
                continue;
            };
            if !node.is_eligible(target_version, policy.max_failures) {
                continue;
            }
            node.phase = phase;
            node.target_version = target_version;
            if !wave.push(node.id) {
                return Err(FleetOtaError::Full);
            }
        }

        if wave.is_empty() {
            return Err(FleetOtaError::NoEligibleNodes);
        }
        Ok(wave)
    }

    pub fn node(&self, id: u16) -> Option<FleetOtaNode> {
        self.find_index(id).and_then(|index| self.nodes[index])
    }

    pub fn len(&self) -> usize {
        self.nodes.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn healthy_percent(&self) -> u8 {
        let total = self.len();
        if total == 0 {
            return 0;
        }
        let healthy = self
            .nodes
            .iter()
            .flatten()
            .filter(|node| node.healthy)
            .count();
        ((healthy * 100) / total) as u8
    }

    pub fn active_count(&self) -> usize {
        self.nodes
            .iter()
            .flatten()
            .filter(|node| node.is_active())
            .count()
    }

    pub fn confirmed_count(&self, target_version: u32) -> usize {
        self.nodes
            .iter()
            .flatten()
            .filter(|node| {
                node.current_version == target_version && node.phase == FleetOtaPhase::Confirmed
            })
            .count()
    }

    fn find_index(&self, id: u16) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| matches!(node, Some(node) if node.id == id))
    }
}

/// Store-and-forward buffer for a sleepy child (M132): hold messages destined for a node
/// that is asleep, deliver them when it polls. Bounded ring; oldest dropped when full.
pub struct StoreForward<T: Copy, const N: usize> {
    dst: [u16; N],
    msg: [Option<T>; N],
    head: usize,
    len: usize,
}

impl<T: Copy, const N: usize> Default for StoreForward<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> StoreForward<T, N> {
    pub const fn new() -> Self {
        Self {
            dst: [0; N],
            msg: [None; N],
            head: 0,
            len: 0,
        }
    }
    /// Buffer `item` for `dst` (dropping the oldest if full).
    pub fn store(&mut self, dst: u16, item: T) {
        let slot = (self.head + self.len) % N;
        if self.len == N {
            self.head = (self.head + 1) % N; // overwrite oldest
        } else {
            self.len += 1;
        }
        self.dst[slot] = dst;
        self.msg[slot] = Some(item);
    }
    /// Pop the next buffered message for `dst` (FIFO), or None.
    pub fn deliver(&mut self, dst: u16) -> Option<T> {
        for k in 0..self.len {
            let i = (self.head + k) % N;
            if self.msg[i].is_some() && self.dst[i] == dst {
                let m = self.msg[i].take();
                // compact is unnecessary; the slot is just marked empty
                return m;
            }
        }
        None
    }
    pub fn pending_for(&self, dst: u16) -> usize {
        (0..self.len)
            .filter(|&k| {
                let i = (self.head + k) % N;
                self.msg[i].is_some() && self.dst[i] == dst
            })
            .count()
    }
}

/// Network formation (M135): a coordinator assigns the next short address and builds a
/// parent map from join requests, forming a self-healing tree. Fixed capacity.
pub struct NetworkFormation<const N: usize> {
    next_addr: u16,
    parent: [(u16, u16); N], // (child_addr, parent_addr)
    len: usize,
}

impl<const N: usize> Default for NetworkFormation<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> NetworkFormation<N> {
    pub const fn new() -> Self {
        Self {
            next_addr: 1,
            parent: [(0, 0); N],
            len: 0,
        }
    }
    /// A node joins via `parent_addr`; returns its assigned short address.
    pub fn join(&mut self, parent_addr: u16) -> Option<u16> {
        if self.len >= N {
            return None;
        }
        let addr = self.next_addr;
        self.next_addr += 1;
        self.parent[self.len] = (addr, parent_addr);
        self.len += 1;
        Some(addr)
    }
    pub fn parent_of(&self, addr: u16) -> Option<u16> {
        self.parent[..self.len]
            .iter()
            .find(|(c, _)| *c == addr)
            .map(|(_, p)| *p)
    }
    /// Self-heal: a node re-parents to a new parent when its link drops.
    pub fn reparent(&mut self, addr: u16, new_parent: u16) -> bool {
        if let Some(e) = self.parent[..self.len].iter_mut().find(|(c, _)| *c == addr) {
            e.1 = new_parent;
            true
        } else {
            false
        }
    }
    /// Hops to the coordinator (addr 0) by walking parents; None if a cycle/orphan.
    pub fn depth(&self, addr: u16) -> Option<u16> {
        let mut cur = addr;
        for d in 0..=N as u16 {
            if cur == 0 {
                return Some(d);
            }
            cur = self.parent_of(cur)?;
        }
        None
    }
}

#[cfg(test)]
mod wireless_tests {
    use super::*;

    #[test]
    fn rssi_picks_best_link_then_cheapest() {
        // (id, quality, cost): id 3 has the best quality
        let c = [(2u16, 100u8, 1u8), (3, 200, 3), (4, 200, 2)];
        // best quality 200 shared by 3 and 4; lower cost (4, cost 2) wins the tie
        assert_eq!(rssi_best_next_hop(&c), Some(4));
        assert_eq!(rssi_best_next_hop(&[]), None);
    }

    #[test]
    fn ota_reassembles_and_reports_missing() {
        assert_eq!(OtaReassembler::<64>::chunk_count(1000, 256), 4);
        let mut r = OtaReassembler::<8>::new(4);
        assert!(r.receive(0) && r.receive(2) && r.receive(3));
        assert!(!r.receive(0)); // dup
        assert_eq!(r.first_missing(), Some(1));
        assert_eq!(r.progress_percent(), 75);
        assert!(r.receive(1));
        assert!(r.is_complete());
        assert_eq!(r.first_missing(), None);
    }

    #[test]
    fn fleet_ota_stages_canary_then_rollout_waves() {
        let mut ota = FleetOtaOrchestrator::<4>::new();
        for id in 1..=4 {
            ota.register(FleetOtaNode::new(id, 1)).unwrap();
        }

        let canary = ota.stage_next_wave(2, FleetOtaPolicy::DEFAULT).unwrap();
        assert_eq!(canary.phase, FleetOtaPhase::Canary);
        assert_eq!(canary.selected[0], Some(1));
        assert_eq!(canary.len, 1);
        assert_eq!(
            ota.node(1).map(|node| node.phase),
            Some(FleetOtaPhase::Canary)
        );

        assert_eq!(
            ota.stage_next_wave(2, FleetOtaPolicy::DEFAULT),
            Err(FleetOtaError::NoCapacity)
        );
        ota.mark_installing(1).unwrap();
        ota.complete_node(1, true).unwrap();
        assert_eq!(ota.confirmed_count(2), 1);

        let rollout = ota.stage_next_wave(2, FleetOtaPolicy::DEFAULT).unwrap();
        assert_eq!(rollout.phase, FleetOtaPhase::Staged);
        assert_eq!(rollout.len, 2);
        assert_eq!(rollout.selected[0], Some(2));
        assert_eq!(rollout.selected[1], Some(3));
        assert_eq!(ota.active_count(), 2);
    }

    #[test]
    fn fleet_ota_blocks_when_fleet_health_is_low() {
        let mut ota = FleetOtaOrchestrator::<4>::new();
        for id in 1..=4 {
            ota.register(FleetOtaNode::new(id, 1)).unwrap();
        }
        ota.set_health(3, false).unwrap();
        ota.set_health(4, false).unwrap();

        assert_eq!(
            ota.stage_next_wave(2, FleetOtaPolicy::DEFAULT),
            Err(FleetOtaError::FleetHealthTooLow {
                healthy_percent: 50,
                required: 80,
            })
        );
    }

    #[test]
    fn fleet_ota_rolls_back_and_blocks_after_repeated_failures() {
        let mut ota = FleetOtaOrchestrator::<2>::new();
        ota.register(FleetOtaNode::new(7, 1)).unwrap();
        let policy = FleetOtaPolicy {
            max_parallel: 1,
            canary_count: 1,
            min_healthy_percent: 1,
            max_failures: 2,
        };

        ota.stage_next_wave(2, policy).unwrap();
        ota.complete_node_with_policy(7, false, policy).unwrap();
        assert_eq!(
            ota.node(7).map(|node| node.phase),
            Some(FleetOtaPhase::RolledBack)
        );

        ota.stage_next_wave(2, policy).unwrap();
        ota.complete_node_with_policy(7, false, policy).unwrap();
        assert_eq!(
            ota.node(7).map(|node| node.phase),
            Some(FleetOtaPhase::Blocked)
        );
        assert_eq!(
            ota.stage_next_wave(2, policy),
            Err(FleetOtaError::NoEligibleNodes)
        );
    }

    #[test]
    fn store_forward_holds_and_delivers_per_dst() {
        let mut sf = StoreForward::<u16, 4>::new();
        sf.store(9, 0xAA);
        sf.store(7, 0xBB);
        sf.store(9, 0xCC);
        assert_eq!(sf.pending_for(9), 2);
        assert_eq!(sf.deliver(9), Some(0xAA)); // FIFO for that dst
        assert_eq!(sf.deliver(9), Some(0xCC));
        assert_eq!(sf.deliver(9), None);
        assert_eq!(sf.deliver(7), Some(0xBB));
    }

    #[test]
    fn network_forms_tree_and_self_heals() {
        let mut net = NetworkFormation::<8>::new();
        let a = net.join(0).unwrap(); // joins coordinator
        let b = net.join(a).unwrap(); // joins via a
        assert_eq!(net.parent_of(b), Some(a));
        assert_eq!(net.depth(b), Some(2)); // b -> a -> coord
                                           // a's link drops; b re-parents directly to the coordinator
        assert!(net.reparent(b, 0));
        assert_eq!(net.depth(b), Some(1));
    }
}
