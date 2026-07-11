//! Linked C-ABI demonstration using the same runtime implementation as `libnobro.a`.
#![no_std]
#![no_main]

#[cfg(feature = "rust-module")]
use nobro_c_abi_module as _;

// The Tier-C runtime owns admission, host services, callback dispatch, reports, and
// the embedded entry point. This binary only selects and force-links one module
// provider so the demonstration and distributed static library cannot drift.
use nobro_tierc as _;
