<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>Core sangat kecil. Real-time yang serius. Perangkat keras Anda.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core menjawab pertanyaan utama produk embedded: **apa yang berjalan
berikutnya, dan apakah dapat selesai tepat waktu?** Runtime ringan tanpa heap
memeriksa deadline sebelum mulai, sementara driver, interrupt, timer, dan
kebijakan daya tetap dikendalikan aplikasi.

## Keunggulan utama

- **Sangat ringan:** konfigurasi MCS-51 terukur dimulai dari 504 byte program dan 7 byte data core.
- **Validasi sebelum berjalan:** overload, timing tidak valid, ID ganda, dan response time mustahil ditolak lebih awal.
- **Peralatan fleksibel:** Arduino, PlatformIO, Rust `no_std`, serta pembangkitan C/assembly dengan Python.
- **Perilaku terbatas:** kapasitas tetap, prioritas eksplisit, dan tanpa stack task tersembunyi.

## Mulai cepat

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

Lihat panduan [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md),
[Python](docs/PYTHON.md), dan [Rust](docs/RUST.md).

## Posisi teknis yang kuat

Dalam model evaluasi 32 dimensi dan 19 sistem yang disertakan, NobroRTOS meraih
**7,23/10 dan peringkat ketiga**. Ini adalah evaluasi arsitektur dan kemampuan
yang terbatas, bukan klaim universal atas benchmark, sertifikasi, atau kematangan komersial.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="Peringkat teknis RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="Rincian skor teknis RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="Perbandingan radar RTOS" width="96%"></p>

## NobroRTOS tingkat lanjut

Kami juga menyediakan core tingkat lanjut yang disesuaikan untuk kebutuhan
timing, keamanan, konektivitas, AI, robotika, dan hardware khusus. Core tersebut
lulus stress test hardware simultan pada 18 development board nyata.

<p align="center"><img src="docs/images/real_tests.jpg" alt="Stress test NobroRTOS pada 18 board" width="92%"></p>

Bergabunglah di [Discord](https://discord.gg/NrRrQKmT2) dan kunjungi
[NiusRobotLab di YouTube](https://www.youtube.com/@NiusRobotLab).

NobroRTOS Core dirilis dengan **GPL-3.0-only**; [LICENSE](LICENSE) adalah ketentuan resmi.
