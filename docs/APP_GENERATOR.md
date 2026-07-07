# Generate a firmware app (no Rust by hand)

`gen-app` turns a declarative module spec into a **buildable NobroRTOS firmware app**.
You describe the module (criticality + memory budget); the generator emits a workspace
crate whose `main.rs` assembles the manifest via `BootAssembly`, admits it, and exports
the host-readable `NOBRO_MANIFEST_REPORT` / `NOBRO_ADMISSION_REPORT`. The generated Rust
is compiler-checked, so your contract is preserved - you never hand-write Rust.

This is developer-experience Track 1A (see [DEVELOPER_EXPERIENCE.md](DEVELOPER_EXPERIENCE.md)).

## Generate

```powershell
python tools/nobro_contract_tool.py gen-app --name my_control_app --module control
# options: --criticality {best_effort|user|driver|system}  --flash <bytes>  --ram <bytes>  --pool <slots>
```

This writes `core/apps/my_control_app/` (`Cargo.toml`, `build.rs`, `src/main.rs`,
`nobro-contract.json`, `README.md`) and registers it as a workspace member.

## Build

```powershell
cd core
cargo build -p my-control-app --release
```

## Verify on hardware

Flash and read the reports (see [HARDWARE_BRINGUP.md](HARDWARE_BRINGUP.md)). A booted
app populates `NOBRO_MANIFEST_REPORT` (magic `NBMF`) and `NOBRO_ADMISSION_REPORT`
(magic `NBAD`); both carry the module count and a sealed checksum, so a host tool
confirms the manifest assembled and admission passed without a `defmt` decoder.

Verified end to end on the development board: a generated `driver`-criticality app compiles for
`thumbv7em`, boots, assembles a 2-module manifest (kernel + your module), and passes
admission.

## Editing the contract

Edit `nobro-contract.json` (the module's criticality / memory budget) and re-run
`gen-app --overwrite`, or edit `src/main.rs` directly. Either way the manifest is
re-validated by the compiler and at admission.

## Authoring module logic in C or C++

`gen-app` scaffolds a Rust app. To write module *logic* outside Rust, generate a
C or C++ module skeleton over the [C ABI](../bindings/c/include/nobro_app.h):

```powershell
python tools/nobro_contract_tool.py gen-module --name my_sensor --lang c   --out my_mod
python tools/nobro_contract_tool.py gen-module --name my_sensor --lang cpp --out my_mod
```

This writes an editable module (`nobro_app_init()` once, `nobro_app_poll()` each
cycle) and prints the build command, which compiles + links your file into the
`c_abi_demo` firmware via the `c-source` / `cpp-source` path (needs
`arm-none-eabi-gcc` / `g++`). Both languages are verified end to end on the development board - the
kernel admits the C or C++ module and it drives a sensor to a passing report. See
[bindings/c/README.md](../bindings/c/README.md) and
[bindings/cpp/README.md](../bindings/cpp/README.md).
