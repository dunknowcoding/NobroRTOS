# Tools

Public, portable tooling is grouped by role:

| Directory | Audience | Contents |
| --- | --- | --- |
| [`cli/`](cli/) | Users and SDK integrations | Project, firmware, flashing, diagnostics, admission, and code-generation commands |
| [`build/`](build/) | Build systems | Linker, image, ABI, and generated-source helpers |
| [`release/`](release/) | Release maintainers | Arduino, PlatformIO, UF2, and Tier-C artifact packagers |
| [`checks/`](checks/) | Contributors and CI | Reproducible source, package, portability, and integration gates |

Use the SDK dispatcher for normal work:

```console
python sdk/cli/nobro.py --help
python sdk/cli/nobro.py project --help
python sdk/cli/nobro.py flash --help
```

Run the portable local gate with:

```console
python tools/checks/run_checks.py
```

Machine-specific hardware automation, raw logs, comparison programs, fuzz
corpora, and private reports are intentionally not tracked here.
