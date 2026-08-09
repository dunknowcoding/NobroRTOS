<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>Ein winziger Kern. Ernsthafte Echtzeit. Ihre Hardware.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core beantwortet die zentrale Frage eines Embedded-Produkts:
**Was läuft als Nächstes, und wird es rechtzeitig fertig?** Die schlanke,
heapfreie Laufzeit prüft Deadlines vor dem Start; Treiber, Interrupts, Timer
und Energiesparmodi bleiben unter Kontrolle der Anwendung.

## Zentrale Vorteile

- **Ultraleicht:** Die gemessene MCS-51-Minimalkonfiguration benötigt 504 Byte Programm und 7 Byte Kerndaten.
- **Prüfung vor dem Start:** Überlast, ungültiges Timing, doppelte IDs und unerfüllbare Antwortzeiten werden abgelehnt.
- **Flexible Werkzeuge:** Arduino, PlatformIO, Rust `no_std` und Python-generierte C/Assembler-Schnittstellen.
- **Begrenztes Verhalten:** Feste Kapazität, explizite Prioritäten und keine versteckten Task-Stacks.

## Schnellstart

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

Siehe [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md),
[Python](docs/PYTHON.md) und [Rust](docs/RUST.md).

## Starke technische Bewertung

Im enthaltenen Modell mit 32 Dimensionen und 19 Systemen erreicht NobroRTOS
**7,23/10 und Platz drei**. Es handelt sich um eine abgegrenzte Architektur-
und Fähigkeitsbewertung, nicht um eine universelle Aussage zu Benchmarks,
Zertifizierung oder Marktreife.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="Technische RTOS-Rangliste" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="Technische Kategorienbewertung" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="RTOS-Radarvergleich" width="96%"></p>

## Erweiterte NobroRTOS-Kerne

Für produktspezifische Timing-, Sicherheits-, Konnektivitäts-, KI-, Robotik-
und Hardwareanforderungen bieten wir angepasste erweiterte Kerne. Sie bestanden
einen gleichzeitigen Hardware-Stresstest auf 18 Entwicklungsboards.

<p align="center"><img src="docs/images/real_tests.jpg" alt="NobroRTOS-Stresstest auf 18 Boards" width="92%"></p>

Treten Sie [Discord](https://discord.gg/NrRrQKmT2) bei und besuchen Sie
[NiusRobotLab auf YouTube](https://www.youtube.com/@NiusRobotLab).

NobroRTOS Core steht unter **GPL-3.0-only**; maßgeblich ist [LICENSE](LICENSE).
