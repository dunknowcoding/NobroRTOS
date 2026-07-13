"""Declarative -> buildable Rust firmware generator (developer-experience Track 1A).

Generates a compiling, in-tree NobroRTOS firmware app from a declarative module spec
(the same fields a non-Rust user puts in nobro-contract.json: criticality + memory
budget). The generated `main.rs` assembles a SystemManifest via `BootAssembly`,
admits it, and exports the host-readable `NOBRO_MANIFEST_REPORT` /
`NOBRO_ADMISSION_REPORT`. The Rust is compiler-checked, so the contract is preserved
- the user edits JSON / re-runs the generator and gets real firmware without writing
Rust by hand.

The app is created as a workspace member under `core/apps/<use-case>/<name>/` so it builds with
the existing toolchain (`cargo build --locked -p <name>` after the one-time lock refresh),
exactly like the bundled demos.
"""
from __future__ import annotations

import json
from pathlib import Path

_CRITICALITY_RUST = {
    "best_effort": "BestEffort",
    "user": "User",
    "driver": "Driver",
    "system": "System",
}
_APP_CATEGORIES = ("ai", "connectivity", "control", "imu", "interop", "kernel", "storage")

_MAIN_RS = r"""//! Generated NobroRTOS firmware app (developer-experience Track 1A).
//! Assembles the manifest for the declared module(s) via BootAssembly, admits it,
//! and exports the host-readable reports. Edit nobro-contract.json + regenerate, or
//! edit this file directly - the contract is preserved by the Rust compiler.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{
    traits::PlatformHal, ActivePlatform as Hal, BoardPackageReport, BoardProfileReport,
    ACTIVE_BOARD_PACKAGE,
};
use nobro_kernel::{
    kernel_module_spec, AdmissionReport, BootAssembly, BootAssemblyReports, Criticality,
    DeadlineContract, FaultThresholds, ManifestReport, MemoryBudget, ModuleId, ModuleSpec,
    StartupDependency, SystemProfile,
};

#[no_mangle]
#[used]
static mut NOBRO_BOARD_PROFILE_REPORT: BoardProfileReport = BoardProfileReport::zeroed();
#[no_mangle]
#[used]
static mut NOBRO_BOARD_PACKAGE_REPORT: BoardPackageReport = BoardPackageReport::zeroed();
#[no_mangle]
#[used]
static mut NOBRO_MANIFEST_REPORT: ManifestReport = ManifestReport::zeroed();
#[no_mangle]
#[used]
static mut NOBRO_ADMISSION_REPORT: AdmissionReport = AdmissionReport::zeroed();

type AppBoot = BootAssembly<4, 4, 4, 4, 4, 4, 4, 4, 16>;

const DEPENDENCIES: [StartupDependency; 1] =
    [StartupDependency::new(ModuleId::App(0), ModuleId::Kernel)];

fn kernel_spec() -> ModuleSpec {
    kernel_module_spec(
        MemoryBudget::new(24 * 1024, 8 * 1024, 4),
        DeadlineContract::new(20_000, 10),
    )
}

// Module "__MODULE__" - generated from nobro-contract.json.
fn app_spec() -> ModuleSpec {
    ModuleSpec::new(ModuleId::App(0), Criticality::__CRIT__)
        .memory(MemoryBudget::new(__FLASH__, __RAM__, __POOL__))
}

#[entry]
fn main() -> ! {
    unsafe {
        NOBRO_BOARD_PROFILE_REPORT =
            BoardProfileReport::from_board::<<Hal as PlatformHal>::Board>();
        NOBRO_BOARD_PACKAGE_REPORT = BoardPackageReport::from_package(&ACTIVE_BOARD_PACKAGE);
    }

    let specs = [kernel_spec(), app_spec()];
    match AppBoot::build_with_failure(
        &specs,
        &DEPENDENCIES,
        SystemProfile::NRF52840_CORE,
        FaultThresholds::DEFAULT,
        0,
    ) {
        Ok(boot) => write_reports(boot.reports()),
        Err(failure) => write_reports(failure.reports()),
    }

    // Your "__MODULE__" control loop goes here. The module is admitted with the
    // budget + criticality declared in nobro-contract.json.
    loop {
        asm::delay(8_000_000);
    }
}

fn write_reports(reports: BootAssemblyReports) {
    unsafe {
        NOBRO_MANIFEST_REPORT = reports.manifest;
        NOBRO_ADMISSION_REPORT = reports.admission;
    }
}
"""

_BUILD_RS = """use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let memory = if env::var("CARGO_FEATURE_BOARD_NICENANO_S140").is_ok() {
        PathBuf::from("../../../memory-s140.x")
    } else {
        PathBuf::from("../../../memory-nosd.x")
    };
    let dest = out.join("memory.x");
    fs::copy(&memory, &dest).expect("copy memory.x");
    println!("cargo:rerun-if-changed={}", memory.display());
    println!("cargo:rerun-if-changed=../../../memory-nosd.x");
    println!("cargo:rerun-if-changed=../../../memory-s140.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_NICENANO_S140");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BOARD_PROMICRO_NOSD");
}
"""


def _cargo_toml(pkg: str, binname: str) -> str:
    return f"""[package]
name = "{pkg}"
version.workspace = true
edition.workspace = true

[[bin]]
name = "{binname}"
path = "src/main.rs"
required-features = ["board-promicro-nosd"]

[dependencies]
nobro-hal = {{ path = "../../../crates/nobro_hal", default-features = false }}
nobro-kernel = {{ path = "../../../crates/nobro_kernel" }}
cortex-m = {{ version = "0.7", features = ["critical-section-single-core"] }}
cortex-m-rt = "0.7"
defmt = "0.3"
defmt-rtt = "0.4"
panic-halt = "0.2"

[features]
default = ["board-promicro-nosd"]
board-promicro-nosd = ["nobro-hal/board-promicro-nosd"]
board-nicenano-s140 = ["nobro-hal/board-nicenano-s140"]
"""


def _readme(name: str, pkg: str, binname: str, module: str) -> str:
    return f"""# {name}

Generated NobroRTOS firmware app for module **{module}** (developer-experience
Track 1A: declarative -> buildable firmware, no hand-written Rust required).

Edit `nobro-contract.json` to change the module's criticality or memory budget, then
re-run the generator, or edit `src/main.rs` directly. The contract is preserved by
the Rust compiler and re-checked at admission.

## Build

```powershell
cd core
# The generator registered a new workspace member; resolve it once and retain
# the resulting Cargo.lock, then keep every build locked.
cargo generate-lockfile
cargo build --locked -p {pkg} --release
```

## Verify on hardware

```powershell
# Follow docs/GETTING_STARTED.md in the NobroRTOS repository to flash and read the
# NOBRO_MANIFEST_REPORT / NOBRO_ADMISSION_REPORT records.
```
"""


def _contract_json(name: str, module: str, criticality: str, flash: int, ram: int, pool: int) -> str:
    contract = {
        "name": name,
        "modules": [
            {
                "id": f"app0:{module}",
                "criticality": criticality,
                "requires_bits": 0,
                "owns_bits": 0,
                "memory": {"flash_bytes": flash, "ram_bytes": ram, "pool_slots": pool},
                "deadline": None,
            }
        ],
        "startup": [{"module": "app0", "depends_on": "kernel"}],
    }
    return json.dumps(contract, indent=2, sort_keys=True) + "\n"


def _register_member(core_cargo: Path, category: str, dir_name: str) -> bool:
    """Append apps/<category>/<dir_name> to the workspace members if absent. Returns True
    if a change was made."""
    text = core_cargo.read_text(encoding="utf-8")
    member = f'"apps/{category}/{dir_name}"'
    if member in text:
        return False
    marker = '    "apps/imu/imu_i2c_demo",'
    if marker in text:
        text = text.replace(marker, f'{marker}\n    {member},', 1)
    else:
        # Fall back: insert before the closing bracket of members = [ ... ].
        idx = text.index("members = [")
        end = text.index("]", idx)
        text = text[:end] + f'    {member},\n' + text[end:]
    core_cargo.write_text(text, encoding="utf-8")
    return True


def generate_rust_app(
    repo_root: Path,
    name: str,
    module: str,
    criticality: str,
    flash_bytes: int,
    ram_bytes: int,
    pool_slots: int,
    category: str = "control",
    overwrite: bool = False,
) -> dict:
    if criticality not in _CRITICALITY_RUST:
        raise ValueError(f"unsupported criticality {criticality!r}")
    if category not in _APP_CATEGORIES:
        raise ValueError(f"unsupported app category {category!r}")
    dir_name = name.replace("-", "_")
    pkg = name.replace("_", "-")
    binname = dir_name

    crate = repo_root / "core" / "apps" / category / dir_name
    if crate.exists() and not overwrite:
        return {"passing": False, "error": f"{crate} exists (use --overwrite)"}

    files = {
        "Cargo.toml": _cargo_toml(pkg, binname),
        "build.rs": _BUILD_RS,
        "src/main.rs": _MAIN_RS.replace("__CRIT__", _CRITICALITY_RUST[criticality])
        .replace("__FLASH__", str(flash_bytes))
        .replace("__RAM__", str(ram_bytes))
        .replace("__POOL__", str(pool_slots))
        .replace("__MODULE__", module),
        "nobro-contract.json": _contract_json(name, module, criticality, flash_bytes, ram_bytes, pool_slots),
        "README.md": _readme(name, pkg, binname, module),
    }

    written = []
    for rel, content in files.items():
        dest = crate / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(content, encoding="utf-8")
        written.append(dest.relative_to(repo_root).as_posix())

    registered = _register_member(repo_root / "core" / "Cargo.toml", category, dir_name)

    return {
        "passing": True,
        "package": pkg,
        "bin": binname,
        "crate_dir": crate.relative_to(repo_root).as_posix(),
        "files": sorted(written),
        "registered_member": registered,
        "build_hint": (
            f"cd core && cargo generate-lockfile && "
            f"cargo build --locked -p {pkg} --release"
        ),
    }
