<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>Um núcleo minúsculo. Tempo real de verdade. Seu hardware.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

O NobroRTOS Core responde à pergunta central de um produto embarcado:
**o que executa em seguida e consegue terminar no prazo?** Ele combina um
runtime leve e sem heap com admissão sensível a deadlines, mantendo drivers,
interrupções, temporizadores e economia de energia sob controle da aplicação.

## Principais vantagens

- **Ultraleve:** o perfil MCS-51 medido começa com 504 bytes de programa e 7 bytes de dados do núcleo.
- **Validação antes da execução:** rejeita sobrecarga, temporização inválida, IDs duplicados e falhas de resposta.
- **Ferramentas flexíveis:** Arduino, PlatformIO, Rust `no_std` e geração C/assembly por Python.
- **Comportamento limitado:** capacidade fixa, prioridades explícitas e nenhuma pilha de tarefa oculta.

## Início rápido

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

Veja os guias de [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md),
[Python](docs/PYTHON.md) e [Rust](docs/RUST.md).

## Destaque na avaliação técnica

No modelo incluído de 32 dimensões e 19 sistemas, o NobroRTOS alcança
**7,23/10 e o terceiro lugar geral**. É uma avaliação delimitada de arquitetura
e recursos, não uma conclusão universal sobre benchmarks, certificação ou maturidade comercial.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="Ranking técnico de RTOS" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="Pontuação técnica por categoria" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="Comparação radar de RTOS" width="96%"></p>

## NobroRTOS avançado

Também fornecemos núcleos avançados personalizados para temporização, segurança,
conectividade, IA, robótica e hardware específico. Eles passaram por um teste
de estresse simultâneo em 18 placas de desenvolvimento reais.

<p align="center"><img src="docs/images/real_tests.jpg" alt="Teste NobroRTOS simultâneo em 18 placas" width="92%"></p>

Participe do [Discord](https://discord.gg/NrRrQKmT2) e acompanhe o
[NiusRobotLab no YouTube](https://www.youtube.com/@NiusRobotLab).

O NobroRTOS Core é distribuído sob **GPL-3.0-only**; consulte [LICENSE](LICENSE).
