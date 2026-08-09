<p align="center"><img src="docs/images/Nobro_full.png" alt="NobroRTOS" width="100%"></p>

<h1 align="center">NobroRTOS Core</h1>

<p align="center"><strong>बहुत छोटा कोर। गंभीर रियल-टाइम नियंत्रण। आपका हार्डवेयर।</strong></p>

<p align="center"><a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.zh-TW.md">繁體中文</a> · <a href="README.ja.md">日本語</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.pt-BR.md">Português</a> · <a href="README.fr.md">Français</a> · <a href="README.de.md">Deutsch</a> · <a href="README.it.md">Italiano</a> · <a href="README.ru.md">Русский</a> · <a href="README.ar.md">العربية</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.id.md">Bahasa Indonesia</a></p>

<p align="center">中文名：<strong>糯哥RTOS</strong> — 面向 AI、机器人、IoT 与智能控制的下一代超轻量嵌入式实时操作系统。</p>

NobroRTOS Core एम्बेडेड उत्पाद के मुख्य प्रश्न का उत्तर देता है: **अगला काम
कौन-सा है और क्या वह समय पर पूरा होगा?** इसका हल्का, heap-रहित रनटाइम deadline
को शुरू होने से पहले जाँचता है, जबकि ड्राइवर, interrupt, timer और power नीति
एप्लिकेशन के नियंत्रण में रहती है।

## मुख्य लाभ

- **अत्यंत हल्का:** मापे गए MCS-51 न्यूनतम रूप में 504 बाइट प्रोग्राम और 7 बाइट कोर डेटा लगता है।
- **चलने से पहले सत्यापन:** overload, गलत timing, duplicate ID और असंभव response time अस्वीकार होते हैं।
- **लचीले उपकरण:** Arduino, PlatformIO, Rust `no_std` और Python से C/assembly निर्माण।
- **सीमित और स्पष्ट व्यवहार:** fixed capacity, स्पष्ट priority और कोई छिपा task stack नहीं।

## तुरंत शुरू करें

```text
python -m pip install nobro_rtos
nobro-core ports/byte/examples/useful/app.json --out generated
```

[Arduino / PlatformIO](docs/ARDUINO_PLATFORMIO.md), [Python](docs/PYTHON.md)
और [Rust](docs/RUST.md) मार्गदर्शिका देखें।

## मजबूत तकनीकी स्थान

32 आयाम और 19 प्रणालियों वाले संलग्न मूल्यांकन मॉडल में NobroRTOS को
**7.23/10 और तीसरा स्थान** मिला है। यह architecture और capability का सीमित
मूल्यांकन है; हर workload के benchmark, certification या बाजार परिपक्वता का सार्वभौमिक दावा नहीं।

<p align="center"><img src="docs/images/rtos_pure_technical_ranking.png" alt="RTOS तकनीकी रैंकिंग" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_technical_bar_ranking.png" alt="RTOS तकनीकी स्कोर विभाजन" width="96%"></p>
<p align="center"><img src="docs/images/rtos_pure_radar_comparison.png" alt="RTOS रडार तुलना" width="96%"></p>

## उन्नत NobroRTOS

हम timing, safety, connectivity, AI, robotics और विशेष hardware आवश्यकताओं के
लिए अनुकूलित उन्नत कोर भी देते हैं। इन कोर ने 18 वास्तविक development boards
पर एक साथ hardware stress test पास किया है।

<p align="center"><img src="docs/images/real_tests.jpg" alt="18 बोर्ड NobroRTOS stress test" width="92%"></p>

[Discord](https://discord.gg/NrRrQKmT2) से जुड़ें और
[NiusRobotLab YouTube](https://www.youtube.com/@NiusRobotLab) देखें।

NobroRTOS Core **GPL-3.0-only** के अंतर्गत है; आधिकारिक शर्तें [LICENSE](LICENSE) में हैं।
