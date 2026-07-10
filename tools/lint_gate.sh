#!/usr/bin/env bash
# Lint gate (M170): clippy with warnings-as-errors across the portable crates - the
# static-analysis bar every commit must clear (the embedded stand-in for a MISRA gate).
set -u
cd "$(dirname "$0")/../core" || exit 1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/../_work/ct-lint}"

out=$(cargo clippy -p nobro-kernel -p nobro-sal -p nobro-net -p nobro-crypto -p nobro-ml \
    -p nobro-sensor -p nobro-power -p nobro-control -p nobro-conformance -p nobro-hal -p nobro-classic \
    --target x86_64-pc-windows-msvc -- -D warnings 2>&1)
rc=$?
echo "$out" | tail -3
if [ "$rc" -eq 0 ]; then
  echo "LINT GATE: PASS"
else
  echo "LINT GATE: FAIL"
  exit 1
fi
