<p align="center">
  <img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%">
</p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>小さなコア。本格的なリアルタイム性。あなたのハードウェアへ。</strong></p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <a href="https://github.com/dunknowcoding/NobroRTOS/actions/workflows/core.yml"><img alt="Core CI" src="https://github.com/dunknowcoding/NobroRTOS/actions/workflows/core.yml/badge.svg"></a>
  <a href="https://github.com/dunknowcoding/NobroRTOS/releases"><img alt="Release" src="https://img.shields.io/github/v/release/dunknowcoding/NobroRTOS"></a>
  <a href="https://discord.gg/NrRrQKmT2"><img alt="Discord に参加" src="https://img.shields.io/badge/Discord-参加する-5865F2?logo=discord&logoColor=white"></a>
  <a href="https://www.youtube.com/@NiusRobotLab"><img alt="NiusRobotLab YouTube" src="https://img.shields.io/badge/YouTube-NiusRobotLab-FF0000?logo=youtube&logoColor=white"></a>
</p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core は組み込み製品の重要な問い、つまり
**「次に何を実行し、期限内に完了できるか」**に明確な答えを与えます。
ヒープを使わない軽量ランタイムとデッドライン対応のワークロード受付を備え、
ドライバー、割り込み、タイマー、スリープ、ボードフレームワークの制御は
アプリケーション側に残します。

8 ビットコントローラーから Arm、RISC-V ボードまで、開発モデルは共通です。
有界な処理を宣言し、実行不可能なスケジュールを起動前に拒否し、
アロケーターや隠れたタスクスタックなしで実行します。

## NobroRTOS Core の特長

| 特長 | 製品にもたらす価値 |
| --- | --- |
| **小ささを前提に設計** | 計測済み MCS-51 最小構成はプログラム 504 バイト、カーネルデータ 7 バイトです。 |
| **起動前の期限判定** | 過負荷、不正なタイミング、重複 ID、応答時間違反を実行前に拒否します。 |
| **複数の開発経路** | Arduino、PlatformIO、Rust `no_std`、Python 生成の C/アセンブリ境界を選べます。 |
| **明快な実行モデル** | 周期・イベントタスクを明示優先度で run-to-completion 実行します。 |
| **省電力制御を保持** | 次のリリース時刻を受け取り、ボード側で `yield`、sleep、低電力命令を選択できます。 |

## Arduino クイックスタート

```cpp
#include <NobroRTOSCore.h>

using nobro::core::Scheduler;
using nobro::core::Task;

Scheduler<1> scheduler;

void sample(void *) {
    // センサーを読み、制御を更新して戻ります。
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

同じパッケージを AVR、SAMD21、Renesas RA4M1、ESP32-S3、RP2040、
ESP8266 の代表的な Arduino コアで検証しています。導入方法と API は
[Arduino / PlatformIO ガイド](docs/ARDUINO_PLATFORMIO.md)、
[Python ガイド](docs/PYTHON.md)、[Rust ガイド](docs/RUST.md)を参照してください。

## 計測された小ささ

| 公開フィクスチャ | SDCC プログラム | カーネルデータ |
| --- | ---: | ---: |
| 周期タスク 1 個 | 504 バイト | 7 バイト |
| 2 タスク + メールボックス + idle/watchdog hooks | 725 バイト | 12 バイト |

数値は明示された NobroRTOS Core フィクスチャに限定され、異なる構成の RTOS
全般に対する優位性を主張するものではありません。詳細は
[リソースレポート](docs/CORE_BENCHMARKS.md)にあります。

## 純技術評価での高い位置

付属する 32 次元・19 システムの技術評価モデルでは、NobroRTOS は
**7.23/10 で総合 3 位**です。スケジューリング、メモリ、アーキテクチャ、
並行性、信頼性、機能、拡張性、先端技術、エンジニアリング品質を重み付きで
評価し、レーダー図で強みとトレードオフを示します。

これは限定されたアーキテクチャ／能力評価です。すべてのワークロードでの
ベンチマーク性能、認証、エコシステム規模、商用成熟度を同一視しません。

<p align="center">
  <img src="docs/images/rtos_pure_technical_ranking.png" alt="32 次元、19 RTOS の純技術ランキング" width="96%">
</p>

<p align="center">
  <img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="RTOS 技術スコアのカテゴリ別内訳" width="96%">
</p>

<p align="center">
  <img src="docs/images/rtos_pure_radar_comparison.png" alt="RTOS 技術カテゴリのレーダー比較" width="96%">
</p>

## さらに高度な NobroRTOS へ

公開 Core に加え、製品固有のタイミング、安全性、通信、AI、ロボティクス、
ハードウェア要件に対応するカスタム高機能コアも提供します。これらの高度な
コアは、18 枚の開発ボードを同時に動作させる実機ストレス試験を通過しました。

<p align="center">
  <img src="docs/images/real_tests.jpg" alt="18 枚の開発ボードによる NobroRTOS 同時実機ストレス試験" width="92%">
</p>

[Discord](https://discord.gg/NrRrQKmT2) でプロジェクトやカスタム要件を相談し、
[NiusRobotLab YouTube チャンネル](https://www.youtube.com/@NiusRobotLab)で
デモ、チュートリアル、新しいプロジェクトをご覧ください。

<p align="center">
  <a href="https://discord.gg/NrRrQKmT2"><img src="docs/images/discord_niusrobotlab.jpg" alt="NobroRTOS Discord に参加" width="220"></a><br>
  <strong>QR コードをスキャンまたはクリックして Discord に参加</strong>
</p>

## ライセンス

NobroRTOS Core は **GPL-3.0-only** で公開されています。商用利用は可能ですが、
再配布および派生物にはライセンス上の義務があります。[LICENSE](LICENSE) が
正式な条件です。
