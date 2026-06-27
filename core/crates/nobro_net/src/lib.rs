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
