<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>Крошечное ядро. Серьёзное реальное время. Ваше оборудование.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core отвечает на главный вопрос встраиваемого продукта:
**что выполняется дальше и успеет ли задача завершиться вовремя?** Лёгкая
среда без кучи проверяет сроки до запуска, оставляя приложению управление
драйверами, прерываниями, таймерами и энергосбережением.

## Основные преимущества

- **Минимальный размер:** измеренная конфигурация MCS-51 занимает 504 байта программы и 7 байт данных ядра.
- **Проверка до запуска:** перегрузка, неверные интервалы, повторяющиеся ID и недостижимые сроки отклоняются заранее.
- **Гибкие инструменты:** Arduino, PlatformIO, Rust `no_std` и генерация C/ассемблера через Python.
- **Ограниченное поведение:** фиксированная ёмкость, явные приоритеты и отсутствие скрытых стеков задач.

## Быстрый старт

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

См. руководства [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md),
[Python](docs/PYTHON.md) и [Rust](docs/RUST.md).

## Сильная техническая позиция

В представленной модели из 32 измерений и 19 систем NobroRTOS получает
**7,23/10 и занимает третье место**. Это ограниченная оценка архитектуры и
возможностей, а не универсальный вывод о тестах, сертификации или зрелости рынка.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="Технический рейтинг RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="Категории технической оценки" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="Радарное сравнение RTOS" width="96%"></p>

## Расширенные ядра NobroRTOS

Мы также предлагаем специализированные расширенные ядра для требований по
времени, безопасности, связи, ИИ, робототехнике и оборудованию. Они прошли
одновременный аппаратный стресс-тест на 18 платах разработки.

<p align="center"><img src="docs/images/real_tests.jpg" alt="Стресс-тест NobroRTOS на 18 платах" width="92%"></p>

Присоединяйтесь к [Discord](https://discord.gg/NrRrQKmT2) и смотрите
[NiusRobotLab на YouTube](https://www.youtube.com/@NiusRobotLab).

NobroRTOS Core распространяется по **GPL-3.0-only**; условия определяет [LICENSE](LICENSE).
