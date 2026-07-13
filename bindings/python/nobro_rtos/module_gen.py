"""Generate a starter NobroRTOS module in C or C++ over the C ABI.

A non-Rust author runs one command and gets an editable module file (init once,
poll each cycle) plus the exact build command. The file is compiled + linked into
the c_abi_demo firmware via the existing `c-source` / `cpp-source` build path, so
the skeleton is guaranteed buildable from the start.
"""
from __future__ import annotations

from pathlib import Path

_C_SKELETON = """\
/* {name} - a NobroRTOS module written in C.
 *
 * Implement nobro_app_init() once (one-time setup) and nobro_app_poll() each cycle.
 * Host services available (see bindings/c/include/nobro_app.h):
 *   uint64_t nobro_now_us(void);
 *   int32_t  nobro_i2c_write(uint8_t addr, const uint8_t* tx, uint32_t len);
 *   int32_t  nobro_i2c_write_read(uint8_t addr, const uint8_t* tx, uint32_t tx_len,
 *                                 uint8_t* rx, uint32_t rx_len);
 *   void     nobro_publish_imu(uint8_t who, uint8_t addr, int16_t ax, int16_t ay,
 *                              int16_t az, int16_t gx, int16_t gy, int16_t gz,
 *                              int16_t temp_raw);
 */
#include "nobro_app.h"

int32_t nobro_app_init(void) {{
    /* one-time setup, e.g. wake a sensor over I2C */
    return 0;
}}

int32_t nobro_app_poll(void) {{
    /* called every cycle by the kernel: read sensors, compute, publish */
    return 0;
}}
"""

_CPP_SKELETON = """\
// {name} - a NobroRTOS module written in C++.
//
// A plain struct with static init()/poll(), registered in one line. Bare-metal-safe:
// no global constructors, vtables, exceptions, or RTTI. See bindings/cpp/include.
#include "nobro_app.hpp"

struct {struct_name} {{
    static int32_t init() {{
        // one-time setup, e.g. nobro::I2c::write(0x68, ...);
        return 0;
    }}
    static int32_t poll() {{
        // called every cycle by the kernel: read sensors, compute, nobro::publish_*()
        return 0;
    }}
}};

NOBRO_REGISTER_MODULE({struct_name})
"""


def _struct_name(name: str) -> str:
    return "".join(part.capitalize() for part in name.replace("-", "_").split("_")) + "Module"


def generate_module(name: str, lang: str, out_dir: str, overwrite: bool = False) -> dict:
    lang = lang.lower()
    if lang not in ("c", "cpp"):
        return {"passing": False, "error": f"unknown lang '{lang}' (use c or cpp)"}

    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    ext = "c" if lang == "c" else "cpp"
    module_path = out / f"{name}.{ext}"
    if module_path.exists() and not overwrite:
        return {"passing": False, "error": f"{module_path} exists (use --overwrite)"}

    if lang == "c":
        content = _C_SKELETON.format(name=name)
        feature, env = "c-source", "NOBRO_C_MODULE"
    else:
        content = _CPP_SKELETON.format(name=name, struct_name=_struct_name(name))
        feature, env = "cpp-source", "NOBRO_CPP_MODULE"
    module_path.write_text(content, encoding="utf-8")

    build_cmd = (
        f"{env}={module_path.resolve().as_posix()} cargo build --locked -p nobro-c-abi-demo "
        f"--no-default-features --features board-promicro-nosd,{feature} --release"
    )
    readme = out / f"{name}.README.md"
    readme.write_text(
        f"# {name} ({lang.upper()} module)\n\n"
        f"Edit `{module_path.name}` - `nobro_app_init()` runs once, `nobro_app_poll()` "
        f"every cycle. Then build it into the c_abi_demo firmware (run from `core/`):\n\n"
        f"```\n{build_cmd}\n```\n\n"
        f"For the public flash and report workflow, see docs/GETTING_STARTED.md in "
        f"the NobroRTOS repository.\n",
        encoding="utf-8",
    )

    return {
        "passing": True,
        "lang": lang,
        "module": module_path.resolve().as_posix(),
        "readme": readme.resolve().as_posix(),
        "build": build_cmd,
    }
