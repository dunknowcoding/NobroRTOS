<p align="center">
  <img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%">
</p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>更小的内核，更认真的实时性，运行在你的硬件上。</strong></p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.ko.md">한국어</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português</a> ·
  <a href="README.fr.md">Français</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.it.md">Italiano</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.ar.md">العربية</a> ·
  <a href="README.hi.md">हिन्दी</a> ·
  <a href="README.id.md">Bahasa Indonesia</a>
</p>

<p align="center">
  <a href="https://github.com/dunknowcoding/NobroRTOS/actions/workflows/core.yml"><img alt="Core CI" src="https://github.com/dunknowcoding/NobroRTOS/actions/workflows/core.yml/badge.svg"></a>
  <a href="https://github.com/dunknowcoding/NobroRTOS/releases"><img alt="Release" src="https://img.shields.io/github/v/release/dunknowcoding/NobroRTOS"></a>
  <a href="https://discord.gg/NrRrQKmT2"><img alt="加入 Discord" src="https://img.shields.io/badge/Discord-加入我们-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://www.youtube.com/@NiusRobotLab"><img alt="NiusRobotLab YouTube" src="https://img.shields.io/badge/YouTube-NiusRobotLab-FF0000?logo=youtube&logoColor=white"></a>
</p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core 用一个清晰的模型回答嵌入式产品最关键的问题：
**下一项工作是什么，它能否按时完成？** 它提供无堆、截止期感知的轻量运行时，
同时把驱动、中断、定时器、睡眠策略和板级框架的控制权留给应用。

从 8 位控制器到现代 Arm 和 RISC-V 开发板，开发方式始终一致：声明有界任务，
在启动前拒绝不可行调度，并在没有隐藏任务栈和动态分配的情况下运行任务。

## 为什么选择 NobroRTOS Core

| 优势 | 对产品的意义 |
| --- | --- |
| **真正轻量** | 已测 MCS-51 最小样例仅占 504 字节程序空间和 7 字节内核数据。 |
| **启动前检查截止期** | 过载、无效时序、重复标识和保守响应时间失败会在运行前被拒绝。 |
| **开发方式自由** | 支持 Arduino、PlatformIO、Rust `no_std`，并可通过 Python 生成紧凑 C/汇编边界。 |
| **行为清晰可控** | 周期与事件任务按显式优先级运行到完成，没有隐式任务栈。 |
| **低功耗由你掌控** | 内核给出下一次释放时间，板级代码可选择 `yield`、睡眠或低功耗指令。 |

## 快速开始

```cpp
#include <NobroRTOSCore.h>

using nobro::core::Scheduler;
using nobro::core::Task;

Scheduler<1> scheduler;

void sample(void *) {
    // 读取传感器、更新控制，然后返回。
}

void setup() {
    static const Task tasks[] = {
        Task::periodic(1, 0, 1000, 1000, 40, sample),
    };
    scheduler.begin(tasks, micros());
}

void loop() {
    scheduler.releaseDue(micros());
    scheduler.runReady();
    yield();
}
```

同一个包已在代表性的 AVR、SAMD21、Renesas RA4M1、ESP32-S3、RP2040 和
ESP8266 Arduino 核心上通过编译。详细安装与 API 请查看
[Arduino 与 PlatformIO 指南](docs/ARDUINO_PLATFORMIO.md)、
[Python 指南](docs/PYTHON.md)和 [Rust 指南](docs/RUST.md)。

## 有数据支撑的轻量设计

| 公开样例 | SDCC 程序空间 | 内核数据 |
| --- | ---: | ---: |
| 单个周期任务 | 504 字节 | 7 字节 |
| 两任务、邮箱及空闲/看门狗钩子 | 725 字节 | 12 字节 |

这些数字仅对应明确命名和配置的 NobroRTOS Core 样例；它们不是对其他配置下
RTOS 的笼统性能结论。完整范围请查看[资源报告](docs/CORE_BENCHMARKS.md)。

## 纯技术评价中的领先位置

在随附的 32 维度、19 个系统技术评价模型中，NobroRTOS 获得
**7.23/10，并位列第三**。加权视图覆盖调度、内存、架构、并发、可靠性、
功能、扩展、前沿技术与工程质量；雷达图则展示了各项优势和取舍。

该结果是限定范围的架构与能力评价，不代表所有工作负载下的基准性能，
也不等同于认证等级、生态规模或商业成熟度。

<p align="center">
  <img src="docs/images/rtos_pure_technical_ranking.png" alt="19 个 RTOS 的 32 维度纯技术排名" width="96%">
</p>

<p align="center">
  <img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="RTOS 技术评分分类贡献" width="96%">
</p>

<p align="center">
  <img src="docs/images/rtos_pure_radar_comparison.png" alt="RTOS 技术分类雷达对比" width="96%">
</p>

## 进一步探索 NobroRTOS

除公开 Core 外，我们还提供针对产品时序、安全、连接、AI、机器人和特定硬件
需求定制的 NobroRTOS 高级内核。这些高级内核已通过 18 块开发板同时运行的
真实硬件压力测试。

<p align="center">
  <img src="docs/images/real_tests.jpg" alt="18 块开发板同时进行 NobroRTOS 硬件压力测试" width="92%">
</p>

欢迎加入 [Discord](https://discord.gg/NrRrQKmT2) 交流项目、提出定制需求，
并在 [NiusRobotLab YouTube 频道](https://www.youtube.com/@NiusRobotLab)
观看演示、教程和新项目。

<p align="center">
  <a href="https://discord.gg/NrRrQKmT2"><img src="docs/images/discord_niusrobotlab.jpg" alt="扫码加入 NobroRTOS Discord" width="220"></a><br>
  <strong>扫描或点击二维码加入 NobroRTOS Discord</strong>
</p>

## 许可证

NobroRTOS Core 采用 **GPL-3.0-only** 开源许可证。商业使用是允许的，
但再分发及衍生作品必须履行许可证义务；以 [LICENSE](LICENSE) 为准。
