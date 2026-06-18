//! Fixed-size event log for post-fault inspection without allocation.

use crate::{Action, KernelError, ModuleId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventSeverity {
    Trace = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Fatal = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Boot,
    Health,
    Recovery,
    TaskOverrun,
    Lease,
    SamplePool,
    Manifest,
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventPayload {
    None,
    Error(KernelError),
    Action(Action),
    Counter(u32),
    Pair(u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventRecord {
    pub seq: u32,
    pub at_us: u64,
    pub module: ModuleId,
    pub severity: EventSeverity,
    pub kind: EventKind,
    pub payload: EventPayload,
}

impl EventRecord {
    pub const fn new(
        at_us: u64,
        module: ModuleId,
        severity: EventSeverity,
        kind: EventKind,
        payload: EventPayload,
    ) -> Self {
        Self {
            seq: 0,
            at_us,
            module,
            severity,
            kind,
            payload,
        }
    }
}

pub struct EventLog<const N: usize> {
    records: [Option<EventRecord>; N],
    next: usize,
    len: usize,
    seq: u32,
    dropped: u32,
}

impl<const N: usize> EventLog<N> {
    pub const fn new() -> Self {
        Self {
            records: [None; N],
            next: 0,
            len: 0,
            seq: 0,
            dropped: 0,
        }
    }

    pub fn push(&mut self, mut record: EventRecord) -> Option<EventRecord> {
        if N == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return Some(record);
        }

        self.seq = self.seq.wrapping_add(1);
        record.seq = self.seq;

        let overwritten = self.records[self.next].replace(record);
        self.next = (self.next + 1) % N;
        if self.len < N {
            self.len += 1;
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
        overwritten
    }

    pub fn push_health(
        &mut self,
        at_us: u64,
        module: ModuleId,
        error: KernelError,
        action: Action,
    ) {
        let severity = match action {
            Action::RebootModule => EventSeverity::Fatal,
            Action::NotifyUserTask => EventSeverity::Error,
            Action::RetryDelay(_) | Action::RetryNow => EventSeverity::Warn,
            Action::Ignore => EventSeverity::Info,
        };
        self.push(EventRecord::new(
            at_us,
            module,
            severity,
            EventKind::Health,
            EventPayload::Error(error),
        ));
        self.push(EventRecord::new(
            at_us,
            module,
            severity,
            EventKind::Recovery,
            EventPayload::Action(action),
        ));
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn latest(&self) -> Option<EventRecord> {
        if self.len == 0 || N == 0 {
            return None;
        }
        let idx = if self.next == 0 { N - 1 } else { self.next - 1 };
        self.records[idx]
    }

    pub fn copy_recent(&self, out: &mut [EventRecord]) -> usize {
        if N == 0 || self.len == 0 || out.is_empty() {
            return 0;
        }

        let count = out.len().min(self.len);
        let start_age = self.len - count;
        for (dst_idx, age) in (start_age..self.len).enumerate() {
            let idx = (self.next + N - self.len + age) % N;
            if let Some(record) = self.records[idx] {
                out[dst_idx] = record;
            }
        }
        count
    }

    pub fn count_at_or_above(&self, severity: EventSeverity) -> usize {
        self.records
            .iter()
            .flatten()
            .filter(|record| record.severity >= severity)
            .count()
    }
}

impl<const N: usize> Default for EventLog<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq_hint: u64, severity: EventSeverity) -> EventRecord {
        EventRecord::new(
            seq_hint,
            ModuleId::Kernel,
            severity,
            EventKind::Boot,
            EventPayload::Counter(seq_hint as u32),
        )
    }

    #[test]
    fn log_preserves_recent_order_after_wrap() {
        let mut log = EventLog::<3>::new();
        log.push(event(10, EventSeverity::Info));
        log.push(event(20, EventSeverity::Warn));
        log.push(event(30, EventSeverity::Error));
        log.push(event(40, EventSeverity::Fatal));

        let mut out = [event(0, EventSeverity::Trace); 3];
        let copied = log.copy_recent(&mut out);

        assert_eq!(copied, 3);
        assert_eq!(log.len(), 3);
        assert_eq!(log.dropped(), 1);
        assert_eq!(out[0].at_us, 20);
        assert_eq!(out[1].at_us, 30);
        assert_eq!(out[2].at_us, 40);
        assert_eq!(log.latest().expect("latest").at_us, 40);
    }

    #[test]
    fn log_counts_severity_threshold() {
        let mut log = EventLog::<4>::new();
        log.push(event(1, EventSeverity::Info));
        log.push(event(2, EventSeverity::Warn));
        log.push(event(3, EventSeverity::Error));
        log.push(event(4, EventSeverity::Fatal));

        assert_eq!(log.count_at_or_above(EventSeverity::Warn), 3);
        assert_eq!(log.count_at_or_above(EventSeverity::Fatal), 1);
    }

    #[test]
    fn health_helper_records_error_and_action() {
        let mut log = EventLog::<4>::new();
        log.push_health(
            100,
            ModuleId::Sensor,
            KernelError::SensorReadFail,
            Action::NotifyUserTask,
        );

        let mut out = [event(0, EventSeverity::Trace); 2];
        assert_eq!(log.copy_recent(&mut out), 2);
        assert_eq!(out[0].kind, EventKind::Health);
        assert_eq!(
            out[0].payload,
            EventPayload::Error(KernelError::SensorReadFail)
        );
        assert_eq!(out[1].kind, EventKind::Recovery);
        assert_eq!(out[1].payload, EventPayload::Action(Action::NotifyUserTask));
    }
}
