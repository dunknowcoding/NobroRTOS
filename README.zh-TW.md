<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>更小的核心，更嚴謹的即時性，運行在你的硬體上。</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core 以清楚的模型回答嵌入式產品最重要的問題：**下一項工作是什麼，
而且能否準時完成？** 它提供無堆、截止期限感知的輕量執行環境，同時把驅動、
中斷、計時器、睡眠策略與板級框架的控制權保留給應用程式。

## 產品優勢

- **極小資源占用：** 已量測 MCS-51 最小範例為 504 位元組程式空間與 7 位元組核心資料。
- **啟動前檢查：** 過載、無效時序、重複識別與保守回應時間失敗會先被拒絕。
- **多種開發路徑：** Arduino、PlatformIO、Rust `no_std` 與 Python 產生的 C/組合語言邊界。
- **行為有界：** 固定容量、明確優先序、無隱藏工作堆疊。

## 快速開始

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

請參閱 [Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md)、
[Python](docs/PYTHON.md) 與 [Rust](docs/RUST.md) 使用指南。

## 純技術評價

在隨附的 32 維度、19 系統評價模型中，NobroRTOS 得分 **7.23/10，排名第三**。
這是限定範圍的架構與能力評價，不等同於所有負載下的基準、認證或市場成熟度。

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="RTOS 純技術排名" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="RTOS 分類技術分數" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="RTOS 雷達比較" width="96%"></p>

## 進階 NobroRTOS

我們也提供針對時序、安全、連線、AI、機器人與特定硬體需求的客製化進階核心；
這些核心已通過 18 塊開發板同時運行的實機壓力測試。

<p align="center"><img src="docs/images/real_tests.jpg" alt="18 塊開發板的 NobroRTOS 壓力測試" width="92%"></p>

加入 [Discord](https://discord.gg/NrRrQKmT2)，並在
[NiusRobotLab YouTube](https://www.youtube.com/@NiusRobotLab) 觀看示範與教學。

NobroRTOS Core 依 **GPL-3.0-only** 發布；正式條款以 [LICENSE](LICENSE) 為準。
