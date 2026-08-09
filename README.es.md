<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>Un núcleo diminuto. Tiempo real serio. Tu hardware.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core responde a una pregunta esencial: **¿qué se ejecuta después y
puede terminar a tiempo?** Combina un runtime ligero sin heap con admisión
consciente de plazos, mientras la aplicación conserva el control de drivers,
interrupciones, temporizadores y ahorro de energía.

## Ventajas principales

- **Ultraligero:** la configuración MCS-51 medida parte de 504 bytes de programa y 7 bytes de datos del núcleo.
- **Validación antes de arrancar:** rechaza sobrecarga, tiempos inválidos, identidades duplicadas y fallos de respuesta.
- **Herramientas flexibles:** Arduino, PlatformIO, Rust `no_std` y generación C/ensamblador con Python.
- **Comportamiento acotado:** capacidad fija, prioridades explícitas y sin pilas de tareas ocultas.

## Inicio rápido

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

Consulta las guías de [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md),
[Python](docs/PYTHON.md) y [Rust](docs/RUST.md).

## Posición técnica destacada

En el modelo incluido de 32 dimensiones y 19 sistemas, NobroRTOS obtiene
**7,23/10 y ocupa el tercer puesto**. Es una evaluación acotada de arquitectura
y capacidades, no una afirmación universal sobre benchmarks, certificación o madurez comercial.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="Clasificación técnica de RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="Desglose técnico por categorías" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="Comparación radar de RTOS" width="96%"></p>

## Más allá del Core público

También ofrecemos núcleos avanzados personalizados para requisitos de tiempo,
seguridad, conectividad, IA, robótica y hardware. Han superado una prueba de
estrés simultánea con 18 placas de desarrollo reales.

<p align="center"><img src="docs/images/real_tests.jpg" alt="Prueba NobroRTOS simultánea en 18 placas" width="92%"></p>

Únete a [Discord](https://discord.gg/NrRrQKmT2) y visita
[NiusRobotLab en YouTube](https://www.youtube.com/@NiusRobotLab).

NobroRTOS Core se publica bajo **GPL-3.0-only**; [LICENSE](LICENSE) contiene los términos oficiales.
