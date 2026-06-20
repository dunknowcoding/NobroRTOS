# NobroRTOS Tools

This folder contains repository-native tools.

Tooling direction:

- SDK package builders
- Arduino and PlatformIO package generators
- host contract validators
- board fixture generators
- report decoding utilities

Generated outputs and caches must stay outside the repository.

## Contract Tool

`nobro_contract_tool.py` runs the Python contract tooling from the repository
root without requiring package installation:

```powershell
python tools/nobro_contract_tool.py check-host-contract
python tools/nobro_contract_tool.py check-distribution-metadata
python tools/nobro_contract_tool.py sample-ai-ros
python tools/nobro_contract_tool.py sample-ai-route
```
