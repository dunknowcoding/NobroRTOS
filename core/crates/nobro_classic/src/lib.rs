//! FreeRTOS-familiar RTOS primitives, re-imagined without FreeRTOS's costs.
//!
//! | FreeRTOS | nobro-classic | what changed |
//! |---|---|---|
//! | `xQueueCreate` + heap | [`Queue<T, N>`] (const N) | fixed capacity, **no heap**, no fragmentation |
//! | `xQueueSend/Receive` | [`Queue::send`]/[`Queue::receive`] | same FIFO semantics, bounded |
//! | `xSemaphoreCreate*` | [`Semaphore`] | binary + counting, no alloc |
//! | `xSemaphoreCreateMutex` | [`Mutex`] | ownership-checked; peripherals use kernel leases (no priority inversion) |
//! | `xTimerCreate` | [`SoftwareTimer`] | tick-driven, one-shot / auto-reload |
//! | `xEventGroupCreate/Set/Clear/WaitBits` | [`EventFlags`] | 32 flags, poll-style wait_any/wait_all |
//! | block on N objects (queue sets) | [`select2`] | bounded multi-event wait, idle hook between polls |
//!
//! Everything here is `no_std`, `#![forbid(unsafe_code)]`, and sized at compile time - a
//! FreeRTOS user migrates the API surface while gaining bounded RAM and safety.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// Fixed-capacity FIFO queue (FreeRTOS `xQueue`) - no heap, no fragmentation.
pub struct Queue<T: Copy, const N: usize> {
    buf: [Option<T>; N],
    head: usize,
    tail: usize,
    count: usize,
}

impl<T: Copy, const N: usize> Default for Queue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> Queue<T, N> {
    pub const fn new() -> Self {
        Self {
            buf: [None; N],
            head: 0,
            tail: 0,
            count: 0,
        }
    }
    /// `xQueueSend` (to back). Returns false if full (pdFALSE).
    pub fn send(&mut self, item: T) -> bool {
        if N == 0 || self.count == N {
            return false;
        }
        self.buf[self.tail] = Some(item);
        self.tail = (self.tail + 1) % N;
        self.count += 1;
        true
    }
    /// `xQueueSendToFront`.
    pub fn send_to_front(&mut self, item: T) -> bool {
        if N == 0 || self.count == N {
            return false;
        }
        self.head = (self.head + N - 1) % N;
        self.buf[self.head] = Some(item);
        self.count += 1;
        true
    }
    /// `xQueueReceive`. Returns None if empty.
    pub fn receive(&mut self) -> Option<T> {
        if self.count == 0 {
            return None;
        }
        let item = self.buf[self.head].take();
        self.head = (self.head + 1) % N;
        self.count -= 1;
        item
    }
    /// `xQueuePeek`.
    pub fn peek(&self) -> Option<T> {
        self.buf[self.head]
    }
    pub fn messages_waiting(&self) -> usize {
        self.count
    }
    pub fn spaces_available(&self) -> usize {
        N - self.count
    }
    pub fn is_full(&self) -> bool {
        self.count == N
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Counting/binary semaphore (FreeRTOS `xSemaphore`).
#[derive(Clone, Copy, Debug)]
pub struct Semaphore {
    count: u32,
    max: u32,
}

impl Semaphore {
    /// `xSemaphoreCreateBinary` (+ optionally pre-given).
    pub const fn binary(available: bool) -> Self {
        Self {
            count: available as u32,
            max: 1,
        }
    }
    /// `xSemaphoreCreateCounting(max, initial)`.
    pub const fn counting(max: u32, initial: u32) -> Self {
        Self {
            count: if initial > max { max } else { initial },
            max,
        }
    }
    /// `xSemaphoreGive`. Returns false if already at max.
    pub fn give(&mut self) -> bool {
        if self.count >= self.max {
            return false;
        }
        self.count += 1;
        true
    }
    /// `xSemaphoreTake` (non-blocking). Returns false if unavailable.
    pub fn take(&mut self) -> bool {
        if self.count == 0 {
            return false;
        }
        self.count -= 1;
        true
    }
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// Non-recursive ownership token (FreeRTOS-style `xMutex`). This is deliberately
/// non-blocking and contains no protected data; it is not a Rust data mutex. Peripheral
/// access should use the kernel's
/// resource leases instead - those are priority-inversion-free because the kernel
/// arbitrates ownership. This is for app-level mutual exclusion.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mutex {
    owner: Option<u8>,
}

impl Mutex {
    pub const fn new() -> Self {
        Self { owner: None }
    }
    /// `xSemaphoreTake` on the token: succeeds only if free. Recursive acquisition is
    /// rejected so one release can never accidentally unlock a nested acquisition.
    pub fn take(&mut self, owner: u8) -> bool {
        match self.owner {
            None => {
                self.owner = Some(owner);
                true
            }
            Some(_) => false,
        }
    }
    /// `xSemaphoreGive`: only the holder may release.
    pub fn give(&mut self, owner: u8) -> bool {
        if self.owner == Some(owner) {
            self.owner = None;
            true
        } else {
            false
        }
    }
    pub fn holder(&self) -> Option<u8> {
        self.owner
    }
}

/// Software timer (FreeRTOS `xTimer`): tick-driven, one-shot or auto-reload.
#[derive(Clone, Copy, Debug)]
pub struct SoftwareTimer {
    period: u64,
    remaining: u64,
    auto_reload: bool,
    active: bool,
}

impl SoftwareTimer {
    /// `xTimerCreate(period, auto_reload)`.
    pub const fn new(period: u64, auto_reload: bool) -> Self {
        Self {
            period,
            remaining: period,
            auto_reload,
            active: false,
        }
    }
    /// `xTimerStart`.
    pub fn start(&mut self) {
        self.remaining = self.period;
        self.active = true;
    }
    /// `xTimerStop`.
    pub fn stop(&mut self) {
        self.active = false;
    }
    /// Advance by `dt`; returns true on each expiry. Auto-reload timers re-arm, one-shots
    /// stop after firing.
    pub fn tick(&mut self, dt: u64) -> bool {
        if !self.active {
            return false;
        }
        if dt >= self.remaining {
            if self.auto_reload {
                let over = dt - self.remaining;
                self.remaining = self.period.saturating_sub(over % self.period.max(1));
            } else {
                self.active = false;
                self.remaining = 0;
            }
            true
        } else {
            self.remaining -= dt;
            false
        }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_fifo_bounded_and_send_to_front_is_lifo() {
        let mut q: Queue<u16, 3> = Queue::new();
        assert!(q.send(1) && q.send(2) && q.send(3));
        assert!(!q.send(4)); // full -> pdFALSE
        assert_eq!(q.messages_waiting(), 3);
        assert_eq!(q.receive(), Some(1)); // FIFO
        assert!(q.send_to_front(9)); // jumps the line
        assert_eq!(q.receive(), Some(9));
        assert_eq!(q.receive(), Some(2));
        assert_eq!(q.receive(), Some(3));
        assert_eq!(q.receive(), None);
    }

    #[test]
    fn semaphore_binary_and_counting() {
        let mut b = Semaphore::binary(false);
        assert!(!b.take());
        assert!(b.give());
        assert!(!b.give()); // already at max 1
        assert!(b.take());
        let mut c = Semaphore::counting(3, 2);
        assert!(c.take() && c.take());
        assert!(!c.take()); // drained
        assert_eq!(c.count(), 0);
    }

    #[test]
    fn mutex_ownership_is_enforced() {
        let mut m = Mutex::new();
        assert!(m.take(1));
        assert!(!m.take(2)); // held by 1
        assert!(!m.take(1)); // explicitly non-recursive
        assert!(!m.give(2)); // only holder releases
        assert!(m.give(1));
        assert!(m.take(2)); // now free
    }

    #[test]
    fn zero_capacity_queue_fails_without_panicking() {
        let mut q = Queue::<u8, 0>::new();
        assert!(!q.send(1));
        assert!(!q.send_to_front(1));
        assert_eq!(q.receive(), None);
    }

    #[test]
    fn software_timer_oneshot_and_autoreload() {
        let mut one = SoftwareTimer::new(100, false);
        one.start();
        assert!(!one.tick(60));
        assert!(one.tick(60)); // crosses 100 -> fires once
        assert!(!one.tick(100)); // stopped
        assert!(!one.is_active());

        let mut auto = SoftwareTimer::new(50, true);
        auto.start();
        let mut fires = 0;
        for _ in 0..10 {
            if auto.tick(30) {
                fires += 1;
            }
        }
        // 10 ticks * 30 = 300; period 50 -> ~6 fires
        assert!((5..=6).contains(&fires), "auto-reload fired {fires} times");
        assert!(auto.is_active());
    }
}

/// Event flag group (FreeRTOS `xEventGroup*`): up to 32 flags in one word, no heap,
/// non-blocking like everything else in this crate. `wait_*` are polls - combine with
/// [`select2`] for bounded multi-event waiting instead of busy loops.
#[derive(Clone, Copy, Debug, Default)]
pub struct EventFlags {
    bits: u32,
}

impl EventFlags {
    /// `xEventGroupCreate` (statically - no allocation).
    pub const fn new() -> Self {
        Self { bits: 0 }
    }
    /// `xEventGroupSetBits`. Returns the flag word after setting.
    pub fn set(&mut self, mask: u32) -> u32 {
        self.bits |= mask;
        self.bits
    }
    /// `xEventGroupClearBits`. Returns the flag word after clearing.
    pub fn clear(&mut self, mask: u32) -> u32 {
        self.bits &= !mask;
        self.bits
    }
    /// `xEventGroupGetBits`.
    pub fn get(&self) -> u32 {
        self.bits
    }
    /// `xEventGroupWaitBits(..., xWaitForAllBits = pdFALSE)` as a poll: if ANY flag in
    /// `mask` is set, returns the matching flags (clearing them when `clear_on_exit`).
    pub fn wait_any(&mut self, mask: u32, clear_on_exit: bool) -> Option<u32> {
        let hit = self.bits & mask;
        if hit == 0 {
            return None;
        }
        if clear_on_exit {
            self.bits &= !hit;
        }
        Some(hit)
    }
    /// `xEventGroupWaitBits(..., xWaitForAllBits = pdTRUE)` as a poll: only when ALL
    /// flags in `mask` are set.
    pub fn wait_all(&mut self, mask: u32, clear_on_exit: bool) -> Option<u32> {
        if self.bits & mask != mask {
            return None;
        }
        if clear_on_exit {
            self.bits &= !mask;
        }
        Some(mask)
    }
}

/// Which of two event sources became ready first (see [`select2`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    First,
    Second,
}

/// Bounded multi-event wait: poll two readiness closures alternately for at most
/// `max_polls` rounds, calling `idle` between rounds (insert `wfe`/a scheduler yield
/// there on a real target; a no-op in tests). This is the answer to "wait on a queue
/// AND a timer" without busy-waiting forever: the wait is bounded by construction,
/// like every other primitive here. Returns which source fired, or `None` on timeout.
pub fn select2(
    mut first: impl FnMut() -> bool,
    mut second: impl FnMut() -> bool,
    max_polls: u32,
    mut idle: impl FnMut(),
) -> Option<Ready> {
    for _ in 0..max_polls {
        if first() {
            return Some(Ready::First);
        }
        if second() {
            return Some(Ready::Second);
        }
        idle();
    }
    None
}

#[cfg(test)]
mod event_flags_tests {
    use super::*;

    #[test]
    fn set_wait_any_and_all() {
        let mut ev = EventFlags::new();
        assert_eq!(ev.wait_any(0b111, true), None);
        ev.set(0b001);
        ev.set(0b100);
        assert_eq!(ev.get(), 0b101);
        // any: returns only the matching, clears only the matching
        assert_eq!(ev.wait_any(0b011, true), Some(0b001));
        assert_eq!(ev.get(), 0b100);
        // all: not satisfied until every bit present
        assert_eq!(ev.wait_all(0b110, true), None);
        ev.set(0b010);
        assert_eq!(ev.wait_all(0b110, true), Some(0b110));
        assert_eq!(ev.get(), 0);
    }

    #[test]
    fn clear_on_exit_false_preserves_flags() {
        let mut ev = EventFlags::new();
        ev.set(0b1010);
        assert_eq!(ev.wait_any(0b1000, false), Some(0b1000));
        assert_eq!(ev.get(), 0b1010); // untouched
        assert_eq!(ev.clear(0b0010), 0b1000);
    }

    #[test]
    fn select2_returns_first_ready_source_and_bounds_the_wait() {
        // queue empties on round 3; timer never fires
        let mut round = 0;
        let got = select2(
            || {
                round += 1;
                round >= 3
            },
            || false,
            10,
            || {},
        );
        assert_eq!(got, Some(Ready::First));

        // neither fires -> bounded timeout, idle called each round
        let mut idles = 0;
        let got = select2(|| false, || false, 5, || idles += 1);
        assert_eq!(got, None);
        assert_eq!(idles, 5);

        // second wins when first is quiet
        let got = select2(|| false, || true, 5, || {});
        assert_eq!(got, Some(Ready::Second));
    }

    #[test]
    fn select2_composes_flags_and_queue() {
        let mut ev = EventFlags::new();
        let mut q: Queue<u8, 4> = Queue::new();
        q.send(9);
        // both ready -> first (flags) has priority order semantics
        ev.set(0b1);
        let got = select2(
            || ev.wait_any(0b1, true).is_some(),
            || q.receive().is_some(),
            3,
            || {},
        );
        assert_eq!(got, Some(Ready::First));
    }
}
