//! FreeRTOS-familiar RTOS primitives, re-imagined without FreeRTOS's costs.
//!
//! | FreeRTOS | nobro-classic | what changed |
//! |---|---|---|
//! | `xQueueCreate` + heap | [`Queue<T, N>`] (const N) | fixed capacity, **no heap**, no fragmentation |
//! | `xQueueSend/Receive` | [`Queue::send`]/[`Queue::receive`] | same FIFO semantics, bounded |
//! | `xSemaphoreCreate*` | [`Semaphore`] | binary + counting, no alloc |
//! | `xSemaphoreCreateMutex` | [`Mutex`] | ownership-checked; peripherals use kernel leases (no priority inversion) |
//! | `xTimerCreate` | [`SoftwareTimer`] | tick-driven, one-shot / auto-reload |
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
        Self { buf: [None; N], head: 0, tail: 0, count: 0 }
    }
    /// `xQueueSend` (to back). Returns false if full (pdFALSE).
    pub fn send(&mut self, item: T) -> bool {
        if self.count == N {
            return false;
        }
        self.buf[self.tail] = Some(item);
        self.tail = (self.tail + 1) % N;
        self.count += 1;
        true
    }
    /// `xQueueSendToFront`.
    pub fn send_to_front(&mut self, item: T) -> bool {
        if self.count == N {
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
        Self { count: available as u32, max: 1 }
    }
    /// `xSemaphoreCreateCounting(max, initial)`.
    pub const fn counting(max: u32, initial: u32) -> Self {
        Self { count: if initial > max { max } else { initial }, max }
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

/// Ownership mutex (FreeRTOS `xMutex`). Peripheral access should use the kernel's
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
    /// `xSemaphoreTake` on a mutex: succeeds if free (or re-taken by the same owner).
    pub fn take(&mut self, owner: u8) -> bool {
        match self.owner {
            None => {
                self.owner = Some(owner);
                true
            }
            Some(o) => o == owner,
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
        Self { period, remaining: period, auto_reload, active: false }
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
        assert!(m.take(1)); // re-entrant by holder
        assert!(!m.give(2)); // only holder releases
        assert!(m.give(1));
        assert!(m.take(2)); // now free
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
