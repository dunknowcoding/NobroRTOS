# Publish Checklist (owner-executed)

Publishing is an external, irreversible act on the maintainer's accounts. Agents and CI
**prepare and verify**; a human presses the buttons. Everything below is idempotent to
re-run.

## 0. Preconditions (automated, must be green)

```bash
python tools/run_checks.py                       # RESULT: ALL PASS + evidence pack
python tools/check_release_versions.py --release # same x.y.z on every surface
```

## 1. Build the artifacts

```bash
python tools/package_arduino.py --zip                       # _work/NobroRTOS-arduino.zip
cd bindings/python && python -m pip wheel . --no-deps -w ../../_work/dist
```

Smoke each artifact the way a user meets it:
- unzip the Arduino package into a clean `libraries/` dir and compile
  `examples/ReportReader` for two architectures (`arduino-cli compile --libraries ...`);
- `pip install` the wheel into a fresh venv, `import nobro_rtos`, run `nobro-rtos --help`.

## 2. Arduino Library Manager (once per new library, then automatic)

1. Tag the release: `git tag x.y.z && git push --tags` (the registry indexes tags).
2. Submit the repository URL to the registry:
   <https://github.com/arduino/library-registry> (one PR adding the repo URL).
3. Subsequent releases are picked up from new tags automatically — keep
   `library.properties` version == tag.

## 3. PyPI

1. Dry run against TestPyPI first: `twine upload -r testpypi _work/dist/*.whl`,
   then `pip install -i https://test.pypi.org/simple/ nobro-rtos-tools` in a venv.
2. Real upload: `twine upload _work/dist/*.whl` (maintainer credentials).

## 4. After publishing

- Bump all three surface versions together (the alignment gate enforces this).
- Update the README install snippets only after the artifacts are live.
- Keep `sdk/sdk-manifest.json` on repo-relative paths until the published URLs exist.
