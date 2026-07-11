//! Fixed-capacity module mailbox for small control messages.

use crate::ModuleId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Command,
    Notification,
    Recovery,
    SampleReady,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub from: ModuleId,
    pub to: ModuleId,
    pub kind: MessageKind,
    pub arg0: u32,
    pub arg1: u32,
}

impl Message {
    pub const fn new(
        from: ModuleId,
        to: ModuleId,
        kind: MessageKind,
        arg0: u32,
        arg1: u32,
    ) -> Self {
        Self {
            from,
            to,
            kind,
            arg0,
            arg1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxError {
    Full,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MailboxWork {
    pub inspected: usize,
    pub shifted: usize,
}

pub struct Mailbox<const N: usize> {
    slots: [Option<Message>; N],
    head: usize,
    len: usize,
    control_len: usize,
    control_reserve: usize,
    dropped: u32,
}

impl<const N: usize> Mailbox<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            head: 0,
            len: 0,
            control_len: 0,
            control_reserve: 0,
            dropped: 0,
        }
    }

    pub const fn with_control_reserve(reserved: usize) -> Self {
        let mut mailbox = Self::new();
        mailbox.control_reserve = if reserved > N { N } else { reserved };
        mailbox
    }

    const fn is_control(message: Message) -> bool {
        matches!(message.kind, MessageKind::Recovery | MessageKind::Shutdown)
    }

    pub fn push(&mut self, message: Message) -> Result<(), MailboxError> {
        let control = Self::is_control(message);
        if self.len == N || (!control && self.len >= N.saturating_sub(self.control_reserve)) {
            self.dropped = self.dropped.saturating_add(1);
            return Err(MailboxError::Full);
        }

        if control {
            for age in (self.control_len..self.len).rev() {
                let src = (self.head + age) % N;
                let dst = (self.head + age + 1) % N;
                self.slots[dst] = self.slots[src].take();
            }
            let idx = (self.head + self.control_len) % N;
            self.slots[idx] = Some(message);
            self.control_len += 1;
        } else {
            let idx = (self.head + self.len) % N;
            self.slots[idx] = Some(message);
        }
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<Message> {
        if self.len == 0 {
            return None;
        }

        let message = self.slots[self.head].take();
        if self.control_len > 0 {
            self.control_len -= 1;
        }
        self.head = (self.head + 1) % N;
        self.len -= 1;
        message
    }

    pub fn pop_for(&mut self, to: ModuleId) -> Option<Message> {
        self.pop_for_with_work(to).0
    }

    pub fn pop_for_with_work(&mut self, to: ModuleId) -> (Option<Message>, MailboxWork) {
        let mut work = MailboxWork::default();
        for age in 0..self.len {
            work.inspected += 1;
            let idx = (self.head + age) % N;
            if self.slots[idx].map(|msg| msg.to == to).unwrap_or(false) {
                let message = self.slots[idx].take();
                work.shifted = self.compact_from(age);
                return (message, work);
            }
        }
        (None, work)
    }

    pub fn remove_for(&mut self, module: ModuleId) -> usize {
        self.remove_for_with(module, |_| {})
    }

    /// Remove every message touching `module`, handing each removed message to
    /// `on_removed` so the caller can reconcile per-message accounting.
    pub fn remove_for_with(
        &mut self,
        module: ModuleId,
        mut on_removed: impl FnMut(Message),
    ) -> usize {
        let mut removed = 0;
        let mut age = 0;
        while age < self.len {
            let idx = (self.head + age) % N;
            if self.slots[idx]
                .map(|msg| msg.from == module || msg.to == module)
                .unwrap_or(false)
            {
                if let Some(message) = self.slots[idx].take() {
                    on_removed(message);
                }
                self.compact_from(age);
                removed += 1;
            } else {
                age += 1;
            }
        }
        removed
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

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub const fn control_reserve(&self) -> usize {
        self.control_reserve
    }

    pub const fn control_len(&self) -> usize {
        self.control_len
    }

    fn compact_from(&mut self, age: usize) -> usize {
        if age < self.control_len {
            self.control_len -= 1;
        }
        let shifted = self.len - age - 1;
        for shift in age..(self.len - 1) {
            let dst = (self.head + shift) % N;
            let src = (self.head + shift + 1) % N;
            self.slots[dst] = self.slots[src].take();
        }
        let tail = (self.head + self.len - 1) % N;
        self.slots[tail] = None;
        self.len -= 1;
        shifted
    }
}

impl<const N: usize> Default for Mailbox<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(from: ModuleId, to: ModuleId, arg0: u32) -> Message {
        Message::new(from, to, MessageKind::Command, arg0, 0)
    }

    #[test]
    fn mailbox_preserves_fifo_order() {
        let mut mailbox = Mailbox::<3>::new();
        mailbox
            .push(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
            .unwrap();
        mailbox
            .push(msg(ModuleId::Sensor, ModuleId::Kernel, 2))
            .unwrap();

        assert_eq!(
            mailbox.pop(),
            Some(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
        );
        assert_eq!(
            mailbox.pop(),
            Some(msg(ModuleId::Sensor, ModuleId::Kernel, 2))
        );
        assert!(mailbox.is_empty());
    }

    #[test]
    fn mailbox_can_pop_for_one_module_without_losing_order() {
        let mut mailbox = Mailbox::<4>::new();
        mailbox
            .push(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
            .unwrap();
        mailbox
            .push(msg(ModuleId::Kernel, ModuleId::Radio, 2))
            .unwrap();
        mailbox
            .push(msg(ModuleId::Sensor, ModuleId::Kernel, 3))
            .unwrap();

        assert_eq!(
            mailbox.pop_for(ModuleId::Radio),
            Some(msg(ModuleId::Kernel, ModuleId::Radio, 2))
        );
        assert_eq!(
            mailbox.pop(),
            Some(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
        );
        assert_eq!(
            mailbox.pop(),
            Some(msg(ModuleId::Sensor, ModuleId::Kernel, 3))
        );
    }

    #[test]
    fn mailbox_reports_full_without_overwriting_messages() {
        let mut mailbox = Mailbox::<1>::new();
        mailbox
            .push(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
            .unwrap();

        assert_eq!(
            mailbox.push(msg(ModuleId::Kernel, ModuleId::Radio, 2)),
            Err(MailboxError::Full)
        );
        assert_eq!(mailbox.dropped(), 1);
        assert_eq!(
            mailbox.pop(),
            Some(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
        );
    }

    #[test]
    fn mailbox_removes_messages_for_disabled_module() {
        let mut mailbox = Mailbox::<4>::new();
        mailbox
            .push(msg(ModuleId::Kernel, ModuleId::Sensor, 1))
            .unwrap();
        mailbox
            .push(msg(ModuleId::Sensor, ModuleId::Kernel, 2))
            .unwrap();
        mailbox
            .push(msg(ModuleId::Kernel, ModuleId::Radio, 3))
            .unwrap();

        assert_eq!(mailbox.remove_for(ModuleId::Sensor), 2);
        assert_eq!(
            mailbox.pop(),
            Some(msg(ModuleId::Kernel, ModuleId::Radio, 3))
        );
        assert!(mailbox.is_empty());
    }

    #[test]
    fn reserved_control_capacity_and_priority_survive_normal_saturation() {
        let mut mailbox = Mailbox::<4>::with_control_reserve(1);
        for arg in 0..3 {
            mailbox
                .push(msg(ModuleId::Sensor, ModuleId::Radio, arg))
                .unwrap();
        }
        assert_eq!(
            mailbox.push(msg(ModuleId::Sensor, ModuleId::Radio, 99)),
            Err(MailboxError::Full)
        );
        let recovery = Message::new(
            ModuleId::Kernel,
            ModuleId::Sensor,
            MessageKind::Recovery,
            7,
            0,
        );
        mailbox.push(recovery).unwrap();
        assert_eq!(mailbox.control_len(), 1);
        assert_eq!(mailbox.pop(), Some(recovery));
        assert_eq!(mailbox.pop().map(|message| message.arg0), Some(0));
    }

    #[test]
    fn full_selective_paths_report_exact_bounded_work() {
        let mut tail = Mailbox::<8>::new();
        for arg in 0..7 {
            tail.push(msg(ModuleId::Sensor, ModuleId::Radio, arg))
                .unwrap();
        }
        tail.push(msg(ModuleId::Sensor, ModuleId::Kernel, 7))
            .unwrap();
        let (_, scan_heavy) = tail.pop_for_with_work(ModuleId::Kernel);
        assert_eq!(
            scan_heavy,
            MailboxWork {
                inspected: 8,
                shifted: 0
            }
        );

        let mut shift = Mailbox::<8>::new();
        shift
            .push(msg(ModuleId::Sensor, ModuleId::Kernel, 0))
            .unwrap();
        for arg in 1..8 {
            shift
                .push(msg(ModuleId::Sensor, ModuleId::Radio, arg))
                .unwrap();
        }
        let (_, shift_heavy) = shift.pop_for_with_work(ModuleId::Kernel);
        assert_eq!(
            shift_heavy,
            MailboxWork {
                inspected: 1,
                shifted: 7
            }
        );
    }
}
