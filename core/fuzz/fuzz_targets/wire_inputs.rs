#![no_main]

use libfuzzer_sys::fuzz_target;
use nobro_net::{secure_link, telemetry_pack, teleop, OtaImageAssembler};

fuzz_target!(|data: &[u8]| {
    let mut samples = [0i32; 32];
    let _ = telemetry_pack::unpack(data, &mut samples);
    let mut plaintext = [0u8; 256];
    let key = [0x5A; 16];
    let _ = secure_link::open_addressed(&key, data, 0, &mut plaintext);
    let _ = teleop::apply(&key, data, 1, 2, 0);

    let mut ota = OtaImageAssembler::<256, 64>::new(128, 8, [0; 32]).unwrap();
    if let Some((&index, chunk)) = data.split_first() {
        let _ = ota.receive(usize::from(index), chunk);
    }
    let _ = ota.verify();
});
