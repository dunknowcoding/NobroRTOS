//! Minimal cooperative executor for Phase 1 (no heap, no async/await yet).

use crate::scheduler::Timer;

pub trait Task {
    fn poll(&mut self, now_us: u64) -> Poll;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    Ready,
}

/// Simple I2C poll task stub.
pub struct I2cPollTask {
    timer: Timer,
    owner: u8,
    pub reads: u32,
}

impl I2cPollTask {
    pub fn new(owner: u8, now_us: u64) -> Self {
        Self {
            timer: Timer::after_ms(100, now_us),
            owner,
            reads: 0,
        }
    }
}

impl Task for I2cPollTask {
    fn poll(&mut self, now_us: u64) -> Poll {
        if !self.timer.is_ready(now_us) {
            return Poll::Pending;
        }
        self.reads += 1;
        self.timer = Timer::after_ms(100, now_us);
        Poll::Ready
    }
}

impl I2cPollTask {
    pub fn owner(&self) -> u8 {
        self.owner
    }
}

/// Heartbeat / stats reporter.
pub struct StatsTask {
    timer: Timer,
}

impl StatsTask {
    pub fn new(now_us: u64) -> Self {
        Self {
            timer: Timer::after_ms(2000, now_us),
        }
    }
}

impl Task for StatsTask {
    fn poll(&mut self, now_us: u64) -> Poll {
        if self.timer.is_ready(now_us) {
            self.timer = Timer::after_ms(2000, now_us);
            Poll::Ready
        } else {
            Poll::Pending
        }
    }
}
