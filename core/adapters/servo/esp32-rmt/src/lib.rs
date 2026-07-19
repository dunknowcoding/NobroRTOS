#![cfg_attr(not(test), no_std)]

use nobro_servo::{PulseEngineBackend, PulseError, PulseResourcePrice, PulseState, PulseSymbol};

pub const BACKEND_ID: &str = "esp32-arduino-rmt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Failed,
    Deadline,
}

pub trait Esp32RmtTransport {
    fn init(&mut self, tick_hz: u32) -> bool;
    fn write(&mut self, symbols: &[PulseSymbol], max_block_us: u32) -> Result<(), TransportError>;
    fn deinit(&mut self) -> bool;
}

pub struct Esp32Rmt<T> {
    transport: T,
    state: PulseState,
    tick_hz: u32,
    max_symbols: usize,
    price: PulseResourcePrice,
    writes: u32,
    symbols: u32,
    oversized: u32,
    transport_errors: u32,
    deadline_misses: u32,
    recoveries: u32,
}

impl<T: Esp32RmtTransport> Esp32Rmt<T> {
    pub const fn new(transport: T, max_symbols: usize, price: PulseResourcePrice) -> Self {
        Self {
            transport,
            state: PulseState::Down,
            tick_hz: 0,
            max_symbols,
            price,
            writes: 0,
            symbols: 0,
            oversized: 0,
            transport_errors: 0,
            deadline_misses: 0,
            recoveries: 0,
        }
    }

    pub const fn admission_price(&self) -> PulseResourcePrice {
        self.price
    }

    pub const fn diagnostics(&self) -> (u32, u32, u32, u32, u32, u32) {
        (
            self.writes,
            self.symbols,
            self.oversized,
            self.transport_errors,
            self.deadline_misses,
            self.recoveries,
        )
    }

    fn transport_failure(&mut self, error: TransportError) -> PulseError {
        self.state = PulseState::Faulted;
        match error {
            TransportError::Deadline => {
                self.deadline_misses = self.deadline_misses.saturating_add(1);
                PulseError::DeadlineMiss
            }
            TransportError::Failed => {
                self.transport_errors = self.transport_errors.saturating_add(1);
                PulseError::Transport
            }
        }
    }
}

impl<T: Esp32RmtTransport> PulseEngineBackend for Esp32Rmt<T> {
    fn state(&self) -> PulseState {
        self.state
    }

    fn configure(&mut self, tick_hz: u32) -> Result<(), PulseError> {
        if tick_hz == 0 || self.max_symbols == 0 || self.state == PulseState::Busy {
            return Err(PulseError::InvalidConfig);
        }
        if !self.transport.init(tick_hz) {
            return Err(self.transport_failure(TransportError::Failed));
        }
        self.tick_hz = tick_hz;
        self.state = PulseState::Ready;
        Ok(())
    }

    fn transmit(&mut self, symbols: &[PulseSymbol], max_block_us: u32) -> Result<(), PulseError> {
        if self.state != PulseState::Ready {
            return Err(PulseError::NotReady);
        }
        if symbols.is_empty()
            || symbols.len() > self.max_symbols
            || symbols.iter().any(|symbol| !symbol.is_valid())
        {
            self.oversized = self.oversized.saturating_add(1);
            return Err(PulseError::TooManySymbols);
        }
        self.state = PulseState::Busy;
        if let Err(error) = self.transport.write(symbols, max_block_us) {
            return Err(self.transport_failure(error));
        }
        self.state = PulseState::Ready;
        self.writes = self.writes.saturating_add(1);
        self.symbols = self
            .symbols
            .saturating_add(u32::try_from(symbols.len()).unwrap_or(u32::MAX));
        Ok(())
    }

    fn quiesce(&mut self) -> Result<(), PulseError> {
        if self.tick_hz != 0 && !self.transport.deinit() {
            return Err(self.transport_failure(TransportError::Failed));
        }
        if self.tick_hz != 0 {
            self.state = PulseState::Suspended;
        }
        Ok(())
    }

    fn recover(&mut self) -> Result<(), PulseError> {
        if self.tick_hz == 0 {
            return Err(PulseError::NotReady);
        }
        if !self.transport.deinit() || !self.transport.init(self.tick_hz) {
            return Err(self.transport_failure(TransportError::Failed));
        }
        self.state = PulseState::Ready;
        self.recoveries = self.recoveries.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        deadline: bool,
        writes: u8,
    }

    impl Esp32RmtTransport for Fake {
        fn init(&mut self, _: u32) -> bool {
            true
        }
        fn write(&mut self, _: &[PulseSymbol], _: u32) -> Result<(), TransportError> {
            self.writes += 1;
            if self.deadline {
                Err(TransportError::Deadline)
            } else {
                Ok(())
            }
        }
        fn deinit(&mut self) -> bool {
            true
        }
    }

    #[test]
    fn symbol_bounds_deadline_and_recovery_are_attributed() {
        let mut rmt = Esp32Rmt::new(Fake::default(), 2, PulseResourcePrice::default());
        rmt.configure(1_000_000).unwrap();
        let symbols = [
            PulseSymbol {
                high_ticks: 4,
                low_ticks: 6,
            },
            PulseSymbol {
                high_ticks: 8,
                low_ticks: 2,
            },
        ];
        assert_eq!(rmt.transmit(&symbols, 100), Ok(()));
        assert_eq!(
            rmt.transmit(&[symbols[0], symbols[1], symbols[0]], 100),
            Err(PulseError::TooManySymbols)
        );
        rmt.transport.deadline = true;
        assert_eq!(rmt.transmit(&symbols, 1), Err(PulseError::DeadlineMiss));
        rmt.transport.deadline = false;
        rmt.recover().unwrap();
        assert_eq!(rmt.diagnostics(), (1, 2, 1, 0, 1, 1));
    }
}
