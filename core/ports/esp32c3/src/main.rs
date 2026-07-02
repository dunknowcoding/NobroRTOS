//! NobroRTOS portable core on the ESP32-C3 (M84): the same kernel control plane and
//! net/ml/crypto/power primitives that run on the nRF52840 (Cortex-M4) execute here on
//! RISC-V rv32imc, self-certifying over USB-Serial-JTAG. The collector's serial_regex
//! protocol can ingest the report line.
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use esp_println::println;

use nobro_crypto::Aes128;
use nobro_kernel::{
    Capability, CapabilityGrantTable, CapabilitySet, ModuleId, QuotaLedger, SupervisionAction,
    SystemBudget, TaskSupervisor,
};
use nobro_ml::{ensemble_vote, RunningStats, Vote};
use nobro_net::{RoutingTable, SeenSet};
use nobro_power::{sampling_divisor, PowerManager, PowerMode};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

fn test_quota() -> bool {
    let mut ledger = QuotaLedger::<2>::new();
    ledger
        .register(ModuleId::Sensor, SystemBudget::new(1024, 256, 2))
        .is_ok()
        && ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(512, 128, 1))
            .is_ok()
        && ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(0, 200, 0))
            .is_err() // over the RAM budget -> rejected
}

fn test_capability() -> bool {
    let mut table = CapabilityGrantTable::<2>::new();
    let granted = CapabilitySet::empty().with(Capability::Bus0);
    table.register(ModuleId::Bus, granted).is_ok()
        && table.authorize(ModuleId::Bus, Capability::Bus0).is_ok()
        && table.authorize(ModuleId::Bus, Capability::Radio).is_err()
}

fn test_supervision() -> bool {
    let mut sup = TaskSupervisor::<2>::new(1, 3, 5);
    sup.register(ModuleId::Sensor, 10_000, 0).ok();
    matches!(sup.poll(11_000), SupervisionAction::Restart(ModuleId::Sensor))
        && sup.checkin(ModuleId::Sensor, 12_000).is_ok()
        && matches!(sup.poll(13_000), SupervisionAction::Healthy)
}

fn test_mesh() -> bool {
    let mut rt = RoutingTable::<4>::new();
    rt.update(5, 2, 1, 1);
    rt.update(5, 9, 3, 2); // fresher backup route wins
    let mut seen = SeenSet::<4>::new();
    rt.next_hop(5) == Some(9) && seen.observe(42) && !seen.observe(42)
}

fn test_ml() -> bool {
    let mut s = RunningStats::new();
    for x in [1000.0f32, 1001.0, 999.0, 1000.0, 1002.0, 998.0] {
        s.update(x);
    }
    let votes = [
        Vote { class: 1, confidence_milli: 900 },
        Vote { class: 0, confidence_milli: 600 },
        Vote { class: 1, confidence_milli: 800 },
    ];
    s.is_anomaly(1200.0, 3.0) && !s.is_anomaly(1001.0, 3.0)
        && ensemble_vote(&votes, 3) == Some((1, 739))
}

fn test_crypto() -> bool {
    let key = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        0x0d, 0x0e, 0x0f];
    let pt = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
        0xdd, 0xee, 0xff];
    let ct = [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70,
        0xb4, 0xc5, 0x5a];
    Aes128::new(&key).encrypt_block(&pt) == ct // FIPS-197 vector on RISC-V
}

fn test_power() -> bool {
    let pm = PowerManager::new(1_000_000, 100_000);
    pm.select(false, Some(50_000)) == PowerMode::LowPower
        && sampling_divisor(100) == 1
        && sampling_divisor(2) == 16
}

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!("NobroRTOS ESP32-C3 port (M84) - portable core on RISC-V rv32imc");

    let results = [
        ("quota", test_quota()),
        ("capability", test_capability()),
        ("supervision", test_supervision()),
        ("mesh", test_mesh()),
        ("ml", test_ml()),
        ("crypto", test_crypto()),
        ("power", test_power()),
    ];
    let mut all = true;
    for (name, ok) in results {
        println!("  {}: {}", name, if ok { "PASS" } else { "FAIL" });
        all &= ok;
    }

    loop {
        println!("NOBRO-C3 arch=riscv32imc subsystems=7 all_pass={}", u32::from(all));
        delay.delay_millis(1000);
    }
}
