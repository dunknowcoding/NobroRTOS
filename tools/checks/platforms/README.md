# Platform checks

Board-profile, provider-tier, generated-firmware, and cross-target portability
gates. The firmware-generation gate target-builds every `application-image`,
generates every Arduino and maintained-port route, and rejects unavailable
profiles. `--arduino-builds` additionally compiles all exact Arduino routes in
the pinned Arduino-package CI job.
