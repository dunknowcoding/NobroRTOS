# Generate a firmware app (no Rust by hand)

`gen-app` turns a declarative module spec into a **buildable NobroRTOS firmware app**.
You describe the module (criticality + memory budget); the generator emits a workspace
crate whose `main.rs` assembles the manifest via `BootAssembly`, admits it, and exports
the host-readable `NOBRO_MANIFEST_REPORT` / `NOBRO_ADMISSION_REPORT`. The generated Rust
is compiler-checked, so your contract is preserved — you never hand-write Rust.

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

Verified end to end on board1: a generated `driver`-criticality app compiles for
`thumbv7em`, boots, assembles a 2-module manifest (kernel + your module), and passes
admission.

## Editing the contract

Edit `nobro-contract.json` (the module's criticality / memory budget) and re-run
`gen-app --overwrite`, or edit `src/main.rs` directly. Either way the manifest is
re-validated by the compiler and at admission. Next on the roadmap: a C ABI and a
C++/Arduino authoring facade so module *logic* (not just the contract) can be written
outside Rust.
