# esp-sync (NobroRTOS vendored compatibility fork)

This fork starts from upstream `esp-sync` 0.2.1 at esp-hal revision
`8cbe014242843d5b4a9ce14d018f20f16e2ebf18`. It keeps the upstream API and
adds one silicon-revision boundary for ESP32-P4: the CSR `0x347`
`mintthresh` workaround is enabled only by the explicit
`esp32p4-zcmp-workaround` feature. ESP32-P4 version-1 silicon does not
implement that CSR and otherwise traps before `main`; version-3 compositions
must enable the feature to retain the Zcmp erratum workaround.

The fork remains MIT OR Apache-2.0 under the included upstream licenses.

# Upstream esp-sync

[![Crates.io](https://img.shields.io/crates/v/esp-sync?labelColor=1C2C2E&color=C96329&logo=Rust&style=flat-square)](https://crates.io/crates/esp-sync)
[![docs.rs](https://img.shields.io/docsrs/esp-sync?labelColor=1C2C2E&color=C96329&logo=rust&style=flat-square)](https://docs.espressif.com/projects/rust/esp-sync/latest/)
![MSRV](https://img.shields.io/badge/MSRV-1.95.0-blue?labelColor=1C2C2E&style=flat-square)
![Crates.io](https://img.shields.io/crates/l/esp-sync?labelColor=1C2C2E&style=flat-square)
[![Matrix](https://img.shields.io/matrix/esp-rs:matrix.org?label=join%20matrix&labelColor=1C2C2E&color=BEC5C9&logo=matrix&style=flat-square)](https://matrix.to/#/#esp-rs:matrix.org)

This crate provides an optimized raw mutex (a lock type that doesn't wrap any data) for ESP32 devices.

## Minimum Supported Rust Version (MSRV)

This crate is guaranteed to compile when using the latest stable Rust version at the time of the crate's release. It _might_ compile with older versions, but that may change in any new release, including patches.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the
work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
