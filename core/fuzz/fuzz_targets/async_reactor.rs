#![no_main]

use core::future::Future;
use core::pin::{pin, Pin};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use libfuzzer_sys::fuzz_target;
use nobro_kernel::async_rt::{
    AsyncCore, CancelToken, Channel, ReactorExecutor, Signal, TimerQueue,
};

static CORE: AsyncCore<1> = AsyncCore::new();

struct WakeCount(u8);

impl Future for WakeCount {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 == 0 {
            Poll::Ready(())
        } else {
            self.0 -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

fn no_op_waker() -> Waker {
    static VTABLE: RawWakerVTable =
        RawWakerVTable::new(|data| RawWaker::new(data, &VTABLE), |_| {}, |_| {}, |_| {});
    // The vtable never dereferences or frees the null data pointer.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

fuzz_target!(|data: &[u8]| {
    let channel = Channel::<u8, 2>::new();
    let timers = TimerQueue::<2>::new();
    let signal = Signal::new();
    let cancel = CancelToken::new();
    let waker = no_op_waker();
    let mut cx = Context::from_waker(&waker);
    let mut now = 0u64;
    let wake_count = data.first().copied().unwrap_or(0) % 8;
    let mut task = pin!(WakeCount(wake_count));
    let mut reactor = ReactorExecutor::<1>::bind(&CORE);
    reactor.spawn(task.as_mut()).unwrap();

    for chunk in data.chunks(3) {
        let op = chunk.first().copied().unwrap_or(0) % 9;
        let value = chunk.get(1).copied().unwrap_or(0);
        let delta = u64::from(chunk.get(2).copied().unwrap_or(0));
        match op {
            0 => {
                let _ = channel.try_send(value);
            }
            1 => {
                let _ = channel.try_recv();
            }
            2 => {
                let mut send = pin!(channel.send(value));
                let _ = send.as_mut().poll(&mut cx);
            }
            3 => {
                let mut recv = pin!(channel.recv());
                let _ = recv.as_mut().poll(&mut cx);
            }
            4 => {
                let mut sleep = pin!(timers.sleep_until(now.saturating_add(delta)));
                let _ = sleep.as_mut().poll(&mut cx);
            }
            5 => {
                now = now.saturating_add(delta);
                let _ = timers.advance(now);
            }
            6 => signal.notify(),
            7 => {
                let mut wait = pin!(signal.wait());
                let _ = wait.as_mut().poll(&mut cx);
            }
            _ => {
                if value & 1 == 0 {
                    cancel.cancel();
                }
                let mut cancelled = pin!(cancel.cancelled());
                let _ = cancelled.as_mut().poll(&mut cx);
            }
        }
        assert!(channel.len() <= 2);
        let fuel = (delta % 3) as u32;
        let stats = reactor.run_ready(fuel);
        assert!(stats.polled <= fuel && stats.live <= 1);
    }
});
