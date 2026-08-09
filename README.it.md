<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>Un core minuscolo. Tempo reale serio. Il tuo hardware.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core risponde alla domanda fondamentale di un prodotto embedded:
**cosa viene eseguito dopo e può terminare entro la scadenza?** Il runtime
leggero e senza heap verifica i deadline prima dell’avvio, lasciando
all’applicazione il controllo di driver, interrupt, timer e risparmio energetico.

## Vantaggi principali

- **Ultraleggero:** la configurazione MCS-51 misurata parte da 504 byte di programma e 7 byte di dati del core.
- **Verifica prima dell’esecuzione:** rifiuta sovraccarico, timing errato, ID duplicati e tempi di risposta impossibili.
- **Strumenti flessibili:** Arduino, PlatformIO, Rust `no_std` e generazione C/assembly tramite Python.
- **Comportamento limitato:** capacità fissa, priorità esplicite e nessuno stack di task nascosto.

## Avvio rapido

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

Consulta le guide [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md),
[Python](docs/PYTHON.md) e [Rust](docs/RUST.md).

## Ottimo posizionamento tecnico

Nel modello incluso con 32 dimensioni e 19 sistemi, NobroRTOS ottiene
**7,23/10 e il terzo posto assoluto**. È una valutazione circoscritta di
architettura e capacità, non una conclusione universale su benchmark,
certificazioni o maturità commerciale.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="Classifica tecnica RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="Punteggio tecnico per categoria" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="Confronto radar RTOS" width="96%"></p>

## NobroRTOS avanzato

Offriamo anche core avanzati personalizzati per requisiti di timing, sicurezza,
connettività, IA, robotica e hardware specifico. Hanno superato uno stress test
simultaneo su 18 schede di sviluppo reali.

<p align="center"><img src="docs/images/real_tests.jpg" alt="Stress test NobroRTOS su 18 schede" width="92%"></p>

Unisciti a [Discord](https://discord.gg/NrRrQKmT2) e visita
[NiusRobotLab su YouTube](https://www.youtube.com/@NiusRobotLab).

NobroRTOS Core è distribuito con licenza **GPL-3.0-only**; fa fede [LICENSE](LICENSE).
