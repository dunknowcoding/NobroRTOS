//! PPI wiring: GPIOTE event -> TIMER CAPTURE (zero-CPU timestamp latch).

use nrf52840_pac::{GPIOTE, PPI, TIMER0};

use crate::board::{LED_PIN, MVK_TRIGGER_PIN};

const PPI_CH: usize = 0;

pub unsafe fn mvk_setup_gpiote_ppi_capture() {
    let gpiote = GPIOTE::ptr();
    (*gpiote).config[0].write(|w| {
        w.mode()
            .event()
            .psel()
            .bits(MVK_TRIGGER_PIN)
            .polarity()
            .lo_to_hi()
            .outinit()
            .low()
    });
    (*gpiote).intenclr.write(|w| w.in0().clear_bit());

    let gpiote_event = core::ptr::addr_of!((*gpiote).events_in[0]) as u32;
    let timer = TIMER0::ptr();
    let timer_capture1 = core::ptr::addr_of!((*timer).tasks_capture[1]) as u32;

    let ppi = PPI::ptr();
    (*ppi).ch[PPI_CH].eep.write(|w| unsafe { w.bits(gpiote_event) });
    (*ppi).ch[PPI_CH].tep.write(|w| unsafe { w.bits(timer_capture1) });
    (*ppi).chenset.write(|w| unsafe { w.bits(1 << PPI_CH) });
}

pub unsafe fn led_init_output() {
    gpio_output(LED_PIN, false);
}

pub unsafe fn led_toggle() {
    let pin = LED_PIN as u32;
    if pin < 32 {
        let p = nrf52840_pac::P0::ptr();
        let cur = (*p).out.read().bits();
        (*p).out.write(|w| w.bits(cur ^ (1 << pin)));
    } else {
        let bit = pin - 32;
        let p = nrf52840_pac::P1::ptr();
        let cur = (*p).out.read().bits();
        (*p).out.write(|w| w.bits(cur ^ (1 << bit)));
    }
}

pub unsafe fn trigger_input_init() {
    gpio_input_pullup(MVK_TRIGGER_PIN);
}

unsafe fn gpio_output(pin: u8, high: bool) {
    let pin = pin as u32;
    if pin < 32 {
        let p = nrf52840_pac::P0::ptr();
        (*p).pin_cnf[pin as usize].write(|w| w.dir().output());
        if high {
            (*p).outset.write(|w| w.bits(1 << pin));
        } else {
            (*p).outclr.write(|w| w.bits(1 << pin));
        }
    } else {
        let bit = pin - 32;
        let p = nrf52840_pac::P1::ptr();
        (*p).pin_cnf[bit as usize].write(|w| w.dir().output());
        if high {
            (*p).outset.write(|w| w.bits(1 << bit));
        } else {
            (*p).outclr.write(|w| w.bits(1 << bit));
        }
    }
}

unsafe fn gpio_input_pullup(pin: u8) {
    let pin = pin as u32;
    if pin < 32 {
        let p = nrf52840_pac::P0::ptr();
        (*p).pin_cnf[pin as usize].write(|w| {
            w.dir()
                .input()
                .input()
                .connect()
                .pull()
                .pullup()
        });
    } else {
        let bit = pin - 32;
        let p = nrf52840_pac::P1::ptr();
        (*p).pin_cnf[bit as usize].write(|w| {
            w.dir()
                .input()
                .input()
                .connect()
                .pull()
                .pullup()
        });
    }
}
