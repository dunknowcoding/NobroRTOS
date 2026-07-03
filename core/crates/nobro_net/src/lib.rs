//! No-heap networking primitives for multi-board NobroRTOS meshes.
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
        let cand = Route { dest, next_hop, cost, seq };
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
        Self { count: 0, sum: 0, min: i64::MAX, max: i64::MIN }
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
        t.integrate_from(2, &[Route { dest: 5, next_hop: 5, cost: 0, seq: 1 }]);
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
        Self { ids: [0; N], head: 0, len: 0 }
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
        Self { items: [None; N], len: 0 }
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
        Self { nodes: [None; N], timeout_us }
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
            *slot = Some(LinkState { id, last_us: now_us, up: true });
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
        self.nodes.iter().filter_map(|s| *s).any(|s| s.id == id && s.up)
    }

    pub fn up_count(&self) -> usize {
        self.nodes.iter().filter_map(|s| *s).filter(|s| s.up).count()
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
