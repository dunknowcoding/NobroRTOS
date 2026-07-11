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

pub struct Mailbox<const N: usize> {
    slots: [Option<Message>; N],
    head: usize,
    len: usize,
    dropped: u32,
}

impl<const N: usize> Mailbox<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub fn push(&mut self, message: Message) -> Result<(), MailboxError> {
        if self.len == N {
            self.dropped = self.dropped.saturating_add(1);
            return Err(MailboxError::Full);
        }

        let idx = (self.head + self.len) % N;
        self.slots[idx] = Some(message);
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<Message> {
        if self.len == 0 {
            return None;
        }

        let message = self.slots[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        message
    }

    pub fn pop_for(&mut self, to: ModuleId) -> Option<Message> {
        for age in 0..self.len {
            let idx = (self.head + age) % N;
            if self.slots[idx].map(|msg| msg.to == to).unwrap_or(false) {
                let message = self.slots[idx].take();
                self.compact_from(age);
                return message;
            }
        }
        None
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

    fn compact_from(&mut self, age: usize) {
        for shift in age..(self.len - 1) {
            let dst = (self.head + shift) % N;
            let src = (self.head + shift + 1) % N;
            self.slots[dst] = self.slots[src].take();
        }
        let tail = (self.head + self.len - 1) % N;
        self.slots[tail] = None;
        self.len -= 1;
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
}
