# AIRON 系统架构设计及开发计划书 (v2.0)

**项目代号（正式名称待定）**：AIRON (AI-IoT-Robotics-OS Nexus / 异构边缘计算基座)  
**目标硬件**：nRF52840 (Cortex-M4, 256 KB Flash, 128 KB RAM)  
**工程目录名**：`aion/`（与代号 AIRON 并存，下同）  
**核心哲学**：瘦内核 + 通用 SAL 接口 + 薄适配 + Deadline 感知调度 + 零拷贝 `Sample` 管道 + 模块级差异化容错  

**文档版本**：v2.1（2026-06-16）  
**相对 v1.0 主要变更**：明确非 PC OS 定位；兼容优先级；6 个通用 SAL；Phase 1 **优先 ArduinoNRF 板库联调与资源调度验证**；PC 上位机时序契约；路线图现实化。

---

## 一、战略定位

### 1.1 是什么 / 不是什么

| 维度 | PC 通用 OS | AIRON |
|------|-----------|-------|
| 目标 | 替代 Linux、跑百万级软件包 | 在 nRF52840 上跑电机 PID + 传感器 + 一种无线 + 可选 AI |
| 成功关键 | libc / POSIX / 编译器生态 | **ArduinoNRF HAL 语义、RADIO 独占、协议栈 RAM、上位机时序** |
| 内核工作量 | 约 20%（参见 [Tony Bai：内核之外的冰山](https://tonybai.com/2025/08/16/brand-new-os-impossible/)） | 同样约 20%；**80% 是与 ArduinoNRF 生态、driver/ 库、PC 工具对接** |

AIRON **不是** PC 操作系统。参考 Tony Bai 博文的警示：**生态才是护城河**——嵌入式语境下体现为 HAL 一致、板级分区、卫星库 FFI、烧录/GDB/串口时序，而非 POSIX/libc。

### 1.2 要解决的问题

- **调度撕裂**：传统 RTOS 无线高优先级 ISR 导致电机 PID 抖动（±50~200 µs）。
- **数据拷贝**：传感器/无线 DMA 到 AI 路径间多次 memcpy。
- **编程模型割裂**：回调/状态机 vs 原生 `async/await`。
- **生态分裂**：自研内核与现有 ArduinoNRF / driver 库无法复用。

### 1.3 竞争优势

- **Deadline** 作为调度一等公民。
- **DPPI** 零 CPU 干预的时间戳锁存。
- **6 个通用 SAL** + 编译时静态绑定，无运行时插件开销。
- **与 ArduinoNRF 板库对齐**，降低迁移与验证成本。

---

## 二、兼容战略（三级）

### 2.1 P0 — ArduinoNRF 及其附属库（第一优先）

AIRON 的 L1 必须与 [ArduinoNRF](../ArduinoNRF) 在寄存器语义、资源独占、分区布局、USB 双 CDC 模型上 **一致**。卫星库通过 `adapters/*` 薄适配挂载到通用 SAL，不重写协议栈。

**ArduinoNRF Core 对齐项**

| 能力 | ArduinoNRF 资产 | AIRON 模块 |
|------|-----------------|------------|
| 寄存器 HAL | `NrfPeripherals.h`、`wiring.cpp` | `airon-hal` |
| 资源独占 | `PeripheralLease`、`RADIO_POLICY.md` | `ResourceLease` |
| 板级 / 分区 | `boards.txt`（ProMicro `0x1000` 等） | `airon-hal` feature |
| PWM / 时间 | `nrfPwm*`、`NrfTimer`、PPI | `airon-hal` + 内建 `ActuatorSal` |
| 上传 / 调试 | 1200 bps touch、双 CDC、GDB stub | `host-contract` + `StreamSal` |

**外部卫星库 → 通用 SAL**

| 库 | 路径 | SAL | adapter |
|----|------|-----|---------|
| NiusIMU | ArduinoNRF-IMU | BusSal + SensorSal | `adapters/nius-imu` |
| NiusThread | ArduinoNRF-Thread | RadioSal | `adapters/nius-thread` |
| NiusCrypto | ArduinoNRF-Crypto | CryptoSal | `adapters/nius-crypto` |
| NiusZigbee | ArduinoNRF-Zigbee | BusSal (UART) | `adapters/nius-zigbee` |
| NimBLE | ArduinoNRF 包内 | RadioSal | `adapters/nimble` |

### 2.2 P1 — driver/ 其余库

| 库 | SAL | adapter | 备注 |
|----|-----|---------|------|
| RoboServo | ActuatorSal | `adapters/robo-servo` | nRF `RoboPwmBackend` |
| DiFinders | SensorSal + BusSal | `adapters/difinders` | 超声 echo 用 GPIOTE+CAPTURE |
| INA_series_sensor | SensorSal + StreamSal | `adapters/ina-jsonl` | JSONL 对接 INA_monitor |
| INA_monitor | —（PC Electron） | host schema | 串口 JSONL |
| CP210x 驱动 | —（PC） | 文档 | 外置 UART |

### 2.3 P2 — IronEngine 系列（Tiezhu）

- `adapters/iron-bridge`：`StreamSal`（0xAA 0x55 + CRC16）+ `ActuatorSal` 转发 `servo_set`。
- 优先级低于 ArduinoNRF；Phase 3 交付。

### 2.4 P3 — 成熟外栈（按需）

TFLite Micro、nCS Zigbee R23 sidecar（见 `arduinonrf_improve.md`）、LittleFS、WAMR（Phase 4 可选）。

**明确不做**：完整 Arduino `setup()/loop()` 运行时、SoftDevice 共存、多 RADIO 协议并行。

---

## 三、模块化设计原则

**总方针：瘦内核、通用接口、薄适配、编译时绑定。**

| 原则 | 要求 | 禁止 |
|------|------|------|
| 模块化 | L2 内核不可拆；L3 整包 feature 移除；adapter 独立 crate | 每库一套私有 IPC |
| 简洁 | **≤6 个 SAL trait** + 1 个 `Sample` 信封 | 第 7 SAL、运行时插件 |
| 高效 | 热路径静态分发；ISR 仅 CAPTURE；零拷贝 `PoolHandle` | 热路径 `dyn Trait`、ISR 内堆分配 |
| 通用接口 | 应用只 `use airon_sal::*` | 直接 `#include NiusIMU.h` |

### 3.1 六个通用 SAL（全系统唯一对外能力接口）

| Trait | 职责 | 挂载示例 |
|-------|------|----------|
| **BusSal** | I2C / SPI / UART + LeaseGuard | NiusIMU、INA、DiFinders、CC2530 |
| **StreamSal** | 成帧字节流 poll/read_frame/write_frame | IronEngine、INA JSONL |
| **RadioSal** | 802.15.4 / BLE + process 泵；编译互斥 | NimBLE、NiusThread |
| **ActuatorSal** | set_duty_us(channel, us, deadline) | PWM、RoboServo、IronBridge |
| **SensorSal** | poll() -> Option\<Sample\> | IMU、ToF、INA 直读 |
| **CryptoSal** | 哈希 / 对称 / 非对称 | NiusCrypto CC310 / OnChip |

KV 持久化为 **`airon-kernel` 内 kv_get/kv_set**（<200 LoC），不增设第 7 SAL。

### 3.2 统一数据信封 `Sample`（原 TensorBuffer 泛化）

```rust
// airon-kernel/src/sample.rs
pub struct Sample {
    pub handle: PoolHandle,
    pub len: u16,
    pub kind: SampleKind,   // Imu | Range | Power | RadioRx | Tensor | Raw
    pub captured_us: u64,     // DPPI 锁存
    pub deadline_us: u64,
}
```

### 3.3 工程目录

```text
aion/
├── crates/
│   ├── airon-hal/       # L1 寄存器 + ResourceLease + 板级
│   ├── airon-kernel/    # L2 调度 + Sample + ErrorPolicy + KV
│   ├── airon-sal/       # 6 trait 定义（无实现）
│   └── airon-host/      # host-contract schema
├── adapters/            # 薄适配，每库一 crate，通常 <500 LoC
├── apps/                # 仅组合 features
└── host/
    └── airon-host-contract.json
```

---

## 四、总体架构

| 层级 | crate | 职责 |
|------|-------|------|
| L4 | `apps/*` | 固件入口，仅依赖 airon-sal + airon-kernel |
| L3 | `airon-sal` | 6 trait 定义 |
| L3′ | `adapters/*` | 现有 C/Arduino 库 → SAL |
| L2 | `airon-kernel` | Deadline 调度、Sample 池、ErrorPolicy |
| L1 | `airon-hal` | 寄存器、Lease、DPPI、TIMER0 |
| Host | `airon-host` | PC 烧录/调试/串口契约 |

### 4.1 ErrorPolicy（保留 v1.0）

| 模块 | 错误场景 | 动作 |
|------|----------|------|
| 电机 | 堵转 / 过流 | NotifyUserTask |
| 无线 | 发送失败 / ACK 超时 | RetryDelay(1000 µs)，超限 NotifyUserTask |
| WASM（可选） | 段错误 | RebootModule |
| 传感器 I2C | 读超时 | Ignore + 日志 |

### 4.2 硬件铁律（nRF52840）

- **TIMER0 + HFXO**：64 位 µs 单调时钟，上电归零后不重置。
- **DPPI**：GPIO / 无线 RX → TIMER CAPTURE，CPU 零干预。
- **RX ISR**：仅 DMA 取包 + DPPI；**禁止**协议解析。
- **调度器时间槽 ISR**：最高优先级。

### 4.3 内存 Profile（Cargo features）

| Profile | adapter | 目标 Flash |
|---------|---------|------------|
| airon-core | 无（HAL + kernel + 内建 PWM） | <80 KB |
| +adapter-nimble | nimble | +120 KB |
| +adapter-nius-imu | nius-imu | +30 KB |
| +adapter-robo-servo | robo-servo | +5 KB |
| +adapter-ina-jsonl | ina-jsonl | +8 KB |
| +adapter-iron-bridge | iron-bridge | +10 KB |

默认验证板：**AliExpress ProMicro nRF52840**（`0x1000`，与 ArduinoNRF 硬件验证路径一致）。

---

## 五、系统资源调度模型（Phase 1 核心）

AIRON 的 `ResourceLease` 对齐 ArduinoNRF `PeripheralLease`，覆盖：

| 资源 | 独占规则 | 参考文档 |
|------|----------|----------|
| RADIO | NimBLE / OpenThread / onboard Zigbee **三选一** | `RADIO_POLICY.md` |
| TWIM0/1、SPIM0 | 总线 lease，冲突返回 Err | `PeripheralLease.cpp` |
| RTC2、TIMER3 | OpenThread 占用时不得他用 | `THREAD.md` |
| PWM0~3 | 同模块共享频率组 | `PWM_MULTI_MODULE.md` |
| TIMER0 | 系统时基，内核独占 | 本文 §4.2 |

**调度器职责**

1. **Deadline 时间槽**：硬实时任务（PID / PWM 更新）最高优先级。
2. **async 任务**：RadioSal.process、SensorSal.poll、StreamSal 解析 — 各自 executor，不得阻塞时间槽。
3. **Lease 审计**：debug build 下 lease 冲突 RTT 告警；release 顺序与 ArduinoNRF 一致。

**Phase 1 资源调度验收场景（`apps/resource_sched_demo`）**

| 场景 | 并发资源 | 通过标准 |
|------|----------|----------|
| A：PWM + I2C | ActuatorSal 50 Hz + BusSal 读假设备/MPU | PID 抖动 < ±10 µs（无 GDB） |
| B：Lease 冲突注入 | 两任务争 TWIM0 | 后者 Err，无总线死锁 |
| C：RX 负载 | 模拟 Radio RX ISR + DPPI | CAPTURE 与示波器 < ±2 µs |
| D：与 ArduinoNRF 对照 | 同等 sketch 换 AIRON 镜像 | 引脚 / 频率 / 分区行为一致 |

---

## 六、开发路线图

### Phase 0 — 硬件基石 + 接口定稿（1~2 周）

**目标**：MVK + 工程脚手架 + SAL 契约冻结。

| 项 | 交付物 |
|----|--------|
| 时间基准 | `timer.rs`：TIMER0+HFXO `now()`；DPPI CAPTURE MVK |
| 接口 | `airon-sal` 六 trait + `Sample` 类型定稿 |
| HAL 骨架 | `ResourceLease` 枚举与 acquire/release API |
| 板级 | ProMicro linker script，与 `boards.txt` 分区一致 |
| 工具 | `rustup target add thumbv7em-none-eabihf`；probe-rs flash |

**验收**：GPIO 翻转 vs RTT 时间戳误差 **< ±2 µs**。

---

### Phase 1 — ArduinoNRF 板库联调 + 系统资源调度验证（2~3 周）【当前优先】

**目标**：在 **不依赖完整卫星 adapter** 的前提下，证明 AIRON 与 ArduinoNRF 板库在 HAL、分区、资源独占、Deadline 调度上 **等价可替换**；完成系统资源调度联调。

**1.1 板库 / HAL 对齐**

- [ ] `airon-hal` 寄存器常量与 `NrfPeripherals.h` 对照表（TIMER/PPI/PWM/GPIOTE）
- [ ] `boards/promicro_nrf52840`：引脚 map、Flash `0x1000`、RAM 256 KB
- [ ] PWM：4 模块 / 16 通道 / 频率组语义与 `wiring.cpp` 一致
- [ ] USB 双 CDC 布局文档化（maintenance vs user）；与 `upload.ps1` 端口策略兼容

**1.2 资源调度联调**

- [ ] `ResourceLease` 实现 TWIM0/1、SPIM0、RADIO、TIMER0 独占
- [ ] Deadline 调度器 + 2+ async 任务（Embassy 风格 `Timer::after`）
- [ ] `apps/resource_sched_demo`：场景 A~D（§5）
- [ ] defmt+RTT 输出 lease 事件与 deadline miss 计数

**1.3 上位机最小联调**

- [ ] `host/airon-host-contract.json` 初版
- [ ] probe-rs 烧录 + 可选 UF2 路径文档
- [ ] **deadline 基准**：RTT-only，禁止 GDB halt

**1.4 本阶段不做**

- 完整 NimBLE / NiusIMU adapter（留 Phase 2）
- IronEngine / RoboServo / INA JSONL
- TFLite / WAMR / ZBOSS in-process

**Phase 1 验收清单**

- [ ] **board1**（J-Link）上 AIRON 镜像可烧录运行，分区 `@ 0x1000` 与 ArduinoNRF 不冲突；**未重刷 bootloader**
- [ ] 场景 A~C 全部 PASS；场景 D 引脚/PWM 行为对照 PASS
- [ ] RADIO lease 在未启用无线时仍可 acquire/release 无泄漏
- [ ] `airon-core` Flash **< 80 KB**

---

### Phase 2 — 卫星 adapter + driver 传感器（3~4 周）

- `adapters/nimble` **或** `adapters/nius-imu`（二选一先行）
- `adapters/robo-servo`、`adapters/difinders`、`adapters/ina-jsonl`
- 第二个 RadioSal；`adapters/nius-crypto`
- INA_monitor 桌面联调
- 验收：无线 + 电机 PID 同跑，抖动较「RX ISR 内解析」对照组改善

---

### Phase 3 — IronEngine + 剩余 adapter（2~3 周）

- `adapters/iron-bridge`（StreamSal + ActuatorSal）
- `adapters/nius-zigbee` 或 nCS sidecar 评估
- IronEngine-RL serial HIL

---

### Phase 4 — AI / OTA / 多节点（后续）

- TFLite Micro + LittleFS
- Beacon 时间同步
- WAMR 可选（非硬实时）

---

## 七、PC 上位机：接口、时序与中断

### 7.1 烧录与端口

| 机制 | 要求 |
|------|------|
| 双 CDC | IronEngine / INA 仅用 **user 口**；上传用 **maintenance 口** |
| 1200 bps touch | UF2/DFU 路径复用 ArduinoNRF 语义，或文档声明 probe-rs-only |
| 上传锁 | 继承 `upload.ps1` per-port lock |
| probe-rs vs 无线 | OT/BLE 活跃时禁止 SWD mass erase |

### 7.2 GDB 与硬实时

- 功能调试：可用 USB GDB stub。
- **Deadline / 无线 soak 基准**：RTT-only，无 GDB halt。

### 7.3 host-contract 示例

```json
{
  "cdc": { "maintenance_mi": "MI_00", "user_mi": "MI_02" },
  "upload": { "touch_baud": 1200, "lock_per_port": true },
  "debug": { "gdb_incompatible_with": ["deadline_benchmark", "radio_soak"] },
  "ironengine": { "default_baud": 115200, "frame_headers": [170, 85] },
  "ina_monitor": { "jsonl_line_rate_hz_max": 50 }
}
```

---

## 八、技术难点（雷区）

| 难点 | 策略 |
|------|------|
| Tickless + 绝对计时 | TIMER0(HFXO) + RTC(LFCLK) 双时基；低功耗单独 profile |
| 分布式时间同步 | Phase 4+；802.15.4 Beacon 漂移补偿 |
| C/Rust FFI | bindgen；静态池；adapter 内禁止 malloc |
| 内存不可全开 | feature 互斥；OpenThread ~226 KB 参考 |
| ZBOSS in-process | **不做首版**；sidecar 路线见 `arduinonrf_improve.md` |
| 生态冰山 | 80% 工作量在 adapter + 板级 + 上位机，非内核 |

---

## 九、环境准备与下一步动作

```bash
rustup target add thumbv7em-none-eabihf
cargo install probe-rs --features cli

mkdir -p aion/crates/{airon-hal,airon-kernel,airon-sal,airon-host}/src
mkdir -p aion/adapters aion/apps/resource_sched_demo aion/host
```

**Phase 0 立即动作**

1. 实现 `airon-hal/src/timer.rs`：`fn now() -> u64`（TIMER0 + HFXO）
2. 实现 DPPI CAPTURE MVK + RTT 循环打印
3. 冻结 `airon-sal` 六 trait 签名
4. 编写 `airon-hal` vs `NrfPeripherals.h` 对齐表

**Phase 1 立即动作（紧随 Phase 0）**

1. 实现 `ResourceLease` 与 `apps/resource_sched_demo`
2. 对照 ArduinoNRF ProMicro 跑场景 A~D
3. 提交 `host/airon-host-contract.json` 初版

---

## 十、成功标准

**Phase 1（当前门槛）**

- ArduinoNRF ProMicro 分区可烧录运行
- 资源调度场景 A~C PASS；与 ArduinoNRF 引脚/PWM 对照 PASS
- `airon-core` Flash < 80 KB；DPPI 误差 < ±2 µs

**Phase 2+**

- NimBLE 或 NiusIMU 功能等价 Arduino sketch
- RoboServo / INA JSONL / DiFinders 经 SAL 可用
- IronEngine `servo_set` CRC 通过

**模块化**

- 示例 app 零直接引用卫星库符号
- 每个 adapter < 500 LoC（不含 vendored C）

---

## 十一、版本记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1.0 | 2026-06-16 | 初版：TensorBuffer、MVK 三阶段、ZBOSS 首集成 |
| v2.0 | 2026-06-16 | 完整重写：非 PC OS、三级兼容、6 SAL、Sample、Phase 1 优先 ArduinoNRF 板库联调与资源调度、PC host-contract、路线图 Phase 0~4 |
| v2.1 | 2026-06-16 | §十二 实验室 5 板清单、board1+J-Link 优先、禁止重刷 bootloader、AIRON/_work 临时根、IronEngineWorld、本地 commit |

**待定**：Rust 依赖版本在初始化 `Cargo.toml` 时锁定；正式产品名称。

---

## 十二、实验室硬件与工程规范（执行期约束）

本节描述当前开发机已连接硬件、工作区卫生、工具链与提交策略。**所有 Agent 测试与构建必须遵守。**

### 12.1 已连接开发板清单（5× ProMicro nRF52840）

| 别名 | Bootloader 布局 | 当前状态 | 调试 / 烧录 | CC2530 |
|------|-----------------|----------|-------------|--------|
| **board1** | no-SoftDevice（app `@ 0x1000`） | **user mode** | **SEGGER J-Link 已接 — 测试首选** | 有，可用 NiusZigbee |
| **board2** | no-SoftDevice | user mode | USB / UF2 | 有 |
| **board3** | no-SoftDevice | user mode | USB / UF2 | 有 |
| **board4** | SoftDevice S140 v6（app `@ 0x26000`） | user mode | USB / UF2 | 有 |
| **board5** | SoftDevice S140 v6 | **DFU mode（可见盘符）** | UF2 _mass storage_ | 有 |

布局定义与 [board.json](../ArduinoNRF/hardware/arduinonrf/nrf52/tools/ncs_zigbee/boards/promicro_nrf52840/board.json) 一致。

**测试优先级**

1. **默认使用 board1**：J-Link SWD 可快速迭代、可做 RTT/GDB，且 VALIDATION 已大量以 board1 为基准。
2. **禁止重刷 bootloader**：仅应用区烧录；不得 `recover` / `eraseall` / 覆盖 MBR 与 bootloader 向量（与 `bootloader_policy: do-not-reflash` 一致）。
3. board1–board3 链接脚本/feature 用 **`no-softdevice` / `0x1000`**；board4–board5 用 **`softdevice-s140-v6` / `0x26000`**。
4. board5 处于 DFU 时可用于 UF2 流程验证；**不要**对其做全片擦除。
5. 五板均接 CC2530 → Phase 2+ 可做多节点 NiusZigbee / Zigbee 联调；Phase 1 不依赖 CC2530。

**AIRON Phase 1 默认目标板**：**board1**（J-Link + no-SD + `0x1000`）。

### 12.2 工作区与临时文件（保持 workspace 清洁）

**原则：所有临时资产收口到 `AIRON/` 目录，不散落 repo 根目录。**

```text
AIRON/
├── technique_route.md          # 本文档
├── .gitignore                  # 忽略 _work/ 等（执行 Phase 0 时创建）
├── _work/                      # 【唯一临时根】— 禁止提交
│   ├── toolchain/              # 本机缺失时自行下载的工具链（probe-rs、gcc 备份等）
│   ├── cargo-target/           # CARGO_TARGET_DIR 指向此处
│   ├── downloads/              # 通用下载缓存
│   ├── ncs/                    # nCS/Zephyr 拉取（若需 sidecar 构建）
│   └── logs/                   # 测试日志、示波器截图等
├── aion/                       # Rust 工程（Phase 0 创建）
└── host/
    └── airon-host-contract.json
```

**板库侧下载目录**（与 ArduinoNRF 构建脚本对齐时）：

- 路径：`ArduinoNRF/hardware/arduinonrf/nrf52/tools/.airon-work/`（执行时创建）
- 写入 **`.gitignore`**，且 README 注明：大文件优先 symlink 或环境变量 `AIRON_WORK_ROOT` 指到 `AIRON/_work/`，避免双份缓存。
- 已有惯例：`.ncs-zigbee-work/`（见 ZIGBEE.md）— 新任务统一迁移/等效到 `AIRON/_work/ncs/`。

**清理策略**

- 每次完整功能模块验收后：删除 `_work/cargo-target/*/debug/deps` 等中间产物；保留最后一次 release 固件 hex 至 `_work/artifacts/`（可选，仍 gitignore）。
- CI / 本地脚本提供 `AIRON/_work/clean.ps1`：清 `target/`、旧 hex、>7 天 logs（Phase 0 脚手架时添加）。

### 12.3 工具链与环境

| 用途 | 环境 | 说明 |
|------|------|------|
| Rust / AIRON 固件 | `rustup` + `thumbv7em-none-eabihf` | `CARGO_TARGET_DIR=AIRON/_work/cargo-target` |
| SWD 烧录 / RTT | probe-rs 或 J-Link（board1） | 优先 J-Link 应用区 flash，不重刷 bootloader |
| Python / west / nCS 构建 | **conda `IronEngineWorld`** | 与 `build_zigbee.ps1`、`arduinonrf_improve.md` 一致 |
| Arduino 对照测试 | arduino-cli + ArduinoNRF 板包 | 场景 D 对照 sketch |

**本机缺工具时**：Agent 自行下载安装至 `AIRON/_work/toolchain/`，并在 `technique_route.md` 或 `_work/README.md` 记录版本与路径；**不得**修改全局 git config。

激活 conda 示例（路径以本机为准，常见 `G:\Anaconda\envs\IronEngineWorld`）：

```powershell
conda activate IronEngineWorld
$env:AIRON_WORK_ROOT = "F:\Arduino\driver\AIRON\_work"
$env:CARGO_TARGET_DIR = "$env:AIRON_WORK_ROOT\cargo-target"
```

### 12.4 版本管理与提交

- **尚无 GitHub remote**：仅在本地 `driver` 仓库 commit。
- **粒度**：每完成一个 **完整功能模块**（如 Phase 0 MVK、Phase 1 ResourceLease、单个 adapter crate）做一次 commit。
- **禁止提交**：`AIRON/_work/`、`AIRON/aion/**/target/`、`_work/ncs/`、下载的 toolchain、临时 hex。
- commit 前运行清理脚本，确保 diff 仅含源码与文档。

### 12.5 测试分工建议（5 板）

| 板 | 建议用途 |
|----|----------|
| board1 | AIRON 主开发、J-Link、Phase 1 资源调度、deadline 基准 |
| board2–3 | 多板 UF2 / 上传锁 / 无 SD 回归 |
| board4 | SoftDevice 布局 `0x26000` 链接验证 |
| board5 | DFU / UF2 盘符流程（慎用，避免误擦 bootloader 区） |
| 五板 CC2530 | Phase 2+ NiusZigbee 双节点 / 三节点 raw 802.15.4 |
