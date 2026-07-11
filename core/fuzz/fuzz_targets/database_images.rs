#![no_main]

use libfuzzer_sys::fuzz_target;
use nobro_database::Table;

fuzz_target!(|data: &[u8]| {
    let _ = Table::<u32, 32>::from_image(data);
    let _ = Table::<i64, 8>::from_image(data);
});
