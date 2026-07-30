//! Short, destructive-to-no-user-state PIO0 instruction/FIFO self-test.
//!
//! The test installs a three-instruction program, executes it on SM0, observes
//! the expected word through the live RX FIFO, then stops the state machine and
//! uninstalls the program. It touches no GPIO and releases every PIO resource
//! before the application starts.

use rp2040_hal as hal;

use hal::pio::{PIOBuilder, PIOExt};

const EXPECTED_WORD: u32 = 0x15;
const POLL_LIMIT: usize = 100_000;

pub fn run(pio0: hal::pac::PIO0, resets: &mut hal::pac::RESETS) -> bool {
    let (mut pio, sm0, _, _, _) = pio0.split(resets);
    let program = pio::pio_asm!(
        ".wrap_target",
        "set x, 21",
        "mov isr, x",
        "push block",
        ".wrap"
    )
    .program;
    let Ok(installed) = pio.install(&program) else {
        return false;
    };
    let (sm, mut rx, tx) = PIOBuilder::from_installed_program(installed).build(sm0);
    let sm = sm.start();

    let mut observed = None;
    for _ in 0..POLL_LIMIT {
        if let Some(word) = rx.read() {
            observed = Some(word);
            break;
        }
        core::hint::spin_loop();
    }

    let sm = sm.stop();
    let (_sm, installed) = sm.uninit(rx, tx);
    pio.uninstall(installed);
    observed == Some(EXPECTED_WORD)
}
