# nobro-admission

The allocation-free admission core shared by generated firmware build scripts and
NobroRTOS dynamic manifest admission. It validates task/profile capacity, deadlines,
jitter, execution and blocking bounds, utilization, and bounded fixed-priority
response time. Successful static builds retain only an `AdmittedWorkload` table in
firmware `.rodata`; task labels stay in the host build script for clear diagnostics.
