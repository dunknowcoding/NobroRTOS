#!/usr/bin/env python3
"""Verify the pinned NiusCam member and three representative board builds."""

import argparse
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "arduino"
PIN = "b03560adf4da07e4a16b4d9e658a01d74ccb9340"

CASES = {
    "ov2640_ai_thinker": ("esp32:esp32:esp32cam", "AiThinkerEsp32Cam"),
    "ov3660_xiao": ("esp32:esp32:XIAO_ESP32S3:PSRAM=opi", "XiaoEsp32S3Sense"),
    "ov5640_s3": ("esp32:esp32:esp32s3:PSRAM=opi,FlashSize=16M", "Esp32S3CamOv5640Fixed130"),
}

SOURCE = r'''#include <NiusCam.h>
#include <NobroNiusCam.h>
NiusCam::Camera camera;
nobro::NiusCamAdapter pipeline(camera, 262144, 4, 524288, 1);
void setup() {
  if (false) {
    NiusCam::Config config = NiusCam::Config::Eco();
    pipeline.begin(NiusCam::BoardProfiles::__PROFILE__(), config);
    NiusCam::Frame frame = pipeline.capture(1, 1000);
    pipeline.release(frame);
    pipeline.resetWindow();
    pipeline.recover();
  }
}
void loop() {}
'''


def run(command, cwd=ROOT):
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError((result.stdout + result.stderr).strip())
    return result.stdout.strip()


def verify(library: pathlib.Path) -> pathlib.Path:
    library = library.resolve(strict=True)
    properties = (library / "library.properties").read_text(encoding="utf-8")
    if "name=NiusCam" not in properties or "version=0.2.0" not in properties:
        raise RuntimeError("NiusCam must be exactly version 0.2.0")
    if run(["git", "rev-parse", "HEAD"], library) != PIN:
        raise RuntimeError(f"NiusCam checkout is not pinned to {PIN}")
    if run(["git", "status", "--porcelain"], library):
        raise RuntimeError("NiusCam checkout has local modifications")
    text = "\n".join(path.read_text(encoding="utf-8", errors="replace")
                       for path in (library / "src").glob("*.h"))
    for token in ("class Camera", "class Frame", "OV2640", "OV3660", "OV5640",
                  "Esp32S3CamN16R8", "AiThinkerEsp32Cam", "XiaoEsp32S3Sense"):
        if token not in text:
            raise RuntimeError(f"NiusCam surface lacks {token}")
    return library


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", type=pathlib.Path, required=True)
    parser.add_argument("--compile", action="store_true")
    args = parser.parse_args()
    try:
        library = verify(args.library)
        if args.compile:
            cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
            if not cli:
                raise RuntimeError("arduino-cli not found")
            with tempfile.TemporaryDirectory(prefix="nobro-camera-") as temp:
                base = pathlib.Path(temp)
                for name, (fqbn, profile) in CASES.items():
                    sketch = base / name
                    sketch.mkdir()
                    (sketch / f"{name}.ino").write_text(
                        SOURCE.replace("__PROFILE__", profile), encoding="utf-8")
                    run([cli, "compile", "--fqbn", fqbn, "--library", str(PACKAGE),
                         "--library", str(library), str(sketch)])
                    print(f"  PASS {fqbn} {name}")
    except (OSError, RuntimeError) as error:
        print(f"CAMERA INTEGRATIONS: FAIL ({error})")
        return 1
    print("CAMERA INTEGRATIONS: PASS (NiusCam 0.2.0; OV2640/OV3660/OV5640 profiles)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
