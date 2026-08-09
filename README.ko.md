<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>작은 코어, 진지한 실시간성, 여러분의 하드웨어.</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core는 임베디드 제품의 핵심 질문에 답합니다. **다음에 무엇을 실행하며,
기한 안에 끝낼 수 있는가?** 힙 없는 경량 런타임과 데드라인 인식 승인 기능을
제공하면서 드라이버, 인터럽트, 타이머, 절전 정책은 애플리케이션이 제어합니다.

## 핵심 장점

- **초경량:** 측정된 MCS-51 최소 구성은 프로그램 504바이트, 커널 데이터 7바이트입니다.
- **실행 전 검증:** 과부하, 잘못된 타이밍, 중복 ID와 응답 시간 실패를 시작 전에 거부합니다.
- **유연한 도구:** Arduino, PlatformIO, Rust `no_std`, Python 기반 C/어셈블리 생성을 지원합니다.
- **예측 가능한 동작:** 고정 용량, 명시적 우선순위, 숨겨진 태스크 스택이 없습니다.

## 빠른 시작

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

[Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md), [Python](docs/PYTHON.md),
[Rust](docs/RUST.md) 사용자 가이드를 확인하세요.

## 순수 기술 평가

32개 차원과 19개 시스템을 다룬 평가 모델에서 NobroRTOS는
**7.23/10으로 종합 3위**를 기록했습니다. 이는 제한된 아키텍처·기능 평가이며,
모든 워크로드의 벤치마크나 인증, 시장 성숙도를 동일시하지 않습니다.

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="RTOS 순수 기술 순위" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="RTOS 기술 점수 구성" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="RTOS 레이더 비교" width="96%"></p>

## 더 강력한 NobroRTOS

제품별 타이밍, 안전, 연결, AI, 로봇 및 하드웨어 요구를 위한 맞춤형 고급 코어도
제공합니다. 이 코어들은 18개 개발 보드를 동시에 구동한 실제 하드웨어 스트레스
테스트를 통과했습니다.

<p align="center"><img src="docs/images/real_tests.jpg" alt="18개 보드 NobroRTOS 스트레스 테스트" width="92%"></p>

[Discord](https://discord.gg/NrRrQKmT2)에 참여하고
[NiusRobotLab YouTube](https://www.youtube.com/@NiusRobotLab)에서 데모와 튜토리얼을 만나보세요.

NobroRTOS Core는 **GPL-3.0-only**로 공개되며, [LICENSE](LICENSE)가 정식 조건입니다.
