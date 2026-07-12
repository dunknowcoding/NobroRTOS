use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

#[embassy_executor::task]
async fn motor() {
    loop { Timer::after(Duration::from_millis(5)).await; }
}

#[embassy_executor::task]
async fn imu() {
    loop { Timer::after(Duration::from_millis(10)).await; }
}

#[embassy_executor::task]
async fn camera() {
    loop { Timer::after(Duration::from_millis(40)).await; }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    spawner.spawn(motor()).unwrap();
    spawner.spawn(imu()).unwrap();
    spawner.spawn(camera()).unwrap();
}
